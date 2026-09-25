//! Isolated repeatable device enrollment and published snapshot retrieval.
//! The public mailbox never exposes bootstrap chunks, images or credentials.
mod management;
use crate::protected_local_keys::peer::OpenInvitation;
use crate::{
    http_admission::{self, Outcome},
    protected_local_keys::ProtectedLocalKeyStore,
    seed_bootstrap_http,
};
use anyhow::{Context as _, Result, ensure};
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
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
pub use management::RemovalStatus;

pub(crate) struct CreatedInvitation {
    pub(crate) invitation: Invitation,
    pub(crate) state: OpenInvitation,
    pub(crate) resumed: bool,
}
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, atomic::AtomicU64};

const PATH: &str = "/e2ee/enrollment/v1";
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const BUSY_RETRIES: usize = 3;
const CONTROL_LIMIT: usize =
    aven_core::sync::base64_bytes::encoded_len(membership::MAX_RECORD_BYTES) + 4096;
const PUBLISHED_RESPONSE_LIMIT: usize = aven_core::sync::base64_bytes::encoded_len(
    aven_core::sync::bootstrap_staging::MAX_REQUEST_BYTES,
) + 4096;
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
        #[serde(with = "aven_core::sync::base64_bytes")]
        declaration: Vec<u8>,
    },
    Post {
        vault: [u8; 32],
        handle: [u8; 32],
        #[serde(with = "aven_core::sync::base64_bytes")]
        request: Vec<u8>,
    },
    Mailbox {
        vault: [u8; 32],
        handle: [u8; 32],
    },
    Admit {
        context: Context,
        handle: [u8; 32],
        #[serde(with = "aven_core::sync::base64_bytes")]
        record: Vec<u8>,
    },
    Membership {
        context: Context,
    },
    PrepareManagement {
        context: Context,
    },
    Manage {
        context: Context,
        #[serde(with = "aven_core::sync::base64_bytes")]
        record: Vec<u8>,
    },
    Cancel {
        context: Context,
        handle: [u8; 32],
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
    Admitted(#[serde(with = "aven_core::sync::base64_bytes")] Vec<u8>),
    Membership(Evidence),
    PreparedManagement(membership::ManagementPreparation),
    Managed(#[serde(with = "aven_core::sync::base64_bytes")] Vec<u8>),
    Cancelled(membership::CancelStatus),
    Published(#[serde(with = "aven_core::sync::base64_bytes")] Vec<u8>),
}
struct Server {
    db: Database,
    gate: tokio::sync::Semaphore,
    #[cfg(test)]
    enrollment_clock: Option<Arc<AtomicU64>>,
}
/// Merge only into an isolated E2EE router, never the legacy plaintext server.
pub fn router(db: Database) -> Router {
    router_with_clock_inner(db, None)
}
fn router_with_clock_inner(db: Database, clock: Option<Arc<AtomicU64>>) -> Router {
    #[cfg(not(test))]
    let _ = clock;
    Router::new()
        .route(PATH, post(handle))
        .with_state(Arc::new(Server {
            db,
            gate: tokio::sync::Semaphore::new(1),
            #[cfg(test)]
            enrollment_clock: clock,
        }))
}
#[cfg(test)]
pub(crate) fn router_with_clock(db: Database, clock: Arc<AtomicU64>) -> Router {
    router_with_clock_inner(db, Some(clock))
}
async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    #[cfg(test)]
    let time = server
        .enrollment_clock
        .as_ref()
        .map(|clock| clock.load(Ordering::SeqCst))
        .map(|time| i64::try_from(time).expect("test clock fits Unix time"));
    #[cfg(not(test))]
    let time = None;
    let response = match http_admission::dispatch(
        &server.gate,
        REQUEST_TIMEOUT,
        dispatch(&server.db, request, time),
    )
    .await
    {
        Outcome::Dispatched(Ok(reply)) => {
            let limit = match &reply {
                Reply::Membership(_) => membership::MAX_EVIDENCE_JSON_BYTES,
                Reply::PreparedManagement(_) => membership::MAX_EVIDENCE_JSON_BYTES + 128,
                Reply::Published(_) => PUBLISHED_RESPONSE_LIMIT,
                _ => CONTROL_LIMIT,
            };
            http_admission::json(&reply, limit)
                .unwrap_or_else(|| (StatusCode::BAD_REQUEST, "enrollment-refused").into_response())
        }
        Outcome::Dispatched(Err(error)) if is_stale(&error) => {
            (StatusCode::CONFLICT, "membership-stale").into_response()
        }
        Outcome::Dispatched(Err(_)) => {
            (StatusCode::BAD_REQUEST, "enrollment-refused").into_response()
        }
        Outcome::DispatchTimeout => {
            (StatusCode::REQUEST_TIMEOUT, "enrollment-timeout").into_response()
        }
        Outcome::PermitTimeout => {
            let mut response = (StatusCode::SERVICE_UNAVAILABLE, "enrollment-busy").into_response();
            http_admission::mark_busy(&mut response);
            response
        }
    };
    http_admission::no_store(response)
}
async fn dispatch(db: &Database, request: Request, time: Option<i64>) -> Result<Reply> {
    #[cfg(not(test))]
    let _ = time;
    ensure!(
        http_admission::is_json(request.headers()),
        "error enrollment-http"
    );
    let credential = request
        .headers()
        .get(header::AUTHORIZATION)
        .map(|h| {
            http_admission::bearer(h).ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))
        })
        .transpose()?;
    let bytes = http_admission::body(request, CONTROL_LIMIT)
        .await
        .ok_or_else(|| anyhow::anyhow!("error enrollment-limit"))?;
    let op: Operation =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("error enrollment-http"))?;
    Ok(match op {
        Operation::PrepareManagement { context } => Reply::PreparedManagement(
            db.prepare_membership_management(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
                ),
            )
            .await?,
        ),
        Operation::Manage { context, record } => Reply::Managed(
            db.apply_membership_management(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
                ),
                &record,
            )
            .await?,
        ),
        Operation::Cancel { context, handle } => {
            let auth = context.auth(
                credential
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
            );
            #[cfg(test)]
            let status = match time {
                Some(time) => {
                    membership::cancel_membership_invitation_at(db, &auth, handle, time).await?
                }
                None => db.cancel_membership_invitation(&auth, handle).await?,
            };
            #[cfg(not(test))]
            let status = db.cancel_membership_invitation(&auth, handle).await?;
            Reply::Cancelled(status)
        }
        Operation::Post {
            vault,
            handle,
            request,
        } => {
            #[cfg(test)]
            match time {
                Some(time) => {
                    membership::post_membership_request_at(db, vault, handle, &request, time)
                        .await?
                }
                None => db.post_membership_request(vault, handle, &request).await?,
            }
            #[cfg(not(test))]
            db.post_membership_request(vault, handle, &request).await?;
            Reply::Done
        }
        Operation::Mailbox { vault, handle } => {
            Reply::Mailbox(db.membership_mailbox(vault, handle).await?)
        }
        Operation::Register {
            context,
            declaration,
        } => {
            let auth = context.auth(
                credential
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
            );
            #[cfg(test)]
            let status = match time {
                Some(time) => {
                    membership::register_membership_invitation_at(db, &auth, &declaration, time)
                        .await?
                }
                None => {
                    db.register_membership_invitation(&auth, &declaration)
                        .await?
                }
            };
            #[cfg(not(test))]
            let status = db
                .register_membership_invitation(&auth, &declaration)
                .await?;
            Reply::Registered(status)
        }
        Operation::Admit {
            context,
            handle,
            record,
        } => {
            let auth = context.auth(
                credential
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))?,
            );
            #[cfg(test)]
            let admitted = match time {
                Some(time) => {
                    membership::admit_membership_device_at(db, &auth, handle, &record, time).await?
                }
                None => db.admit_membership_device(&auth, handle, &record).await?,
            };
            #[cfg(not(test))]
            let admitted = db.admit_membership_device(&auth, handle, &record).await?;
            Reply::Admitted(admitted)
        }
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
        } else if matches!(&op, Operation::PrepareManagement { .. }) {
            membership::MAX_EVIDENCE_JSON_BYTES + 128
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
        let mut attempt = 0;
        let mut response = loop {
            let mut response = request
                .try_clone()
                .ok_or_else(|| anyhow::anyhow!("error enrollment-http"))?
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("error enrollment-network outcome-unknown"))?;
            if response.status() == StatusCode::OK {
                break response;
            }
            let status = response.status();
            let retry_after = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            let mut response_bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("error enrollment-network outcome-unknown"))?
            {
                ensure!(
                    chunk.len() <= 256 - response_bytes.len(),
                    "error enrollment-refused outcome-unknown"
                );
                response_bytes.extend_from_slice(&chunk);
            }
            let busy = status == StatusCode::SERVICE_UNAVAILABLE
                && response_bytes == b"enrollment-busy"
                && retry_after.is_some();
            if busy && attempt < BUSY_RETRIES {
                tokio::time::sleep(busy_retry_delay(attempt, retry_after.unwrap())).await;
                attempt += 1;
                continue;
            }
            if status == StatusCode::CONFLICT && response_bytes == b"membership-stale" {
                anyhow::bail!(membership::StaleContext);
            }
            if busy {
                anyhow::bail!("error enrollment-busy");
            }
            anyhow::bail!("error enrollment-refused outcome-unknown");
        };
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
        let initial_context = Context {
            vault: peer.vault(),
            genesis: verified.genesis().commitment(),
            device: peer.device(),
            credential_version: 1,
            head: floor.head(),
        };
        let evidence = self.membership(&initial_context, peer.bearer()).await?;
        floor = store
            .adopt_download_refresh(db, identity, peer, verified, &evidence)
            .await?;
        let mut retried = false;
        let mut read = async |component, index| -> Result<Vec<u8>> {
            let mut context = Context {
                vault: peer.vault(),
                genesis: verified.genesis().commitment(),
                device: peer.device(),
                credential_version: 1,
                head: floor.head(),
            };
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
                            .adopt_download_refresh(db, identity, peer, verified, &evidence)
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
        Ok(self
            .invite_with_status(store, db, expires)
            .await?
            .invitation)
    }

    pub(crate) async fn invite_with_status(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<CreatedInvitation> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await?;
        let previous = store.open_invitation(db, &inputs).await?;
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
                    let invitation = store.registered_invitation(db, &inputs, &journal).await?;
                    let state = store
                        .open_invitation(db, &inputs)
                        .await?
                        .context("error enrollment-invitation-missing")?;
                    return Ok(CreatedInvitation {
                        invitation,
                        state,
                        resumed: previous.is_some_and(|old| old.handle == journal.handle),
                    });
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
        let peer = self.prepare(store, db, invitation, false).await?;
        self.post(&peer).await
    }
    /// Requests admission with a replacement invitation from the same inviter
    /// while joining is unfinished. Earlier attempts stay retained and can
    /// still complete the join.
    pub async fn replace(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Invitation,
    ) -> Result<()> {
        let peer = self.prepare(store, db, Some(invitation), true).await?;
        self.post(&peer).await
    }
    /// Selects or creates the attempt to post from protected local state.
    /// Its failures are integrity or eligibility refusals, never outcomes of
    /// an exchange.
    pub(crate) async fn prepare(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Option<Invitation>,
        replace: bool,
    ) -> Result<membership::Joiner> {
        match invitation {
            Some(invitation) if replace => store.replace_peer(db, &self.locator, invitation).await,
            invitation => store.prepare_peer(db, &self.locator, invitation).await,
        }
    }
    /// Whether attempts other than the one `prepare` returned could still
    /// complete this join.
    pub(crate) async fn has_other_attempts(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        Ok(store.peer_attempts(db, &self.locator).await?.len() > 1)
    }
    pub(crate) async fn post(&self, peer: &membership::Joiner) -> Result<()> {
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
    /// Checks every retained attempt for an admission and completes the one
    /// whose grant opens for its exact request; a grant that does not open is
    /// an integrity failure. A refusal or unreachable mailbox for one attempt
    /// proves nothing about the others: any busy mailbox is reported as busy,
    /// any attempt still waiting yields `false`, and only when every mailbox
    /// failed is the latest attempt's error reported. Once a response is
    /// pinned, only that attempt is checked.
    pub async fn complete(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        let attempts = store.peer_attempts(db, &self.locator).await?;
        let (mut busy, mut waiting, mut failed) = (None, false, None);
        for peer in attempts.iter().rev() {
            match self
                .exchange(
                    Operation::Mailbox {
                        vault: peer.vault(),
                        handle: peer.handle(),
                    },
                    None,
                )
                .await
            {
                Ok(Reply::Mailbox(mail)) if mail.admission.is_some() => {
                    let grant = store.pin_peer_response(db, &mail).await?;
                    return self.finish(store, db, peer, grant).await;
                }
                Ok(Reply::Mailbox(_)) => waiting = true,
                Ok(_) => {
                    failed.get_or_insert(anyhow::anyhow!("error enrollment-response"));
                }
                Err(error) if error.to_string() == "error enrollment-busy" => busy = Some(error),
                Err(error) => {
                    failed.get_or_insert(error);
                }
            }
        }
        match (busy, failed) {
            (Some(error), _) => Err(error),
            _ if waiting => Ok(false),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(false),
        }
    }
    async fn finish(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        peer: &membership::Joiner,
        grant: membership::ProvisionalGrant,
    ) -> Result<bool> {
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
fn busy_retry_delay(attempt: usize, retry_after: u64) -> std::time::Duration {
    let base_ms = 50_u64 << attempt.min(5);
    let mut random = [0_u8; 2];
    let _ = getrandom::fill(&mut random);
    let jitter_ms = u16::from_le_bytes(random) as u64 % (base_ms / 2 + 1);
    std::time::Duration::from_millis(base_ms + jitter_ms)
        .max(std::time::Duration::from_secs(retry_after.min(2)))
}

fn is_stale(error: &anyhow::Error) -> bool {
    error.is::<membership::StaleContext>()
}
#[cfg(test)]
mod tests;
