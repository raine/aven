//! Isolated first-peer enrollment and published snapshot retrieval, not shipping sync.
//! The public mailbox never exposes bootstrap chunks, images or credentials.
use crate::{protected_local_keys::ProtectedLocalKeyStore, seed_bootstrap_http};
use anyhow::{Result, ensure};
use aven_core::{
    db::Database,
    sync::seed_claim::{
        Secret,
        membership::{self, Evidence, Invitation, Mailbox},
        peer,
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
const CONTROL_LIMIT: usize = 4 * peer::CONTROL_LIMIT + 4096;
const PUBLISHED_RESPONSE_LIMIT: usize = 4 * (1_048_576 + 222) + 4096;
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
    fn active(inputs: &crate::protected_local_keys::peer::ActiveInputs) -> Self {
        Self {
            vault: inputs.membership.genesis().context().vault_id,
            genesis: inputs.membership.genesis().commitment(),
            device: inputs.device(),
            credential_version: 1,
            head: inputs.membership.head(),
        }
    }
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
        handle: [u8; 32],
        record: Vec<u8>,
    },
    Membership {
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
    Admitted(Vec<u8>),
    Membership(Evidence),
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
    let mut response = if let Ok(_permit) = server.gate.try_acquire() {
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            dispatch(&server.db, request),
        )
        .await
        {
            Ok(Ok(reply)) => match serde_json::to_vec(&reply) {
                Ok(bytes)
                    if bytes.len()
                        <= match &reply {
                            Reply::Membership(_) => membership::MAX_EVIDENCE_JSON_BYTES,
                            Reply::Published(_) => PUBLISHED_RESPONSE_LIMIT,
                            _ => CONTROL_LIMIT,
                        } =>
                {
                    ([(header::CONTENT_TYPE, "application/json")], bytes).into_response()
                }
                _ => (StatusCode::BAD_REQUEST, "enrollment-refused").into_response(),
            },
            Ok(Err(error)) if is_stale(&error) => {
                (StatusCode::CONFLICT, "membership-stale").into_response()
            }
            _ => (StatusCode::BAD_REQUEST, "enrollment-refused").into_response(),
        }
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "enrollment-busy").into_response()
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
    let bytes = to_bytes(request.into_body(), CONTROL_LIMIT)
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
            db.post_membership_request(vault, handle, &request).await?;
            Reply::Done
        }
        Operation::Mailbox { vault, handle } => {
            Reply::Mailbox(db.membership_mailbox(vault, handle).await?)
        }
        Operation::Register {
            context,
            declaration,
        } => Reply::Registered(
            db.register_membership_invitation(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
                ),
                &declaration,
            )
            .await?,
        ),
        Operation::Admit {
            context,
            handle,
            record,
        } => Reply::Admitted(
            db.admit_membership_device(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
                ),
                handle,
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
        Operation::Membership { context } => Reply::Membership(
            db.membership_evidence(
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
        let response_limit = if matches!(&op, Operation::Published { .. }) {
            PUBLISHED_RESPONSE_LIMIT
        } else if matches!(&op, Operation::Membership { .. }) {
            membership::MAX_EVIDENCE_JSON_BYTES
        } else {
            CONTROL_LIMIT
        };
        let bytes =
            serde_json::to_vec(&op).map_err(|_| anyhow::anyhow!("error enrollment-http"))?;
        ensure!(bytes.len() <= CONTROL_LIMIT, "error enrollment-limit");
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
        if response.status() != StatusCode::OK {
            let status = response.status();
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("error enrollment-network outcome-unknown"))?
            {
                ensure!(
                    chunk.len() <= 256 - bytes.len(),
                    "error enrollment-refused outcome-unknown"
                );
                bytes.extend_from_slice(&chunk);
            }
            if status == StatusCode::CONFLICT && bytes == b"membership-stale" {
                anyhow::bail!(membership::StaleContext);
            }
            if status == StatusCode::SERVICE_UNAVAILABLE && bytes == b"enrollment-busy" {
                anyhow::bail!("error enrollment-busy");
            }
            anyhow::bail!("error enrollment-refused outcome-unknown");
        }
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
            response
                .content_length()
                .is_none_or(|n| n <= response_limit as u64),
            "error enrollment-limit"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("error enrollment-network outcome-unknown"))?
        {
            ensure!(
                chunk.len() <= response_limit - bytes.len(),
                "error enrollment-limit"
            );
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
    ) -> Result<aven_core::sync::SharedStateInstallReport> {
        store.install_peer_snapshot(db, self, &self.locator).await
    }

    pub(crate) async fn download(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        identity: [u8; 32],
        peer: &membership::Joiner,
        verified: &membership::VerifiedEnrollment,
        descriptor: &[u8],
    ) -> Result<aven_core::sync::bootstrap_format::download::Metadata> {
        use aven_core::sync::{
            bootstrap_format::{self, download::Metadata},
            bootstrap_staging::{Component, MAX_CHUNKS, MAX_STORAGE_BYTES},
        };
        let b = verified.publication().binding();
        let mut floor = verified.membership().clone();
        let mut retried = false;
        let mut read = async |component, index| -> Result<Vec<u8>> {
            // Every new component read authenticates its own current context.
            let mut context = Context {
                vault: peer.vault(),
                genesis: verified.genesis().commitment(),
                device: peer.device(),
                credential_version: 1,
                head: floor.head(),
            };
            let evidence = self.membership(&context, peer.bearer()).await?;
            floor = store
                .adopt_download_refresh(db, identity, peer, verified.key(), &evidence)
                .await?;
            context.head = floor.head();
            loop {
                match self
                    .exchange(
                        Operation::Published {
                            context: context.clone(),
                            descriptor: b.descriptor_commitment,
                            component,
                            index,
                        },
                        Some(peer.bearer()),
                    )
                    .await
                {
                    Ok(Reply::Published(bytes)) => return Ok(bytes),
                    Err(error) if is_stale(&error) && !retried => {
                        retried = true;
                        let evidence = self.membership(&context, peer.bearer()).await?;
                        floor = store
                            .adopt_download_refresh(db, identity, peer, verified.key(), &evidence)
                            .await?;
                        context.head = floor.head();
                    }
                    Err(error) => return Err(error),
                    _ => anyhow::bail!("error snapshot-response"),
                }
            }
        };
        ensure!(
            read(None, 0).await? == descriptor,
            "error snapshot-descriptor-substitution"
        );
        let mut package = Metadata {
            descriptor: descriptor.to_vec(),
            catalogs: Default::default(),
            state: vec![],
            manifest: vec![],
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
            if matches!(recipe.component, Component::Image(_)) {
                continue;
            }
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
                _ => anyhow::bail!("error snapshot-component"),
            }
        }
        Ok(package)
    }
    async fn membership(&self, context: &Context, bearer: &Secret) -> Result<Evidence> {
        let Reply::Membership(evidence) = self
            .exchange(
                Operation::Membership {
                    context: context.clone(),
                },
                Some(bearer),
            )
            .await?
        else {
            anyhow::bail!("error membership-response");
        };
        evidence.verify()?;
        Ok(evidence)
    }
    pub(crate) async fn refresh_inputs(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        inputs: &mut crate::protected_local_keys::peer::ActiveInputs,
    ) -> Result<()> {
        let evidence = self
            .membership(&Context::active(inputs), inputs.bearer())
            .await?;
        store.adopt_refresh(db, inputs, evidence).await
    }
    pub(crate) async fn refresh(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await
    }
    pub async fn invite(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<Invitation> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await?;
        let journal = store
            .prepare_invitation(db, &inputs, Some(expires), None)
            .await?;
        for attempt in 0..2 {
            match self
                .exchange(
                    Operation::Register {
                        context: Context::active(&inputs),
                        declaration: journal.declaration.clone(),
                    },
                    Some(inputs.bearer()),
                )
                .await
            {
                Ok(Reply::Registered(peer::RegistrationStatus::Open)) => {
                    return store.registered_invitation(db, &inputs, &journal).await;
                }
                Err(error) if is_stale(&error) && attempt == 0 => {
                    self.refresh_inputs(store, db, &mut inputs).await?
                }
                Err(error) => return Err(error),
                _ => anyhow::bail!("error enrollment-invitation-unavailable"),
            }
        }
        unreachable!()
    }
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
    pub async fn admit(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        self.admit_handle(store, db, None).await
    }
    /// Exact historical outcomes remain addressable after subsequent invitations.
    pub async fn admit_handle(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        handle: Option<[u8; 32]>,
    ) -> Result<bool> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await?;
        let journal = store.prepare_invitation(db, &inputs, None, handle).await?;
        let Reply::Mailbox(mail) = self
            .exchange(
                Operation::Mailbox {
                    vault: inputs.membership.genesis().context().vault_id,
                    handle: journal.handle,
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
        for attempt in 0..2 {
            let record = store
                .prepare_admission(db, &inputs, &journal, &request)
                .await?;
            match self
                .exchange(
                    Operation::Admit {
                        context: Context::active(&inputs),
                        handle: journal.handle,
                        record: record.clone(),
                    },
                    Some(inputs.bearer()),
                )
                .await
            {
                Ok(Reply::Admitted(accepted)) => {
                    ensure!(accepted == record, "error enrollment-outcome-mismatch");
                    self.refresh_inputs(store, db, &mut inputs).await?;
                    store.finish_inviter(db, &inputs, &journal, &record).await?;
                    return Ok(true);
                }
                Err(error) if is_stale(&error) && attempt == 0 => {
                    self.refresh_inputs(store, db, &mut inputs).await?
                }
                Err(error) => return Err(error),
                _ => anyhow::bail!("error enrollment-response"),
            }
        }
        unreachable!()
    }
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
        if mail.admission.is_none() {
            return Ok(false);
        }
        let grant = store.pin_peer_response(db, &mail).await?;
        let context = Context {
            vault: peer.vault(),
            genesis: grant.genesis,
            device: peer.device(),
            credential_version: 1,
            head: grant.outcome,
        };
        let evidence = self.membership(&context, peer.bearer()).await?;
        store.finish_peer(db, &evidence).await?;
        Ok(true)
    }
}
fn is_stale(error: &anyhow::Error) -> bool {
    error.is::<membership::StaleContext>()
}
#[cfg(test)]
mod tests;
