//! Isolated seed bootstrap transport, not ordinary sync or a setup command.
//!
//! Provisional HTTP framing: POST /e2ee/bootstrap/v1, application/json, one
//! externally tagged operation in a context envelope. Byte arrays carry exact
//! existing codec bytes, never a second encrypted package representation. Setup
//! and device credentials use Authorization: Bearer <64 lowercase hex digits>;
//! only ClaimSetup uses setup authority. IDs and payloads never enter URLs.
//! JSON expansion is bounded at four bytes per binary byte plus 4096 bytes of
//! framing. Status responses are bounded at 1 MiB. One active request per router
//! bounds concurrent core materialization; busy callers explicitly retry.
//! No request tracing, credential redirects, automatic retries or cancellation.

use crate::protected_local_keys::ProtectedLocalKeyStore;
use anyhow::{Result, ensure};
use aven_core::{
    db::Database,
    sync::{
        bootstrap_staging as staging,
        seed_claim::{
            ClaimAuthentication, ClaimResult, Genesis, PublicationOutcome, Secret, SetupAuthority,
        },
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
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

const PATH: &str = "/e2ee/bootstrap/v1";
const REQUEST_LIMIT: usize = 4 * staging::MAX_REQUEST_BYTES + 4096;
const RESPONSE_LIMIT: usize = 1_048_576;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    vault: [u8; 32],
    genesis: [u8; 32],
    operation: Operation,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Operation {
    ClaimSetup {
        bytes: Vec<u8>,
    },
    ClaimBearer {
        bytes: Vec<u8>,
    },
    Declare {
        descriptor: Vec<u8>,
        budget: staging::Budget,
    },
    Status {
        bootstrap: [u8; 32],
    },
    Ensure {
        bootstrap: [u8; 32],
        commitment: [u8; 32],
    },
    Cancel {
        bootstrap: [u8; 32],
    },
    Put {
        bootstrap: [u8; 32],
        commitment: [u8; 32],
        epoch: u64,
        component: staging::Component,
        index: u64,
        bytes: Vec<u8>,
    },
    Publish {
        bootstrap: [u8; 32],
        commitment: [u8; 32],
        epoch: u64,
        record: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Reply {
    Claimed {
        vault: [u8; 32],
        claim: [u8; 32],
        genesis: [u8; 32],
    },
    Missing,
    Canceled,
    Staging(staging::StagingStatus),
    Put(staging::PutOutcome),
    Published(Vec<u8>),
}

impl From<staging::Status> for Reply {
    fn from(value: staging::Status) -> Self {
        match value {
            staging::Status::Missing => Self::Missing,
            staging::Status::Canceled => Self::Canceled,
            staging::Status::Staging(status) => Self::Staging(status),
            staging::Status::Published(outcome) => {
                Self::Published(outcome.publication().record().to_vec())
            }
        }
    }
}

struct Server {
    database: Database,
    setup: Option<SetupAuthority>,
    policy: staging::PublicationPolicy,
    admission: Semaphore,
}

/// A dedicated router with no plaintext routes or legacy-token authentication.
/// Its database must be isolated from a plaintext server and other routers.
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
            admission: Semaphore::new(1),
        }))
}

fn refusal(status: StatusCode) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        "{\"error\":\"bootstrap-refused\"}",
    )
        .into_response()
}

async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let Ok(_permit) = server.admission.try_acquire() else {
        return refusal(StatusCode::SERVICE_UNAVAILABLE);
    };
    match tokio::time::timeout(TIMEOUT, handle_bounded(&server, request)).await {
        Ok(response) => response,
        Err(_) => refusal(StatusCode::REQUEST_TIMEOUT),
    }
}

