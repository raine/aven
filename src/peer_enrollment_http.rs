//! Repeatable device enrollment and published snapshot retrieval.
//! The public mailbox never exposes bootstrap chunks, images or credentials.
use crate::http_admission;
#[cfg(test)]
use crate::{protected_local_keys::ProtectedLocalKeyStore, seed_bootstrap_http};
use anyhow::Result;
#[cfg(test)]
pub(crate) use aven_core::sync::client::enrollment::Context;
pub use aven_core::sync::client::enrollment::RemovalStatus;
pub(crate) use aven_core::sync::client::enrollment::{
    CONTROL_LIMIT, Operation, PATH, PUBLISHED_RESPONSE_LIMIT, Reply,
};
#[cfg(test)]
use aven_core::sync::seed_claim::Secret;
#[cfg(test)]
use aven_core::sync::{
    client::enrollment,
    seed_claim::membership::{Invitation, Joiner},
};
use aven_core::{db::Database, sync::seed_claim::membership};
use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::HeaderMap,
    response::Response,
    routing::post,
};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const CODES: http_admission::Codes = http_admission::codes!("enrollment");
/// Unix seconds that invitation expiry is judged against.
type Clock = Arc<dyn Fn() -> Result<i64> + Send + Sync>;
struct Server {
    db: Database,
    gate: http_admission::Admission,
    clock: Clock,
}
pub fn router(db: Database) -> Router {
    router_with(db, Arc::new(membership::now))
}
fn router_with(db: Database, clock: Clock) -> Router {
    Router::new()
        .route(PATH, post(handle))
        .with_state(Arc::new(Server {
            db,
            gate: http_admission::Admission::new(1),
            clock,
        }))
}
#[cfg(test)]
pub(crate) fn router_with_clock(db: Database, clock: Arc<AtomicU64>) -> Router {
    router_with(
        db,
        Arc::new(move || Ok(i64::try_from(clock.load(Ordering::SeqCst))?)),
    )
}
async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let server = &*server;
    let outcome = http_admission::dispatch(
        &server.gate,
        REQUEST_TIMEOUT,
        request,
        CONTROL_LIMIT,
        |headers, bytes| async move {
            match dispatch(server, headers, bytes).await {
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
async fn dispatch(server: &Server, headers: HeaderMap, bytes: Option<Bytes>) -> Result<Reply> {
    let db = &server.db;
    let bytes = CODES.json_body(&headers, bytes)?;
    let credential = CODES.optional_bearer(&headers)?;
    let op: Operation = CODES.parse(&bytes)?;
    let bearer = || {
        credential
            .as_ref()
            .ok_or_else(|| CODES.missing_credential())
    };
    Ok(match op {
        Operation::Post {
            vault,
            handle,
            request,
        } => {
            db.post_membership_request_at(vault, handle, &request, (server.clock)()?)
                .await?;
            Reply::Done
        }
        Operation::Mailbox { vault, handle } => {
            Reply::Mailbox(db.membership_mailbox(vault, handle).await?)
        }
        Operation::PrepareManagement { context } => Reply::PreparedManagement(
            db.prepare_membership_management(&context.auth(bearer()?))
                .await?,
        ),
        Operation::Manage { context, record } => Reply::Managed(
            db.apply_membership_management(&context.auth(bearer()?), &record)
                .await?,
        ),
        Operation::Cancel { context, handle } => Reply::Cancelled(
            db.cancel_membership_invitation_at(&context.auth(bearer()?), handle, (server.clock)()?)
                .await?,
        ),
        Operation::Register {
            context,
            declaration,
        } => Reply::Registered(
            db.register_membership_invitation_at(
                &context.auth(bearer()?),
                &declaration,
                (server.clock)()?,
            )
            .await?,
        ),
        Operation::Admit {
            context,
            handle,
            record,
        } => Reply::Admitted(
            db.admit_membership_device_at(
                &context.auth(bearer()?),
                handle,
                &record,
                (server.clock)()?,
            )
            .await?,
        ),
        Operation::Published {
            context,
            descriptor,
            component,
            index,
        } => Reply::Published(
            db.published_snapshot_read(&context.auth(bearer()?), descriptor, component, index)
                .await?,
        ),
        Operation::Membership { context } => {
            Reply::Membership(db.membership_evidence(&context.auth(bearer()?)).await?)
        }
    })
}

#[cfg(test)]
mod client;
#[cfg(test)]
pub(crate) use client::Client;

#[cfg(test)]
mod tests;
