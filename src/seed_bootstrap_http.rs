//! Isolated seed bootstrap transport, not ordinary sync or a setup command.
//!
//! Provisional HTTP framing: POST /e2ee/bootstrap/v1, application/json, one
//! externally tagged operation in a context envelope. Base64 strings carry exact
//! existing codec bytes, never a second encrypted package representation. Setup
//! and device credentials use Authorization: Bearer <64 lowercase hex digits>;
//! only ClaimSetup uses setup authority. IDs and payloads never enter URLs.
//! Requests are bounded at the base64 length of one chunk plus 4096 bytes of
//! framing. Status responses are bounded at 1 MiB. One active request per router
//! bounds concurrent core materialization; busy callers retry a bounded number
//! of times. No request tracing, credential redirects or cancellation.

use crate::{
    http_admission::{self, Outcome},
    protected_local_keys::ProtectedLocalKeyStore,
    sync_http::HttpDriver,
};
use anyhow::{Result, ensure};
pub(crate) use aven_core::sync::client::bootstrap::{
    Envelope, Operation, PATH, REQUEST_LIMIT, RESPONSE_LIMIT, Reply,
};
use aven_core::{
    db::Database,
    sync::{
        bootstrap_staging as staging,
        client::bootstrap,
        seed_claim::{ClaimAuthentication, ClaimRefusal, Genesis, Secret, SetupAuthority},
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
use std::{sync::Arc, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(30);

struct Server {
    database: Database,
    setup: Option<SetupAuthority>,
    policy: staging::PublicationPolicy,
    admission: http_admission::Admission,
}

/// A dedicated router with no plaintext routes or legacy-token authentication.
/// Its database must be isolated from a plaintext server and other routers.
/// Without an explicit `setup`, claims use the storage's unexpired issued verifier.
/// Bind loopback for local construction, or terminate TLS before remote access.
pub fn router(
    database: Database,
    setup: Option<SetupAuthority>,
    policy: staging::PublicationPolicy,
) -> Router {
    Router::new()
        .route(PATH, post(handle))
        .fallback(|| async { refusal(StatusCode::NOT_FOUND) })
        .method_not_allowed_fallback(|| async { refusal(StatusCode::METHOD_NOT_ALLOWED) })
        .with_state(Arc::new(Server {
            database,
            setup,
            policy,
            admission: http_admission::Admission::new(1),
        }))
}

fn refusal(status: StatusCode) -> Response {
    refusal_with(status, "bootstrap-refused")
}

fn refusal_with(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        format!("{{\"error\":\"{code}\"}}"),
    )
        .into_response()
}

async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    match http_admission::dispatch(
        &server.admission,
        TIMEOUT,
        request,
        REQUEST_LIMIT,
        |headers, bytes| handle_bounded(&server, headers, bytes),
    )
    .await
    {
        Outcome::Dispatched(response) => response,
        Outcome::DispatchTimeout => refusal(StatusCode::REQUEST_TIMEOUT),
        Outcome::PermitTimeout => {
            let mut response = refusal(StatusCode::SERVICE_UNAVAILABLE);
            http_admission::mark_busy(&mut response);
            response
        }
    }
}

async fn handle_bounded(server: &Server, headers: HeaderMap, bytes: Option<Bytes>) -> Response {
    if !http_admission::is_json(&headers) {
        return refusal(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let Some(secret) = headers
        .get(header::AUTHORIZATION)
        .and_then(http_admission::bearer)
    else {
        return refusal(StatusCode::UNAUTHORIZED);
    };
    let Some(bytes) = bytes else {
        return refusal(StatusCode::PAYLOAD_TOO_LARGE);
    };
    let Ok(envelope) = serde_json::from_slice::<Envelope>(&bytes) else {
        return refusal(StatusCode::BAD_REQUEST);
    };
    let claim = matches!(
        &envelope.operation,
        Operation::ClaimSetup { .. } | Operation::ClaimBearer { .. }
    );
    let reply = match dispatch(server, &secret, envelope).await {
        Ok(reply) => reply,
        // Only a refusal decided inside the claim transaction is definite; any
        // other claim error, such as a storage timeout, leaves the outcome
        // unknown so the claimant keeps its authority and retries.
        Err(error) if claim => {
            let code = match error.downcast_ref::<ClaimRefusal>() {
                Some(ClaimRefusal::Unauthorized { claimed: false }) => {
                    "bootstrap-setup-invitation-rejected"
                }
                Some(ClaimRefusal::Expired) => "bootstrap-setup-invitation-expired",
                Some(_) => "bootstrap-storage-already-claimed",
                None => return refusal(StatusCode::CONFLICT),
            };
            return refusal_with(StatusCode::CONFLICT, code);
        }
        Err(error) if error.to_string() == "error bootstrap-unauthorized" => {
            let code = match server.database.e2ee_server_is_claimed().await {
                Ok(true) => "bootstrap-storage-already-claimed",
                Ok(false) => "bootstrap-setup-invitation-rejected",
                Err(_) => return refusal(StatusCode::INTERNAL_SERVER_ERROR),
            };
            return refusal_with(StatusCode::CONFLICT, code);
        }
        Err(_) => return refusal(StatusCode::CONFLICT),
    };
    http_admission::json(&reply, RESPONSE_LIMIT)
        .map(http_admission::no_store)
        .unwrap_or_else(|| refusal(StatusCode::INTERNAL_SERVER_ERROR))
}

async fn dispatch(server: &Server, secret: &Secret, e: Envelope) -> Result<Reply> {
    let db = &server.database;
    let auth = staging::Authentication {
        vault_id: e.vault,
        genesis_commitment: e.genesis,
        bearer: secret,
    };
    Ok(match e.operation {
        Operation::ClaimSetup { ref bytes } | Operation::ClaimBearer { ref bytes } => {
            // Context mismatch must fail before the claim transaction can mutate.
            ensure!(
                bytes.len() == aven_core::sync::seed_claim::CLAIM_BYTES,
                "invalid claim"
            );
            let genesis = Genesis::from_claim(bytes)?;
            ensure!(
                genesis.context().vault_id == e.vault && genesis.commitment() == e.genesis,
                "invalid context"
            );
            let authentication = if matches!(e.operation, Operation::ClaimSetup { .. }) {
                ClaimAuthentication::SetupSecret(secret)
            } else {
                ClaimAuthentication::SeedBearer(secret)
            };
            let result = db
                .admit_seed_claim(bytes, server.setup.as_ref(), authentication)
                .await?;
            Reply::Claimed {
                vault: result.vault_id,
                claim: result.claim_id,
                genesis: result.genesis_commitment,
            }
        }
        Operation::Declare { descriptor, budget } => Reply::Staging(
            db.declare_bootstrap_staging(&auth, &descriptor, budget)
                .await?,
        ),
        Operation::Status { bootstrap } => {
            db.bootstrap_staging_status(&auth, bootstrap).await?.into()
        }
        Operation::Ensure {
            bootstrap,
            commitment,
        } => Reply::Staging(
            db.ensure_bootstrap_staging(&auth, bootstrap, commitment)
                .await?,
        ),
        Operation::Cancel { bootstrap } => {
            db.cancel_bootstrap_staging(&auth, bootstrap).await?.into()
        }
        Operation::Put {
            bootstrap,
            commitment,
            component,
            index,
            bytes,
        } => {
            db.put_bootstrap_chunk(
                &auth,
                staging::PutChunk {
                    bootstrap_id: bootstrap,
                    descriptor_commitment: commitment,
                    component,
                    index,
                    bytes: &bytes,
                },
            )
            .await?;
            Reply::Stored
        }
        Operation::Publish {
            bootstrap,
            commitment,
            record,
        } => Reply::Published(
            db.publish_bootstrap(
                &auth,
                staging::PublishBootstrap {
                    bootstrap_id: bootstrap,
                    descriptor_commitment: commitment,
                    record: &record,
                },
                server.policy,
            )
            .await?
            .publication()
            .record()
            .to_vec(),
        ),
    })
}

/// Seed bootstrap exchanges with one server over HTTP.
pub struct Client {
    #[cfg(test)]
    pub(crate) http: reqwest::Client,
    pub(crate) endpoint: reqwest::Url,
    pub(crate) driver: HttpDriver,
    origin: String,
}

impl Client {
    pub fn new(origin: &str) -> Result<Self> {
        let driver = HttpDriver::new()?;
        let _ = bootstrap::Client::new(origin, Default::default())?;
        let mut endpoint =
            reqwest::Url::parse(origin).map_err(|_| anyhow::anyhow!("error bootstrap-origin"))?;
        endpoint.set_path(PATH);
        Ok(Self {
            #[cfg(test)]
            http: driver.http.clone(),
            endpoint,
            driver,
            origin: origin.into(),
        })
    }

    #[cfg(test)]
    async fn exchange(
        &self,
        genesis: &Genesis,
        secret: &Secret,
        operation: Operation,
    ) -> Result<Reply> {
        self.driver
            .run(|link| async move {
                bootstrap::Client::new(&self.origin, link)?
                    .exchange(genesis, secret, operation)
                    .await
            })
            .await
    }

    /// Initial setup claim or exact bearer-authorized claim resumption.
    pub async fn claim(
        &self,
        genesis: &Genesis,
        authentication: ClaimAuthentication<'_>,
    ) -> Result<()> {
        self.driver
            .run(|link| async move {
                bootstrap::Client::new(&self.origin, link)?
                    .claim(genesis, authentication)
                    .await
            })
            .await
    }

    /// Resume one frozen candidate, then validate outcome, adopt and clean up.
    pub async fn resume(
        &self,
        store: &ProtectedLocalKeyStore,
        database: &Database,
    ) -> Result<bool> {
        self.driver
            .run(|link| async move {
                bootstrap::Client::new(&self.origin, link)?
                    .resume(store, database)
                    .await
            })
            .await
    }
}

#[cfg(test)]
pub(crate) mod tests;
