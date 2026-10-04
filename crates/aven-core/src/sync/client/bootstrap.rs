//! Seed bootstrap: claiming a server for a new sync and publishing the
//! seed's frozen snapshot.
//!
//! HTTP framing: POST /e2ee/bootstrap/v1. Control operations are
//! application/json, one externally tagged operation in a context envelope.
//! Package records travel only in binary batches (see
//! [`staging::batch`]), each carrying its context in the batch header; the
//! server answers a stored batch with `Stored`. Base64 strings and batch
//! records carry exact existing codec bytes, never a second encrypted package
//! representation. Setup and device credentials use Authorization: Bearer <64
//! lowercase hex digits>; only ClaimSetup uses setup authority. IDs and
//! payloads never enter URLs. JSON requests are bounded at the base64 length
//! of one chunk plus 4096 bytes of framing, batches at
//! [`staging::batch::MAX_BYTES`]. Status responses are bounded at 1 MiB. Busy
//! callers retry a bounded number of times.
use anyhow::{Result, ensure};
use aven_protocol::refusal::Bootstrap as Refusal;

use super::exchange::{self, Link};
use super::keys::ProtectedLocalKeyStore;
use crate::db::Database;
use crate::sync::shared_state::validated::{ProofCache, ValidatedSeed};
use crate::sync::{
    bootstrap_staging as staging,
    seed_claim::{ClaimAuthentication, ClaimResult, Genesis, PublicationOutcome, Secret},
};

/// Logs one setup stage's duration at debug level when dropped.
pub(crate) struct StageTimer {
    stage: &'static str,
    started: std::time::Instant,
}

impl StageTimer {
    pub(crate) fn start(stage: &'static str) -> Self {
        Self {
            stage,
            started: std::time::Instant::now(),
        }
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        tracing::debug!(
            stage = self.stage,
            elapsed_ms = self.started.elapsed().as_secs_f64() * 1000.0,
            "setup stage finished"
        );
    }
}

