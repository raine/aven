//! Seed bootstrap: claiming a server for a new sync and publishing the
//! seed's frozen snapshot.
//!
//! HTTP framing: POST /e2ee/bootstrap/v1, application/json, one
//! externally tagged operation in a context envelope. Base64 strings carry exact
//! existing codec bytes, never a second encrypted package representation. Setup
//! and device credentials use Authorization: Bearer <64 lowercase hex digits>;
//! only ClaimSetup uses setup authority. IDs and payloads never enter URLs.
//! Requests are bounded at the base64 length of one chunk plus 4096 bytes of
//! framing. Status responses are bounded at 1 MiB. Busy callers retry a
//! bounded number of times.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::exchange::{self, Link};
use super::keys::ProtectedLocalKeyStore;
use crate::db::Database;
use crate::sync::{
    base64_bytes, bootstrap_staging as staging,
    seed_claim::{ClaimAuthentication, ClaimResult, Genesis, PublicationOutcome, Secret},
};

pub const PATH: &str = "/e2ee/bootstrap/v1";
pub const REQUEST_LIMIT: usize = base64_bytes::encoded_len(staging::MAX_REQUEST_BYTES) + 4096;
pub const RESPONSE_LIMIT: usize = 1_048_576;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub operation: Operation,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    ClaimSetup {
        #[serde(with = "crate::sync::base64_bytes")]
        bytes: Vec<u8>,
    },
    ClaimBearer {
        #[serde(with = "crate::sync::base64_bytes")]
        bytes: Vec<u8>,
    },
    Declare {
        #[serde(with = "crate::sync::base64_bytes")]
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
        component: staging::Component,
        index: u64,
        #[serde(with = "crate::sync::base64_bytes")]
        bytes: Vec<u8>,
    },
    Publish {
        bootstrap: [u8; 32],
        commitment: [u8; 32],
        #[serde(with = "crate::sync::base64_bytes")]
        record: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Reply {
    Claimed {
        vault: [u8; 32],
        claim: [u8; 32],
        genesis: [u8; 32],
    },
    Missing,
    Canceled,
    Staging(staging::StagingStatus),
    Stored,
    Published(#[serde(with = "crate::sync::base64_bytes")] Vec<u8>),
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

/// Bounded seed bootstrap exchanges with one server.
pub struct Client {
    link: Link,
    endpoint: url::Url,
}

impl Client {
    pub fn new(origin: &str, link: Link) -> Result<Self> {
        Ok(Self {
            link,
            endpoint: super::origin::endpoint(origin, PATH)?,
        })
    }

    pub async fn exchange(
        &self,
        genesis: &Genesis,
        secret: &Secret,
        operation: Operation,
    ) -> Result<Reply> {
        let claim = matches!(
            &operation,
            Operation::ClaimSetup { .. } | Operation::ClaimBearer { .. }
        );
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
        let bytes = exchange::post_json(
            &self.link,
            &self.endpoint,
            Some(secret),
            bytes,
            RESPONSE_LIMIT,
        )
        .await
        .map_err(|failure| match failure {
            exchange::Failure::Network => {
                anyhow::anyhow!("error bootstrap-network outcome-unknown")
            }
            exchange::Failure::Malformed => anyhow::anyhow!("error bootstrap-response"),
            exchange::Failure::TooLarge => anyhow::anyhow!("error bootstrap-response-limit"),
            exchange::Failure::Refused { code, .. } => match code.as_deref() {
                Some("bootstrap-storage-already-claimed") => {
                    anyhow::anyhow!("error bootstrap-storage-already-claimed")
                }
                Some("bootstrap-setup-invitation-rejected") if claim => {
                    anyhow::anyhow!("error bootstrap-setup-invitation-rejected")
                }
                Some("bootstrap-setup-invitation-expired") if claim => {
                    anyhow::anyhow!("error bootstrap-setup-invitation-expired")
                }
                _ => anyhow::anyhow!("error bootstrap-refused outcome-unknown"),
            },
        })?;
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
                // reason to regenerate ciphertext or skip server-side checks.
                for (component, chunks) in components {
                    for (index, bytes) in chunks.into_iter().enumerate() {
                        let reply = self
                            .exchange(
                                seed.genesis(),
                                seed.bearer(),
                                Operation::Put {
                                    bootstrap: binding.bootstrap_id,
                                    commitment: binding.descriptor_commitment,
                                    component,
                                    index: index as u64,
                                    bytes: bytes.to_vec(),
                                },
                            )
                            .await?;
                        ensure!(
                            matches!(reply, Reply::Stored),
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

/// A package's chunks by component, in upload order.
pub fn components(
    package: &crate::sync::bootstrap_format::Package,
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