async fn handle_bounded(server: &Server, request: Request) -> Response {
    let headers = request.headers();
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some("application/json")
        || headers.contains_key(header::CONTENT_ENCODING)
    {
        return refusal(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let secret = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .and_then(|s| hex::decode(s).ok())
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
        .map(Secret::new);
    let Some(secret) = secret else {
        return refusal(StatusCode::UNAUTHORIZED);
    };
    let Ok(bytes) = to_bytes(request.into_body(), REQUEST_LIMIT).await else {
        return refusal(StatusCode::PAYLOAD_TOO_LARGE);
    };
    let Ok(envelope) = serde_json::from_slice::<Envelope>(&bytes) else {
        return refusal(StatusCode::BAD_REQUEST);
    };
    let reply = match dispatch(server, &secret, envelope).await {
        Ok(reply) => reply,
        Err(_) => return refusal(StatusCode::CONFLICT),
    };
    match serde_json::to_vec(&reply) {
        Ok(bytes) if bytes.len() <= RESPONSE_LIMIT => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        _ => refusal(StatusCode::INTERNAL_SERVER_ERROR),
    }
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
            epoch,
            component,
            index,
            bytes,
        } => Reply::Put(
            db.put_bootstrap_chunk(
                &auth,
                staging::PutChunk {
                    bootstrap_id: bootstrap,
                    descriptor_commitment: commitment,
                    epoch,
                    component,
                    index,
                    bytes: &bytes,
                },
            )
            .await?,
        ),
        Operation::Publish {
            bootstrap,
            commitment,
            epoch,
            record,
        } => Reply::Published(
            db.publish_bootstrap(
                &auth,
                staging::PublishBootstrap {
                    bootstrap_id: bootstrap,
                    descriptor_commitment: commitment,
                    epoch,
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

/// Bounded transport. Diagnostics deliberately discard Reqwest URLs and bodies.
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) endpoint: reqwest::Url,
}

impl Client {
    pub fn new(origin: &str) -> Result<Self> {
        let mut url =
            reqwest::Url::parse(origin).map_err(|_| anyhow::anyhow!("error bootstrap-origin"))?;
        let loopback = url.host_str().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
                || h == "[::1]"
        });
        ensure!(
            (url.scheme() == "https" || (url.scheme() == "http" && loopback))
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "error bootstrap-origin"
        );
        url.set_path(PATH);
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| anyhow::anyhow!("error bootstrap-transport"))?;
        Ok(Self {
            http,
            endpoint: url,
        })
    }

    async fn exchange(
        &self,
        genesis: &Genesis,
        secret: &Secret,
        operation: Operation,
    ) -> Result<Reply> {
        let bytes = serde_json::to_vec(&Envelope {
            vault: genesis.context().vault_id,
            genesis: genesis.commitment(),
            operation,
        })
        .map_err(|_| anyhow::anyhow!("error bootstrap-request"))?;
        ensure!(
            bytes.len() <= REQUEST_LIMIT,
            "error bootstrap-request-limit"
        );
        let mut authorization = reqwest::header::HeaderValue::from_str(&format!(
            "Bearer {}",
            hex::encode(secret.expose())
        ))
        .map_err(|_| anyhow::anyhow!("error bootstrap-credential"))?;
        authorization.set_sensitive(true);
        let mut response = self
            .http
            .post(self.endpoint.clone())
            .header(header::AUTHORIZATION, authorization)
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("error bootstrap-network outcome-unknown"))?;
        ensure!(
            response.status() == StatusCode::OK,
            "error bootstrap-refused outcome-unknown"
        );
        ensure!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                == Some("application/json")
                && !response.headers().contains_key(header::CONTENT_ENCODING),
            "error bootstrap-response"
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= RESPONSE_LIMIT as u64),
            "error bootstrap-response-limit"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("error bootstrap-network outcome-unknown"))?
        {
            ensure!(
                chunk.len() <= RESPONSE_LIMIT - bytes.len(),
                "error bootstrap-response-limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("error bootstrap-response"))
    }

    /// Initial setup claim or exact bearer-authorized claim resumption.
    /// Authority must already be durably protected locally. Publication retires
    /// claim resumption; use the protected-intent resume path after dispatch.
    pub async fn claim(
        &self,
        genesis: &Genesis,
        authentication: ClaimAuthentication<'_>,
    ) -> Result<()> {
        let (secret, operation) = match authentication {
            ClaimAuthentication::SetupSecret(secret) => (
                secret,
                Operation::ClaimSetup {
                    bytes: genesis.claim_bytes(),
                },
            ),
            ClaimAuthentication::SeedBearer(secret) => (
                secret,
                Operation::ClaimBearer {
                    bytes: genesis.claim_bytes(),
                },
            ),
        };
        match self.exchange(genesis, secret, operation).await? {
            Reply::Claimed {
                vault,
                claim,
                genesis: commitment,
            } => ClaimResult {
                vault_id: vault,
                claim_id: claim,
                genesis_commitment: commitment,
            }
            .validate_pinned(genesis),
            _ => anyhow::bail!("error bootstrap-response"),
        }
    }

    /// Resume one frozen candidate, then validate outcome, adopt and clean up.
    /// Call after claim, source preparation, capture and packaging. This never
    /// creates authority, recaptures, cancels, or enables ordinary encrypted sync.
    pub async fn resume(
        &self,
        store: &ProtectedLocalKeyStore,
        database: &Database,
    ) -> Result<bool> {
        let (seed, intent, package) = store.seed_http_inputs(database).await?;
        let publication = intent.publication(seed.genesis())?;
        let binding = publication.binding();
        let status = self
            .exchange(
                seed.genesis(),
                seed.bearer(),
                Operation::Status {
                    bootstrap: binding.bootstrap_id,
                },
            )
            .await?;
        let record = match status {
            Reply::Published(record) => record,
            Reply::Missing | Reply::Staging(_) => {
                let package =
                    package.ok_or_else(|| anyhow::anyhow!("error bootstrap-outcome-missing"))?;
                let components = components(&package);
                let budget = staging::Budget {
                    bytes: components
                        .iter()
                        .flat_map(|(_, chunks)| chunks)
                        .map(|b| b.len() as u64)
                        .sum(),
                    chunks: components
                        .iter()
                        .map(|(_, chunks)| chunks.len() as u64)
                        .sum(),
                };
                let staging = match status {
                    Reply::Missing => {
                        self.exchange(
                            seed.genesis(),
                            seed.bearer(),
                            Operation::Declare {
                                descriptor: package.descriptor.clone(),
                                budget,
                            },
                        )
                        .await?
                    }
                    Reply::Staging(ref s) => {
                        ensure!(
                            s.descriptor_commitment == binding.descriptor_commitment
                                && s.stream_id == binding.stream_id
                                && s.budget == budget,
                            "error bootstrap-status-mismatch"
                        );
                        self.exchange(
                            seed.genesis(),
                            seed.bearer(),
                            Operation::Ensure {
                                bootstrap: binding.bootstrap_id,
                                commitment: binding.descriptor_commitment,
                            },
                        )
                        .await?
                    }
                    _ => unreachable!(),
                };
                let Reply::Staging(staging) = staging else {
                    anyhow::bail!("error bootstrap-response");
                };
                ensure!(
                    staging.descriptor_commitment == binding.descriptor_commitment
                        && staging.stream_id == binding.stream_id
                        && staging.budget == budget,
                    "error bootstrap-status-mismatch"
                );
                // Exact duplicate PUT is intentional: server status is not a
                // reason to regenerate ciphertext or omit catalog validation.
                for (component, chunks) in components {
                    for (index, bytes) in chunks.into_iter().enumerate() {
                        let reply = self
                            .exchange(
                                seed.genesis(),
                                seed.bearer(),
                                Operation::Put {
                                    bootstrap: binding.bootstrap_id,
                                    commitment: binding.descriptor_commitment,
                                    epoch: staging.epoch,
                                    component,
                                    index: index as u64,
                                    bytes: bytes.to_vec(),
                                },
                            )
                            .await?;
                        ensure!(
                            matches!(
                                reply,
                                Reply::Put(
                                    staging::PutOutcome::Quarantined
                                        | staging::PutOutcome::Verified
                                )
                            ),
                            "error bootstrap-upload-refused"
                        );
                    }
                }
                match self
                    .exchange(
                        seed.genesis(),
                        seed.bearer(),
                        Operation::Publish {
                            bootstrap: binding.bootstrap_id,
                            commitment: binding.descriptor_commitment,
                            epoch: staging.epoch,
                            record: publication.record().to_vec(),
                        },
                    )
                    .await?
                {
                    Reply::Published(record) => record,
                    _ => anyhow::bail!("error bootstrap-response"),
                }
            }
            _ => anyhow::bail!("error bootstrap-candidate-unavailable"),
        };
        let outcome =
            PublicationOutcome::from_response(seed.genesis(), intent.descriptor(), &record)?;
        ensure!(
            outcome.publication() == &publication,
            "error bootstrap-outcome-mismatch"
        );
        store.adopt_seed_publication(database, &outcome).await
    }
}

fn components(
    package: &aven_core::sync::bootstrap_format::Package,
) -> Vec<(staging::Component, Vec<&[u8]>)> {
    let mut out = Vec::new();
    for (index, component) in [
        staging::Component::DataCatalog,
        staging::Component::PrefixCatalog,
        staging::Component::ImageCatalog,
    ]
    .into_iter()
    .enumerate()
    {
        out.push((
            component,
            package.catalogs[index].chunks(1_048_576).collect(),
        ));
    }
    out.push((
        staging::Component::Manifest,
        package.manifest.iter().map(Vec::as_slice).collect(),
    ));
    out.push((
        staging::Component::State,
        package.state.iter().map(Vec::as_slice).collect(),
    ));
    for image in &package.images {
        out.push((
            staging::Component::Image(image.object_id),
            image.records.iter().map(Vec::as_slice).collect(),
        ));
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
