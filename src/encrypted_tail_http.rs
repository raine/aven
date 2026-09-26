//! Encrypted ordinary-task sync transport.
use crate::http_admission;
#[cfg(test)]
use crate::protected_local_keys::peer::TailSnapshot;
#[cfg(test)]
use crate::{protected_local_keys::ProtectedLocalKeyStore, seed_bootstrap_http};
use anyhow::Result;
#[cfg(test)]
pub(crate) use aven_core::sync::client::tail::DrainSnapshot;
#[cfg(test)]
use aven_core::sync::client::tail::PushStep;
pub(crate) use aven_core::sync::client::tail::{Envelope, PATH};
pub use aven_core::sync::client::tail::{ImageTransfer, Round};
#[cfg(test)]
use aven_core::sync::{encrypted_tail::Context, seed_claim::Secret};
use aven_core::{
    db::Database,
    sync::encrypted_tail::{self as tail, Operation, Reply},
};
use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use std::sync::Arc;
mod images;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
struct Server {
    db: Database,
    gate: http_admission::Admission,
    image_policy: aven_core::attachments::LifecyclePolicy,
}
pub fn router(db: Database, image_policy: aven_core::attachments::LifecyclePolicy) -> Router {
    Router::new()
        .route(PATH, post(handle))
        .route(images::PATH, post(images::handle))
        .with_state(Arc::new(Server {
            db,
            gate: http_admission::Admission::new(2),
            image_policy,
        }))
}
const CODES: http_admission::Codes = http_admission::codes!("encrypted-tail");
async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let server = &*server;
    let outcome = http_admission::dispatch(
        &server.gate,
        REQUEST_TIMEOUT,
        request,
        tail::APPEND_LIMIT,
        |headers, bytes| async move {
            match dispatch(&server.db, headers, bytes).await {
                Ok(reply) => http_admission::reply(&CODES, &reply, tail::RESPONSE_LIMIT),
                Err(error)
                    if error
                        .downcast_ref::<tail::PrefixIdentityCollision>()
                        .is_some() =>
                {
                    http_admission::refusal(
                        StatusCode::CONFLICT,
                        "encrypted-tail-prefix-identity-collision",
                    )
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
) -> Result<Envelope<Reply>> {
    let bytes = CODES.json_body(&headers, bytes)?;
    let secret = CODES.bearer(&headers)?;
    let input: Envelope<Operation> = CODES.parse(&bytes)?;
    if !matches!(input.operation, Operation::Append { .. }) && bytes.len() > tail::CONTROL_LIMIT {
        return Err(CODES.too_large().into());
    }
    let operation = db
        .encrypted_tail_exchange(&input.context, &secret, input.operation)
        .await?;
    Ok(Envelope {
        context: input.context,
        correlation: input.correlation,
        operation,
    })
}
#[cfg(test)]
mod client;
#[cfg(test)]
pub(crate) use client::Client;

#[cfg(test)]
mod tests;
