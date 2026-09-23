//! Isolated first-peer enrollment and published snapshot retrieval, not shipping sync.
//! The public mailbox never exposes bootstrap chunks, images or credentials.
use crate::{protected_local_keys::ProtectedLocalKeyStore, seed_bootstrap_http};
use anyhow::{Result, ensure};
use aven_core::{
    db::Database,
    sync::seed_claim::{
        Secret,
        peer::{self, Evidence, Invitation, Mailbox},
    },
};
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const PATH: &str = "/e2ee/enrollment/v1";
const LIMIT: usize = 4 * (1_048_576 + 222) + 4096;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    vault: [u8; 32],
    genesis: [u8; 32],
    device: [u8; 32],
    credential_version: u32,
    head: [u8; 32],
}
impl Context {
    fn auth<'a>(&self, bearer: &'a Secret) -> peer::Authentication<'a> {
        peer::Authentication {
            vault: self.vault,
            genesis: self.genesis,
            device: self.device,
            credential_version: self.credential_version,
            head: self.head,
            bearer,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Operation {
    Register {
        context: Context,
        declaration: Vec<u8>,
    },
    Post {
        vault: [u8; 32],
        handle: [u8; 32],
        request: Vec<u8>,
    },
    Mailbox {
        vault: [u8; 32],
        handle: [u8; 32],
    },
    Admit {
        context: Context,
        record: Vec<u8>,
    },
    Descriptor {
        context: Context,
    },
    Published {
        context: Context,
        descriptor: [u8; 32],
        component: Option<aven_core::sync::bootstrap_staging::Component>,
        index: u64,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Reply {
    Done,
    Registered(peer::RegistrationStatus),
    Mailbox(Mailbox),
    Admitted(Evidence),
    Descriptor(Vec<u8>),
    Published(Vec<u8>),
}
struct Server {
    db: Database,
    gate: tokio::sync::Semaphore,
}
/// Merge only into an isolated E2EE router, never the legacy plaintext server.
pub fn router(db: Database) -> Router {
    Router::new()
        .route(PATH, post(handle))
        .with_state(Arc::new(Server {
            db,
            gate: tokio::sync::Semaphore::new(1),
        }))
}
async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let Ok(_permit) = server.gate.try_acquire() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "enrollment-busy").into_response();
    };
    let mut response = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        dispatch(&server.db, request),
    )
    .await
    {
        Ok(Ok(reply)) => axum::Json(reply).into_response(),
        _ => (StatusCode::BAD_REQUEST, "enrollment-refused").into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
async fn dispatch(db: &Database, request: Request) -> Result<Reply> {
    ensure!(
        request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            == Some("application/json")
            && !request.headers().contains_key(header::CONTENT_ENCODING),
        "error enrollment-http"
    );
    let credential = request
        .headers()
        .get(header::AUTHORIZATION)
        .map(|h| {
            let text = h
                .to_str()
                .map_err(|_| anyhow::anyhow!("error enrollment-credential"))?;
            let hex = text
                .strip_prefix("Bearer ")
                .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?;
            ensure!(
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "error enrollment-credential"
            );
            Ok::<_, anyhow::Error>(Secret::new(
                hex::decode(hex)?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("error enrollment-credential"))?,
            ))
        })
        .transpose()?;
    let bytes = to_bytes(request.into_body(), LIMIT)
        .await
        .map_err(|_| anyhow::anyhow!("error enrollment-limit"))?;
    let op: Operation =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("error enrollment-http"))?;
    Ok(match op {
        Operation::Post {
            vault,
            handle,
            request,
        } => {
            db.post_peer_request(vault, handle, &request).await?;
            Reply::Done
        }
        Operation::Mailbox { vault, handle } => {
            Reply::Mailbox(db.peer_mailbox(vault, handle).await?)
        }
        Operation::Register {
            context,
            declaration,
        } => Reply::Registered(
            db.register_peer_invitation(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
                ),
                &declaration,
            )
            .await?,
        ),
        Operation::Admit { context, record } => Reply::Admitted(
            db.admit_first_peer(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
                ),
                &record,
            )
            .await?,
        ),
        Operation::Published {
            context,
            descriptor,
            component,
            index,
        } => Reply::Published(
            db.published_snapshot_read(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
                ),
                descriptor,
                component,
                index,
            )
            .await?,
        ),
        Operation::Descriptor { context } => Reply::Descriptor(
            db.peer_enrollment_descriptor(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
                ),
            )
            .await?,
        ),
    })
}

