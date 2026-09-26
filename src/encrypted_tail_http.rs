//! Isolated encrypted ordinary-task transport, not shipping sync configuration.
#[cfg(test)]
use crate::protected_local_keys::peer::TailSnapshot;
use crate::{
    http_admission::{self, Outcome},
    protected_local_keys::ProtectedLocalKeyStore,
    seed_bootstrap_http,
};
use anyhow::{Result, ensure};
#[cfg(test)]
use aven_core::sync::client::tail::PushStep;
pub(crate) use aven_core::sync::client::tail::{DrainSnapshot, Envelope, PATH};
pub use aven_core::sync::client::tail::{ImageTransfer, Round};
#[cfg(test)]
use aven_core::sync::{encrypted_tail::Context, seed_claim::Secret};
use aven_core::{
    db::Database,
    sync::{
        client::tail as tail_client,
        encrypted_tail::{self as tail, Operation, Reply},
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
use std::path::Path;
use std::sync::Arc;
mod images;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
struct Server {
    db: Database,
    gate: http_admission::Admission,
    image_policy: aven_core::attachments::LifecyclePolicy,
}
pub fn router(db: Database) -> Router {
    router_with_policy(
        db,
        crate::config::AttachmentLifecycleConfig::default().server_policy(),
    )
}
pub fn router_with_policy(
    db: Database,
    image_policy: aven_core::attachments::LifecyclePolicy,
) -> Router {
    Router::new()
        .route(PATH, post(handle))
        .route(images::PATH, post(images::handle))
        .with_state(Arc::new(Server {
            db,
            gate: http_admission::Admission::new(2),
            image_policy,
        }))
}
async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let response = match http_admission::dispatch(
        &server.gate,
        REQUEST_TIMEOUT,
        request,
        tail::APPEND_LIMIT,
        |headers, bytes| dispatch(&server.db, headers, bytes),
    )
    .await
    {
        Outcome::Dispatched(Ok(reply)) => http_admission::json(&reply, tail::RESPONSE_LIMIT)
            .unwrap_or_else(|| {
                (StatusCode::INTERNAL_SERVER_ERROR, "encrypted_tail_refused").into_response()
            }),
        Outcome::Dispatched(Err(e)) => {
            let category = if is_stale(&e) {
                "membership-stale"
            } else if e.to_string() == "error encrypted-tail-prefix-identity-collision" {
                "prefix_identity_collision"
            } else {
                "encrypted_tail_refused"
            };
            (StatusCode::CONFLICT, category).into_response()
        }
        Outcome::DispatchTimeout => {
            (StatusCode::REQUEST_TIMEOUT, "encrypted_tail_timeout").into_response()
        }
        Outcome::PermitTimeout => {
            let mut response =
                (StatusCode::SERVICE_UNAVAILABLE, "encrypted_tail_busy").into_response();
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
) -> Result<Envelope<Reply>> {
    ensure!(
        http_admission::is_json(&headers),
        "error encrypted-tail-http"
    );
    let secret = headers
        .get(header::AUTHORIZATION)
        .and_then(http_admission::bearer)
        .ok_or_else(|| anyhow::anyhow!("error encrypted-tail-credential"))?;
    let bytes = bytes.ok_or_else(|| anyhow::anyhow!("error encrypted-tail-limit"))?;
    let input: Envelope<Operation> =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("error encrypted-tail-http"))?;
    ensure!(
        matches!(input.operation, Operation::Append { .. }) || bytes.len() <= tail::CONTROL_LIMIT,
        "error encrypted-tail-limit"
    );
    let operation = db
        .encrypted_tail_exchange(&input.context, &secret, input.operation)
        .await?;
    Ok(Envelope {
        context: input.context,
        correlation: input.correlation,
        operation,
    })
}
/// Encrypted tail exchanges with one server over HTTP.
pub struct Client {
    transport: seed_bootstrap_http::Client,
    pub(crate) locator: String,
}

/// Runs one core tail operation over this client's transport.
macro_rules! run {
    ($self:ident, |$client:ident| $body:expr) => {
        $self
            .transport
            .driver
            .run(|link| async move {
                let $client = tail_client::Client::new(&$self.locator, link)?;
                $body.await
            })
            .await
    };
}

impl Client {
    pub fn new(origin: &str) -> Result<Self> {
        let mut transport = seed_bootstrap_http::Client::new(origin)?;
        transport.endpoint.set_path(PATH);
        Ok(Self {
            transport,
            locator: origin.into(),
        })
    }
    #[cfg(test)]
    async fn exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        operation: Operation,
    ) -> Result<Reply> {
        run!(self, |client| client.exchange(context, bearer, operation))
    }
    #[cfg(test)]
    async fn image_exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        operation: aven_core::sync::encrypted_tail::attachments::Operation,
    ) -> Result<aven_core::sync::encrypted_tail::attachments::Reply> {
        run!(self, |client| client
            .image_exchange(context, bearer, operation))
    }
    #[cfg(test)]
    async fn push(
        &self,
        inputs: &TailSnapshot,
        db: &Database,
        blob_dir: &Path,
    ) -> Result<PushStep> {
        run!(self, |client| client.push(inputs, db, blob_dir))
    }
    /// Reads one authorized page without preparing uploads.
    pub async fn pull_only_round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        run!(self, |client| client.pull_only_round(store, db))
    }
    pub(crate) async fn start_drain(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<DrainSnapshot> {
        run!(self, |client| client.start_drain(store, db))
    }
    /// Resolves at most one ordered local head, applies one metadata page and
    /// downloads at most one image.
    pub async fn round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
    ) -> Result<Round> {
        run!(self, |client| client.round(store, db, blob_dir))
    }
    pub(crate) async fn round_in_drain(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        drain: &mut DrainSnapshot,
    ) -> Result<Round> {
        run!(self, |client| client
            .round_in_drain(store, db, blob_dir, drain))
    }
    /// Repairs one known reference without changing its descriptor or metadata.
    pub async fn repair_attachment(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        workspace: &str,
        reference: &str,
    ) -> Result<()> {
        run!(self, |client| client
            .repair_attachment(store, db, blob_dir, workspace, reference))
    }
}

#[cfg(test)]
mod tests;

fn is_stale(error: &anyhow::Error) -> bool {
    error.is::<aven_core::sync::seed_claim::membership::StaleContext>()
}