pub use aven_protocol::wire::bootstrap::{
    Envelope, Operation, PATH, REQUEST_LIMIT, RESPONSE_LIMIT, Reply,
};

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
        self.post(secret, exchange::json_content(), bytes, claim)
            .await
    }

    /// Stores one batch of package records. Every record is sent exactly as
    /// frozen; the server checks each against its descriptor slot.
    pub async fn put_batch(
        &self,
        secret: &Secret,
        header: &staging::batch::Header,
        records: &[&[u8]],
    ) -> Result<Reply> {
        let bytes = staging::batch::encode(header, records)?;
        let content_type = exchange::HttpHeader {
            name: "content-type".into(),
            value: staging::batch::CONTENT_TYPE.into(),
        };
        self.post(secret, content_type, bytes, false).await
    }

    async fn post(
        &self,
        secret: &Secret,
        content_type: exchange::HttpHeader,
        bytes: Vec<u8>,
        claim: bool,
    ) -> Result<Reply> {
        let bytes = exchange::post(
            &self.link,
            &self.endpoint,
            Some(secret),
            content_type,
            bytes,
            RESPONSE_LIMIT,
        )
        .await
        .map_err(|failure| match failure {
            exchange::Failure::Hosting(hosting) => super::errors::hosting_error(hosting),
            exchange::Failure::Network => {
                anyhow::anyhow!("error bootstrap-network outcome-unknown")
            }
            exchange::Failure::SecureTransport => {
                anyhow::anyhow!("error bootstrap-tls outcome-unknown")
            }
            exchange::Failure::Malformed => anyhow::anyhow!("error bootstrap-response"),
            exchange::Failure::TooLarge => anyhow::anyhow!("error bootstrap-response-limit"),
            exchange::Failure::RequestBodyLimit => {
                anyhow::anyhow!("error sync-request-body-limit")
            }
            exchange::Failure::Refused { code, .. } => {
                match Refusal::classify(code.as_deref(), claim) {
                    Refusal::Claimed => {
                        anyhow::anyhow!("error bootstrap-storage-already-claimed")
                    }
                    Refusal::SetupRejected => {
                        anyhow::anyhow!("error bootstrap-setup-invitation-rejected")
                    }
                    Refusal::SetupExpired => {
                        anyhow::anyhow!("error bootstrap-setup-invitation-expired")
                    }
                    Refusal::Quota => {
                        anyhow::anyhow!("error attachment-quota-exceeded")
                    }
                    Refusal::Unknown => anyhow::anyhow!("error bootstrap-refused outcome-unknown"),
                }
            }
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
        self.resume_reporting(store, database, &|_, _| {}).await
    }

    /// [`Self::resume`], reporting acknowledged payload bytes for an
    /// unpublished upload attempt against the package's exact total. Reporting
    /// starts at zero; server presence never suppresses a frozen slot.
    pub async fn resume_reporting(
        &self,
        store: &ProtectedLocalKeyStore,
        database: &Database,
        uploaded: &(dyn Fn(u64, u64) + Sync),
    ) -> Result<bool> {
        self.resume_validated(store, database, None, uploaded).await
    }

    /// [`Self::resume_reporting`], reusing a proof the caller already made
    /// so the frozen package is not authenticated again.
    pub(crate) async fn resume_validated(
        &self,
        store: &ProtectedLocalKeyStore,
        database: &Database,
        proof: Option<ValidatedSeed>,
        uploaded: &(dyn Fn(u64, u64) + Sync),
    ) -> Result<bool> {
        let mut proofs = ProofCache::new(proof);
        let (seed, intent) = store.seed_resume_intent(database, &mut proofs).await?;
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
                let upload = store
                    .seed_upload(database, &intent, &mut proofs)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("error bootstrap-outcome-missing"))?;
                let _timer = StageTimer::start("upload");
                let budget = upload.budget();
                if let Reply::Staging(ref current) = status {
                    validate_staging(current, upload.slots(), binding, budget)?;
                }
                let staging = match status {
                    Reply::Missing => {
                        self.exchange(
                            seed.genesis(),
                            seed.bearer(),
                            Operation::Declare {
                                descriptor: upload.descriptor().to_vec(),
                                budget,
                            },
                        )
                        .await?
                    }
                    Reply::Staging(_) => {
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
                validate_staging(&staging, upload.slots(), binding, budget)?;
                let header = |records| staging::batch::Header {
                    vault: seed.genesis().context().vault_id,
                    genesis: seed.genesis().commitment(),
                    bootstrap: binding.bootstrap_id,
                    commitment: binding.descriptor_commitment,
                    records,
                };
                uploaded(0, budget.bytes);
                let mut sent = 0_u64;
                // Catalog slices precede every artifact batch, so the server
                // can verify each catalog before it accepts dependent records.
                for batch in pack(upload.slots(), header) {
                    let records = upload.read(database, &batch).await?;
                    let refs: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
                    let reply = self.put_batch(seed.bearer(), &header(batch), &refs).await?;
                    ensure!(
                        matches!(reply, Reply::Stored),
                        "error bootstrap-upload-refused"
                    );
                    sent += records
                        .iter()
                        .map(|record| record.len() as u64)
                        .sum::<u64>();
                    uploaded(sent, budget.bytes);
                }
                tracing::debug!(sent, total = budget.bytes, "uploaded frozen staged bytes");
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
        let _timer = StageTimer::start("adopt");
        store
            .adopt_seed_publication_with(database, &outcome, &mut proofs)
            .await
    }
}

/// Validates a staging reply without treating server presence as an upload
/// filter. Artifact components may be absent until their catalog is complete.
fn validate_staging(
    status: &staging::StagingStatus,
    slots: &[staging::batch::Slot],
    binding: &crate::sync::seed_claim::PublicationBinding,
    budget: staging::Budget,
) -> Result<()> {
    ensure!(
        status.descriptor_commitment == binding.descriptor_commitment
            && status.stream_id == binding.stream_id
            && status.budget == budget,
        "error bootstrap-status-mismatch"
    );

    let mut seen = Vec::new();
    for listed in &status.components {
        let expected = slots
            .iter()
            .filter(|slot| slot.component == listed.component)
            .count();
        ensure!(
            expected > 0 && listed.chunks.len() == expected && !seen.contains(&listed.component),
            "error bootstrap-status-mismatch"
        );
        seen.push(listed.component);
    }

    // Catalog slots and the descriptor's manifest are always listed. State
    // and image slots appear only after their describing catalogs verify.
    for component in [
        staging::Component::DataCatalog,
        staging::Component::PrefixCatalog,
        staging::Component::ImageCatalog,
        staging::Component::Manifest,
    ] {
        ensure!(seen.contains(&component), "error bootstrap-status-mismatch");
    }

    let catalog_complete = |component| {
        status
            .components
            .iter()
            .find(|listed| listed.component == component)
            .is_some_and(|listed| {
                listed
                    .chunks
                    .iter()
                    .all(|presence| *presence == staging::Presence::Verified)
            })
    };
    let data_complete = catalog_complete(staging::Component::DataCatalog);
    let images_complete = catalog_complete(staging::Component::ImageCatalog);
    let listed = |component| seen.contains(&component);
    ensure!(
        listed(staging::Component::State) == data_complete,
        "error bootstrap-status-mismatch"
    );
    let mut image_components = Vec::new();
    for slot in slots {
        if let staging::Component::Image(_) = slot.component
            && !image_components.contains(&slot.component)
        {
            image_components.push(slot.component);
        }
    }
    ensure!(
        image_components
            .iter()
            .all(|component| listed(*component) == images_complete),
        "error bootstrap-status-mismatch"
    );
    Ok(())
}

/// Target payload size for setup uploads. Smaller batches provide more frequent
/// server acknowledgements without splitting frozen encrypted records.
const UPLOAD_BATCH_PAYLOAD: u64 = staging::MAX_REQUEST_BYTES as u64;

/// Splits `slots` into batches within every batch limit, keeping catalog
/// slices apart from the records catalogs describe.
fn pack(
    slots: &[staging::batch::Slot],
    header: impl Fn(Vec<staging::batch::Slot>) -> staging::batch::Header,
) -> Vec<Vec<staging::batch::Slot>> {
    let mut batches: Vec<Vec<staging::batch::Slot>> = Vec::new();
    let mut current: Vec<staging::batch::Slot> = Vec::new();
    let mut payload = 0_u64;
    for slot in slots {
        let fits = !current.is_empty()
            && current.len() < staging::batch::MAX_RECORDS
            && payload + slot.len <= UPLOAD_BATCH_PAYLOAD
            && current[0].component.is_catalog() == slot.component.is_catalog()
            && staging::batch::header_len(&header(
                current.iter().copied().chain([*slot]).collect(),
            ))
            .is_some();
        if !fits && !current.is_empty() {
            batches.push(std::mem::take(&mut current));
            payload = 0;
        }
        current.push(*slot);
        payload += slot.len;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// A package's chunks by component, in upload order.
#[cfg(any(test, feature = "test-support"))]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn header(records: Vec<staging::batch::Slot>) -> staging::batch::Header {
        staging::batch::Header {
            vault: [1; 32],
            genesis: [2; 32],
            bootstrap: [3; 32],
            commitment: [4; 32],
            records,
        }
    }

    #[test]
    fn upload_batches_acknowledge_each_full_size_record() {
        let slots: Vec<_> = (0..4)
            .map(|index| staging::batch::Slot {
                component: staging::Component::State,
                index,
                len: staging::MAX_REQUEST_BYTES as u64,
            })
            .collect();
        let batches = pack(&slots, header);
        assert_eq!(batches.len(), slots.len());
        assert!(batches.iter().all(|batch| batch.len() == 1));
        assert_eq!(batches.concat(), slots);
    }

    #[test]
    fn upload_batches_group_small_records_without_mixing_catalogs() {
        let slots: Vec<_> = (0..10)
            .map(|index| staging::batch::Slot {
                component: if index < 2 {
                    staging::Component::DataCatalog
                } else {
                    staging::Component::State
                },
                index,
                len: UPLOAD_BATCH_PAYLOAD / 4,
            })
            .collect();
        let batches = pack(&slots, header);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [2, 4, 4]);
        assert_eq!(batches.concat(), slots);
        for batch in batches {
            let records: Vec<_> = batch
                .iter()
                .map(|slot| vec![0; slot.len as usize])
                .collect();
            let refs: Vec<_> = records.iter().map(Vec::as_slice).collect();
            assert!(staging::batch::encode(&header(batch), &refs).is_ok());
        }
    }
}
