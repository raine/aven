//! Repeatable device enrollment and published snapshot retrieval.
//! The public mailbox never exposes bootstrap chunks, images or credentials.
use crate::{http_admission, protected_local_keys::ProtectedLocalKeyStore, seed_bootstrap_http};
use anyhow::{Result, ensure};
#[cfg(test)]
pub(crate) use aven_core::sync::client::enrollment::Context;
pub use aven_core::sync::client::enrollment::RemovalStatus;
pub(crate) use aven_core::sync::client::enrollment::{
    CONTROL_LIMIT, Operation, PATH, PUBLISHED_RESPONSE_LIMIT, Reply,
};
#[cfg(test)]
use aven_core::sync::seed_claim::Secret;
#[cfg(test)]
use aven_core::sync::seed_claim::membership::Joiner;
use aven_core::{
    db::Database,
    sync::{
        client::enrollment,
        seed_claim::membership::{self, Invitation},
    },
};
use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::HeaderMap,
    response::Response,
    routing::post,
};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, atomic::AtomicU64};

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const CODES: http_admission::Codes = http_admission::codes!("enrollment");
struct Server {
    db: Database,
    gate: http_admission::Admission,
    #[cfg(test)]
    enrollment_clock: Option<Arc<AtomicU64>>,
}
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
    let server = &*server;
    let outcome = http_admission::dispatch(
        &server.gate,
        REQUEST_TIMEOUT,
        request,
        CONTROL_LIMIT,
        |headers, bytes| async move {
            match dispatch(&server.db, headers, bytes, time).await {
                Ok(reply) => {
                    let limit = match &reply {
                        Reply::Membership(_) => membership::MAX_EVIDENCE_JSON_BYTES,
                        Reply::PreparedManagement(_) => membership::MAX_EVIDENCE_JSON_BYTES + 128,
                        Reply::Published(_) => PUBLISHED_RESPONSE_LIMIT,
                        _ => CONTROL_LIMIT,
                    };
                    http_admission::reply(&CODES, &reply, limit)
                }
                Err(error) => http_admission::operation_refusal(&CODES, &error),
            }
        },
    )
    .await;
    http_admission::respond(&CODES, outcome)
}
async fn dispatch(
    db: &Database,
    headers: HeaderMap,
    bytes: Option<Bytes>,
    time: Option<i64>,
) -> Result<Reply> {
    #[cfg(not(test))]
    let _ = time;
    let bytes = CODES.json_body(&headers, bytes)?;
    let credential = CODES.optional_bearer(&headers)?;
    let op: Operation = CODES.parse(&bytes)?;
    Ok(match op {
        Operation::PrepareManagement { context } => Reply::PreparedManagement(
            db.prepare_membership_management(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| CODES.missing_credential())?,
                ),
            )
            .await?,
        ),
        Operation::Manage { context, record } => Reply::Managed(
            db.apply_membership_management(
                &context.auth(
                    credential
                        .as_ref()
                        .ok_or_else(|| CODES.missing_credential())?,
                ),
                &record,
            )
            .await?,
        ),
        Operation::Cancel { context, handle } => {
            let auth = context.auth(
                credential
                    .as_ref()
                    .ok_or_else(|| CODES.missing_credential())?,
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
                    .ok_or_else(|| CODES.missing_credential())?,
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
                    .ok_or_else(|| CODES.missing_credential())?,
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
                        .ok_or_else(|| CODES.missing_credential())?,
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
                        .ok_or_else(|| CODES.missing_credential())?,
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
    pub async fn invite(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<Invitation> {
        run!(self, |client| client.invite(store, db, expires))
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
    pub async fn admit(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        run!(self, |client| client.admit(store, db))
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
    #[cfg(test)]
    pub(crate) async fn finish_pending_management(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        run!(self, |client| client.finish_pending_management(store, db))
    }
    #[cfg(test)]
    pub(crate) async fn refresh(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        run!(self, |client| client.refresh(store, db))
    }
}
#[cfg(test)]
mod tests;
