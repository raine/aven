//! Isolated repeatable device enrollment and published snapshot retrieval.
//! The public mailbox never exposes bootstrap chunks, images or credentials.
use crate::protected_local_keys::peer::ActiveInputs;
use crate::{
    http_admission::{self, Outcome},
    protected_local_keys::ProtectedLocalKeyStore,
    seed_bootstrap_http,
};
use anyhow::{Result, ensure};
#[cfg(test)]
pub(crate) use aven_core::sync::client::enrollment::Context;
pub use aven_core::sync::client::enrollment::RemovalStatus;
pub(crate) use aven_core::sync::client::enrollment::{
    CONTROL_LIMIT, CreatedInvitation, Operation, PATH, PUBLISHED_RESPONSE_LIMIT, Reply,
};
#[cfg(test)]
use aven_core::sync::seed_claim::Secret;
use aven_core::{
    db::Database,
    sync::{
        client::enrollment,
        seed_claim::membership::{self, CancelStatus, Invitation, Joiner},
    },
};
use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, atomic::AtomicU64};

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
struct Server {
    db: Database,
    gate: http_admission::Admission,
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
            gate: http_admission::Admission::new(1),
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
        request,
        CONTROL_LIMIT,
        |headers, bytes| dispatch(&server.db, headers, bytes, time),
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
        Outcome::Dispatched(Err(error)) if is_unauthorized(&error) => {
            (StatusCode::FORBIDDEN, "enrollment-unauthorized").into_response()
        }
        Outcome::Dispatched(Err(error)) if aven_core::db::is_storage_error(&error) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "enrollment-server-error").into_response()
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
async fn dispatch(
    db: &Database,
    headers: HeaderMap,
    bytes: Option<Bytes>,
    time: Option<i64>,
) -> Result<Reply> {
    #[cfg(not(test))]
    let _ = time;
    ensure!(http_admission::is_json(&headers), "error enrollment-http");
    let credential = headers
        .get(header::AUTHORIZATION)
        .map(|h| {
            http_admission::bearer(h).ok_or_else(|| anyhow::anyhow!("error enrollment-credential"))
        })
        .transpose()?;
    let bytes = bytes.ok_or_else(|| anyhow::anyhow!("error enrollment-limit"))?;
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

/// Enrollment exchanges with one server over HTTP. One bounded exchange at
/// a time; callers explicitly retry the same protected intent.
pub struct Client {
    pub(crate) transport: seed_bootstrap_http::Client,
    pub(crate) locator: String,
}

/// Runs one core enrollment operation over this client's transport.
macro_rules! run {
    ($self:ident, |$client:ident| $body:expr) => {
        $self
            .transport
            .driver
            .run(|link| async move {
                let $client = enrollment::Client::new(&$self.locator, link)?;
                $body.await
            })
            .await
    };
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
    #[cfg(test)]
    async fn exchange(&self, op: Operation, secret: Option<&Secret>) -> Result<Reply> {
        run!(self, |client| client.exchange(op, secret))
    }
    /// Installs only a protected, independently enrolled peer into a fresh target.
    /// A completed retry is local and never rewinds later changes.
    pub async fn install(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<aven_core::sync::SharedStateInstallReport> {
        run!(self, |client| client.install(store, db))
    }
    pub(crate) async fn refresh_inputs(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        inputs: &mut ActiveInputs,
    ) -> Result<()> {
        run!(self, |client| client.refresh_inputs(store, db, inputs))
    }
    pub(crate) async fn refresh(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        run!(self, |client| client.refresh(store, db))
    }
    pub async fn invite(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<Invitation> {
        run!(self, |client| client.invite(store, db, expires))
    }
    pub(crate) async fn invite_with_status(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<CreatedInvitation> {
        run!(self, |client| client.invite_with_status(store, db, expires))
    }
    pub async fn request(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Option<Invitation>,
    ) -> Result<()> {
        run!(self, |client| client.request(store, db, invitation))
    }
    /// Requests admission with a replacement invitation from the same inviter
    /// while joining is unfinished.
    pub async fn replace(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Invitation,
    ) -> Result<()> {
        run!(self, |client| client.replace(store, db, invitation))
    }
    pub(crate) async fn prepare(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Option<Invitation>,
        replace: bool,
    ) -> Result<Joiner> {
        run!(self, |client| client
            .prepare(store, db, invitation, replace))
    }
    pub(crate) async fn has_other_attempts(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        run!(self, |client| client.has_other_attempts(store, db))
    }
    pub(crate) async fn post(&self, peer: &Joiner) -> Result<()> {
        run!(self, |client| client.post(peer))
    }
    pub async fn admit(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        run!(self, |client| client.admit(store, db))
    }
    pub(crate) async fn join_requested(&self, vault: [u8; 32], handle: [u8; 32]) -> Result<bool> {
        run!(self, |client| client.join_requested(vault, handle))
    }
    pub async fn admit_handle(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        handle: Option<[u8; 32]>,
    ) -> Result<bool> {
        run!(self, |client| client.admit_handle(store, db, handle))
    }
    pub async fn complete(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        run!(self, |client| client.complete(store, db))
    }
    pub async fn remove_device(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        target: [u8; 32],
    ) -> Result<RemovalStatus> {
        run!(self, |client| client.remove_device(store, db, target))
    }
    pub(crate) async fn finish_pending_management(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        run!(self, |client| client.finish_pending_management(store, db))
    }
    #[cfg(test)]
    async fn manage(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        target: Option<[u8; 32]>,
        withdraw: Option<[u8; 32]>,
    ) -> Result<RemovalStatus> {
        run!(self, |client| client.manage(store, db, target, withdraw))
    }
    #[cfg(test)]
    async fn finish(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        peer: &Joiner,
        mail: &membership::Mailbox,
        grant: membership::ProvisionalGrant,
    ) -> Result<bool> {
        run!(self, |client| client.finish(store, db, peer, mail, grant))
    }
    pub(crate) async fn cancel(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        inputs: &mut ActiveInputs,
        handle: [u8; 32],
    ) -> Result<CancelStatus> {
        run!(self, |client| client.cancel(store, db, inputs, handle))
    }
}
fn is_stale(error: &anyhow::Error) -> bool {
    error.is::<membership::StaleContext>()
}
/// Credential or membership authentication failed for this request.
fn is_unauthorized(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string() == "error enrollment-unauthorized")
}
#[cfg(test)]
mod tests;
