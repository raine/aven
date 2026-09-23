//! Isolated encrypted ordinary-task transport, not shipping sync configuration.
use crate::{protected_local_keys::ProtectedLocalKeyStore, seed_bootstrap_http};
use anyhow::{Result, ensure};
use aven_core::{
    db::Database,
    sync::{
        encrypted_tail::{self as tail, Accepted, Context, Operation, Reply},
        seed_claim::Secret,
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
const PATH: &str = "/e2ee/tail/v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    context: Context,
    correlation: [u8; 32],
    operation: T,
}
struct Server {
    db: Database,
    gate: tokio::sync::Semaphore,
}
pub fn router(db: Database) -> Router {
    Router::new()
        .route(PATH, post(handle))
        .with_state(Arc::new(Server {
            db,
            gate: tokio::sync::Semaphore::new(2),
        }))
}
async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let Ok(_permit) = server.gate.try_acquire() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match dispatch(&server.db, request).await {
        Ok(reply) => match serde_json::to_vec(&reply) {
            Ok(bytes) if bytes.len() <= tail::RESPONSE_LIMIT => {
                ([(header::CONTENT_TYPE, "application/json")], bytes).into_response()
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Err(e) => {
            let category = if e.to_string() == "error encrypted-tail-prefix-identity-collision" {
                "prefix_identity_collision"
            } else {
                "encrypted_tail_refused"
            };
            (StatusCode::CONFLICT, category).into_response()
        }
    }
}
async fn dispatch(db: &Database, request: Request) -> Result<Envelope<Reply>> {
    ensure!(
        request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            == Some("application/json")
            && !request.headers().contains_key(header::CONTENT_ENCODING),
        "error encrypted-tail-http"
    );
    let auth = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| anyhow::anyhow!("error encrypted-tail-credential"))?;
    ensure!(
        auth.len() == 64
            && auth
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "error encrypted-tail-credential"
    );
    let secret = Secret::new(
        hex::decode(auth)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-credential"))?,
    );
    let bytes = to_bytes(request.into_body(), tail::APPEND_LIMIT)
        .await
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-limit"))?;
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
pub struct Client {
    transport: seed_bootstrap_http::Client,
    locator: String,
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
    async fn exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        operation: Operation,
    ) -> Result<Reply> {
        let response_limit = match &operation {
            Operation::Append { .. } => tail::CONTROL_LIMIT,
            Operation::Lookup { .. } => tail::APPEND_LIMIT,
            Operation::Pull { .. } => tail::RESPONSE_LIMIT,
        };
        let request_limit = if matches!(&operation, Operation::Append { .. }) {
            tail::APPEND_LIMIT
        } else {
            tail::CONTROL_LIMIT
        };
        let mut correlation = [0; 32];
        getrandom::fill(&mut correlation)
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-entropy"))?;
        let bytes = serde_json::to_vec(&Envelope {
            context: context.clone(),
            correlation,
            operation,
        })?;
        ensure!(bytes.len() <= request_limit, "error encrypted-tail-limit");
        let mut authorization = reqwest::header::HeaderValue::from_str(&format!(
            "Bearer {}",
            hex::encode(bearer.expose())
        ))?;
        authorization.set_sensitive(true);
        let mut response = self
            .transport
            .http
            .post(self.transport.endpoint.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, authorization)
            .body(bytes)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-network outcome-unknown"))?;
        if response.status() != StatusCode::OK {
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("error encrypted-tail-network outcome-unknown"))?
            {
                ensure!(
                    bytes.len() + chunk.len() <= tail::CONTROL_LIMIT,
                    "error encrypted-tail-refused"
                );
                bytes.extend(chunk);
            }
            if bytes == b"prefix_identity_collision" {
                anyhow::bail!("error encrypted-tail-prefix-identity-collision")
            }
            anyhow::bail!("error encrypted-tail-refused outcome-unknown")
        }
        ensure!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                == Some("application/json")
                && !response.headers().contains_key(header::CONTENT_ENCODING)
                && response
                    .content_length()
                    .is_none_or(|n| n <= response_limit as u64),
            "error encrypted-tail-http"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-network outcome-unknown"))?
        {
            ensure!(
                chunk.len() <= response_limit - bytes.len(),
                "error encrypted-tail-limit"
            );
            bytes.extend(chunk);
        }
        let response: Envelope<Reply> = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-http"))?;
        ensure!(
            response.context == *context && response.correlation == correlation,
            "error encrypted-tail-context"
        );
        Ok(response.operation)
    }
    /// Bounded internal round. False means more work, never cursor-only success.
    pub async fn round(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        let inputs = store.tail_inputs(db, &self.locator).await?;
        let a = &inputs.authority;
        if let Some(record) = db.prepare_encrypted_tail(a).await? {
            let Reply::Appended(mapping) = self
                .exchange(&a.context, &inputs.bearer, Operation::Append { record })
                .await?
            else {
                anyhow::bail!("error encrypted-tail-reply")
            };
            #[cfg(test)]
            if std::env::var("AVEN_TAIL_CRASH").as_deref() == Ok("after-append") {
                std::process::exit(84);
            }
            let frozen = db.observe_encrypted_tail(a, &mapping).await?;
            use sha2::{Digest, Sha256};
            let accepted = if Sha256::digest(&frozen).as_slice() == mapping.commitment {
                Accepted {
                    mapping,
                    record: frozen,
                }
            } else {
                let Reply::Found(record) = self
                    .exchange(
                        &a.context,
                        &inputs.bearer,
                        Operation::Lookup {
                            operation_id: mapping.operation_id.clone(),
                            expected: Some(mapping.clone()),
                        },
                    )
                    .await?
                else {
                    anyhow::bail!("error encrypted-tail-accepted-unavailable")
                };
                ensure!(record.mapping == mapping, "error encrypted-tail-mapping");
                record
            };
            db.verify_encrypted_tail_outcome(a, &accepted).await?;
        }
        let caught_up = self.pull(a, &inputs.bearer, db).await?;
        Ok(caught_up && db.prepare_encrypted_tail(a).await?.is_none())
    }
    /// Reads one authorized page without preparing uploads. True refers only to
    /// this remote watermark, never to unresolved local work or overall readiness.
    pub async fn pull_only_round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        let inputs = store.tail_inputs(db, &self.locator).await?;
        self.pull(&inputs.authority, &inputs.bearer, db).await
    }
    async fn pull(&self, a: &tail::Authority, bearer: &Secret, db: &Database) -> Result<bool> {
        let after = db.encrypted_tail_cursor(a).await?;
        let Reply::Page(page) = self
            .exchange(
                &a.context,
                bearer,
                Operation::Pull {
                    after,
                    limit: tail::PAGE_COUNT,
                    watermark: None,
                },
            )
            .await?
        else {
            anyhow::bail!("error encrypted-tail-reply")
        };
        ensure!(page.after == after, "error encrypted-tail-cursor");
        db.apply_encrypted_tail_page(a, &page).await?;
        Ok(!page.has_more)
    }
}
#[cfg(test)]
mod tests;
