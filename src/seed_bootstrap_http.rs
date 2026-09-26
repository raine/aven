//! Server side of seed bootstrap: claiming a server and publishing the seed's
//! frozen snapshot. The wire framing is documented in
//! `aven_core::sync::client::bootstrap`. One active request per router bounds
//! concurrent core materialization.

use crate::{http_admission, protected_local_keys::ProtectedLocalKeyStore, sync_http::HttpDriver};
use anyhow::{Result, ensure};
pub(crate) use aven_core::sync::client::bootstrap::{
    Envelope, Operation, PATH, REQUEST_LIMIT, RESPONSE_LIMIT, Reply,
};
use aven_core::{
    db::Database,
    sync::{
        bootstrap_staging as staging,
        client::bootstrap,
        seed_claim::{ClaimAuthentication, ClaimRefusal, Genesis, Secret},
    },
};
use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use std::{sync::Arc, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(30);
const CODES: http_admission::Codes = http_admission::codes!("bootstrap");

struct Server {
    database: Database,
    policy: staging::PublicationPolicy,
    admission: http_admission::Admission,
}

/// A router serving only the bootstrap route.
/// Claims use the storage's unexpired issued setup verifier.
/// Bind loopback for local construction, or terminate TLS before remote access.
pub fn router(database: Database, policy: staging::PublicationPolicy) -> Router {
    Router::new()
        .route(PATH, post(handle))
        .fallback(|| async { http_admission::refusal(StatusCode::NOT_FOUND, "not-found") })
        .method_not_allowed_fallback(|| async {
            http_admission::refusal(StatusCode::METHOD_NOT_ALLOWED, "method-not-allowed")
        })
        .with_state(Arc::new(Server {
            database,
            policy,
            admission: http_admission::Admission::new(1),
        }))
}

async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let server = &*server;
    let outcome = http_admission::dispatch(
        &server.admission,
        TIMEOUT,
        request,
        REQUEST_LIMIT,
        |headers, bytes| handle_bounded(server, headers, bytes),
    )
    .await;
    http_admission::respond(&CODES, outcome)
}

fn framing(
    headers: &HeaderMap,
    bytes: Option<Bytes>,
) -> Result<(Secret, Envelope), http_admission::Refusal> {
    let bytes = CODES.json_body(headers, bytes)?;
    let secret = CODES.bearer(headers)?;
    Ok((secret, CODES.parse(&bytes)?))
}

async fn handle_bounded(server: &Server, headers: HeaderMap, bytes: Option<Bytes>) -> Response {
    let (secret, envelope) = match framing(&headers, bytes) {
        Ok(framed) => framed,
        Err(refusal) => return http_admission::operation_refusal(&CODES, &refusal.into()),
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
            return match error.downcast_ref::<ClaimRefusal>() {
                Some(ClaimRefusal::Unauthorized { claimed: false }) => http_admission::refusal(
                    StatusCode::FORBIDDEN,
                    "bootstrap-setup-invitation-rejected",
                ),
                Some(ClaimRefusal::Expired) => http_admission::refusal(
                    StatusCode::FORBIDDEN,
                    "bootstrap-setup-invitation-expired",
                ),
                Some(_) => claimed(),
                None => http_admission::operation_refusal(&CODES, &error),
            };
        }
        Err(error) if error.downcast_ref::<staging::Unauthorized>().is_some() => {
            return match server.database.e2ee_server_is_claimed().await {
                Ok(true) => claimed(),
                Ok(false) => http_admission::refusal(
                    StatusCode::FORBIDDEN,
                    "bootstrap-setup-invitation-rejected",
                ),
                Err(error) => http_admission::operation_refusal(&CODES, &error),
            };
        }
        Err(error) => return http_admission::operation_refusal(&CODES, &error),
    };
    http_admission::reply(&CODES, &reply, RESPONSE_LIMIT)
}

fn claimed() -> Response {
    http_admission::refusal(StatusCode::CONFLICT, "bootstrap-storage-already-claimed")
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
            let result = db.admit_seed_claim(bytes, authentication).await?;
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