/// One bounded exchange at a time; callers explicitly retry the same protected intent.
pub struct Client {
    transport: seed_bootstrap_http::Client,
    locator: String,
}
impl Client {
    pub fn new(origin: &str) -> Result<Self> {
        ensure!(origin.len() <= 2048, "error enrollment-locator-limit");
        let mut transport = seed_bootstrap_http::Client::new(origin)?;
        transport.endpoint.set_path(PATH);
        Ok(Self {
            transport,
            locator: origin.into(),
        })
    }
    async fn exchange(&self, op: Operation, secret: Option<&Secret>) -> Result<Reply> {
        let bytes =
            serde_json::to_vec(&op).map_err(|_| anyhow::anyhow!("error enrollment-http"))?;
        ensure!(bytes.len() <= LIMIT, "error enrollment-limit");
        let mut request = self
            .transport
            .http
            .post(self.transport.endpoint.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes);
        if let Some(secret) = secret {
            let mut value = reqwest::header::HeaderValue::from_str(&format!(
                "Bearer {}",
                hex::encode(secret.expose())
            ))
            .map_err(|_| anyhow::anyhow!("error enrollment-credential"))?;
            value.set_sensitive(true);
            request = request.header(header::AUTHORIZATION, value);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("error enrollment-network outcome-unknown"))?;
        ensure!(
            response.status() == StatusCode::OK
                && response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|h| h.to_str().ok())
                    == Some("application/json")
                && !response.headers().contains_key(header::CONTENT_ENCODING),
            "error enrollment-refused outcome-unknown"
        );
        ensure!(
            response.content_length().is_none_or(|n| n <= LIMIT as u64),
            "error enrollment-limit"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("error enrollment-network outcome-unknown"))?
        {
            ensure!(chunk.len() <= LIMIT - bytes.len(), "error enrollment-limit");
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("error enrollment-http"))
    }
    /// Installs only a protected, independently enrolled peer into a fresh target.
    /// A completed retry is local and never rewinds later changes.
    pub async fn install(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &std::path::Path,
    ) -> Result<aven_core::sync::SharedStateInstallReport> {
        store
            .install_peer_snapshot(db, self, &self.locator, blob_dir)
            .await
    }

    pub(crate) async fn download(
        &self,
        peer: &peer::PeerAuthority,
        verified: &peer::VerifiedEnrollment,
        descriptor: &[u8],
    ) -> Result<aven_core::sync::bootstrap_format::Package> {
        use aven_core::sync::{
            bootstrap_format::{self, ImageRecords, Package},
            bootstrap_staging::{Component, MAX_CHUNKS, MAX_STORAGE_BYTES},
        };
        let b = verified.publication().binding();
        let context = Context {
            vault: peer.vault(),
            genesis: verified.genesis().commitment(),
            device: peer.device(),
            credential_version: 1,
            head: verified.admission().commitment(),
        };
        let read = async |component, index| -> Result<Vec<u8>> {
            let Reply::Published(bytes) = self
                .exchange(
                    Operation::Published {
                        context: context.clone(),
                        descriptor: b.descriptor_commitment,
                        component,
                        index,
                    },
                    Some(peer.bearer()),
                )
                .await?
            else {
                anyhow::bail!("error snapshot-response");
            };
            Ok(bytes)
        };
        ensure!(
            read(None, 0).await? == descriptor,
            "error snapshot-descriptor-substitution"
        );
        let mut package = Package {
            descriptor: descriptor.to_vec(),
            catalogs: Default::default(),
            state: vec![],
            manifest: vec![],
            images: vec![],
        };
        let mut total = 0_u64;
        let mut chunks = 0_u64;
        for (i, recipe) in bootstrap_format::download::catalogs(descriptor)?
            .into_iter()
            .enumerate()
        {
            for (index, length) in recipe.lengths.into_iter().enumerate() {
                let bytes = read(Some(recipe.component), u64::try_from(index)?).await?;
                ensure!(
                    u64::try_from(bytes.len())? == length,
                    "error snapshot-length"
                );
                total = total
                    .checked_add(length)
                    .ok_or_else(|| anyhow::anyhow!("error snapshot-limit"))?;
                chunks += 1;
                ensure!(
                    total <= MAX_STORAGE_BYTES && chunks <= MAX_CHUNKS,
                    "error snapshot-limit"
                );
                package.catalogs[i].extend(bytes);
                #[cfg(test)]
                if std::env::var("AVEN_SNAPSHOT_CRASH").as_deref() == Ok("download") {
                    std::process::exit(83);
                }
            }
        }
        for recipe in bootstrap_format::download::artifacts(descriptor, &package.catalogs)? {
            let mut records = Vec::new();
            for (index, length) in recipe.lengths.into_iter().enumerate() {
                let bytes = read(Some(recipe.component), u64::try_from(index)?).await?;
                ensure!(
                    u64::try_from(bytes.len())? == length,
                    "error snapshot-length"
                );
                total = total
                    .checked_add(length)
                    .ok_or_else(|| anyhow::anyhow!("error snapshot-limit"))?;
                chunks += 1;
                ensure!(
                    total <= MAX_STORAGE_BYTES && chunks <= MAX_CHUNKS,
                    "error snapshot-limit"
                );
                records.push(bytes);
            }
            match recipe.component {
                Component::Manifest => package.manifest = records,
                Component::State => package.state = records,
                Component::Image(object_id) => {
                    package.images.push(ImageRecords { object_id, records })
                }
                _ => anyhow::bail!("error snapshot-component"),
            }
        }
        Ok(package)
    }
    /// Returns the secret invitation only after exact registration is resolved.
    pub async fn invite(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<Invitation> {
        let (seed, p, d) = store
            .prepare_invitation(db, &self.locator, Some(expires))
            .await?;
        let context = Context {
            vault: seed.genesis().context().vault_id,
            genesis: seed.genesis().commitment(),
            device: seed.genesis().device_id(),
            credential_version: 1,
            head: p.commitment(),
        };
        ensure!(
            matches!(
                self.exchange(
                    Operation::Register {
                        context,
                        declaration: d.record().to_vec()
                    },
                    Some(seed.bearer())
                )
                .await?,
                Reply::Registered(peer::RegistrationStatus::Open)
            ),
            "error enrollment-invitation-unavailable"
        );
        store.registered_invitation(db, &d).await
    }
    /// Protects an independently generated peer, then posts its one frozen request.
    /// Use None on restart. Passing another invitation never replaces an identity.
    pub async fn request(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Option<Invitation>,
    ) -> Result<()> {
        let peer = store.prepare_peer(db, &self.locator, invitation).await?;
        ensure!(
            matches!(
                self.exchange(
                    Operation::Post {
                        vault: peer.vault(),
                        handle: peer.handle(),
                        request: peer.request().to_vec()
                    },
                    None
                )
                .await?,
                Reply::Done
            ),
            "error enrollment-response"
        );
        Ok(())
    }
    /// Admits only the retained recipient. A lost reply leaves a disclosure fence.
    pub async fn admit(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        // Resumption cannot create or renew an invitation.
        let (seed, _, d) = store.prepare_invitation(db, &self.locator, None).await?;
        let Reply::Mailbox(mail) = self
            .exchange(
                Operation::Mailbox {
                    vault: seed.genesis().context().vault_id,
                    handle: d.handle(),
                },
                None,
            )
            .await?
        else {
            anyhow::bail!("error enrollment-response");
        };
        let Some(request) = mail.request else {
            return Ok(false);
        };
        let (seed, p, a) = store.prepare_admission(db, &self.locator, &request).await?;
        let mut context = Context {
            vault: seed.genesis().context().vault_id,
            genesis: seed.genesis().commitment(),
            device: seed.genesis().device_id(),
            credential_version: 1,
            head: p.commitment(),
        };
        let Reply::Admitted(evidence) = self
            .exchange(
                Operation::Admit {
                    context: context.clone(),
                    record: a.record().to_vec(),
                },
                Some(seed.bearer()),
            )
            .await?
        else {
            anyhow::bail!("error enrollment-response");
        };
        context.head = a.commitment();
        let Reply::Descriptor(descriptor) = self
            .exchange(Operation::Descriptor { context }, Some(seed.bearer()))
            .await?
        else {
            anyhow::bail!("error enrollment-response");
        };
        store.finish_inviter(db, &evidence, &descriptor).await?;
        Ok(true)
    }
    /// Verifies and protects complete admission key coverage, not task installation.
    pub async fn complete(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        let peer = store.prepare_peer(db, &self.locator, None).await?;
        let Reply::Mailbox(mail) = self
            .exchange(
                Operation::Mailbox {
                    vault: peer.vault(),
                    handle: peer.handle(),
                },
                None,
            )
            .await?
        else {
            anyhow::bail!("error enrollment-response");
        };
        let Some(evidence) = mail.evidence else {
            return Ok(false);
        };
        let grant = store.pin_peer_response(db, &evidence).await?;
        let context = Context {
            vault: peer.vault(),
            genesis: grant.genesis,
            device: peer.device(),
            credential_version: 1,
            head: grant.head,
        };
        let Reply::Descriptor(descriptor) = self
            .exchange(Operation::Descriptor { context }, Some(peer.bearer()))
            .await?
        else {
            anyhow::bail!("error enrollment-response");
        };
        store.finish_peer(db, &evidence, &descriptor).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
