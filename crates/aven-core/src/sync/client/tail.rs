//! Encrypted ordinary-task transport: record rounds and image transfers.
use anyhow::{Result, ensure};
use aven_protocol::refusal::Tail as Refusal;
use serde::Serialize;

use super::errors::is_stale;
use super::exchange::{self, Link};
use super::keys::ProtectedLocalKeyStore;
use super::keys::peer::{PublishingBlocked, TailSnapshot};
use crate::db::Database;
use crate::sync::{
    encrypted_tail::{self as tail, Accepted, Context, Operation, Reply},
    seed_claim::Secret,
};
mod images;
pub use aven_protocol::wire::tail::Envelope;
pub use aven_protocol::wire::tail::{BATCH_PATH, PATH};
pub use images::{DrainSnapshot, IMAGES_PATH, ImageTransfer, Round};
pub struct Client {
    link: Link,
    origin: url::Url,
    locator: String,
}

pub enum PushStep {
    Appended,
    BatchAppended(usize),
    Image(Option<ImageTransfer>),
    Empty,
}

impl Client {
    pub fn new(origin: &str, link: Link) -> Result<Self> {
        Ok(Self {
            link,
            origin: super::origin::endpoint(origin, PATH)?,
            locator: origin.into(),
        })
    }
    fn enrollment(&self) -> Result<super::enrollment::Client> {
        super::enrollment::Client::new(&self.locator, self.link.clone())
    }
    pub async fn exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        operation: Operation,
    ) -> Result<Reply> {
        let response_limit = match &operation {
            Operation::Append { .. } | Operation::Features => tail::CONTROL_LIMIT,
            Operation::Lookup { .. } => tail::APPEND_LIMIT,
            Operation::Pull { .. } => tail::RESPONSE_LIMIT,
        };
        let request_limit = if matches!(&operation, Operation::Append { .. }) {
            tail::APPEND_LIMIT
        } else {
            tail::CONTROL_LIMIT
        };
        self.exchange_to(
            PATH,
            context,
            bearer,
            operation,
            request_limit,
            response_limit,
        )
        .await
    }
    async fn exchange_to<O: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        context: &Context,
        bearer: &Secret,
        operation: O,
        request_limit: usize,
        response_limit: usize,
    ) -> Result<R> {
        let mut endpoint = self.origin.clone();
        endpoint.set_path(path);
        let mut correlation = [0; 32];
        getrandom::fill(&mut correlation)
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-entropy"))?;
        let compact = path == BATCH_PATH;
        let bytes = if compact {
            serde_json::to_vec(&tail::batch::Envelope {
                context: context.clone(),
                correlation,
                operation,
            })?
        } else {
            serde_json::to_vec(&Envelope {
                context: context.clone(),
                correlation,
                operation,
            })?
        };
        ensure!(bytes.len() <= request_limit, "error encrypted-tail-limit");
        let bytes = exchange::post_json(&self.link, &endpoint, Some(bearer), bytes, response_limit)
            .await
            .map_err(|failure| match failure {
                exchange::Failure::Network => {
                    anyhow::anyhow!("error encrypted-tail-network outcome-unknown")
                }
                exchange::Failure::SecureTransport => {
                    anyhow::anyhow!("error encrypted-tail-tls outcome-unknown")
                }
                exchange::Failure::Malformed => anyhow::anyhow!("error encrypted-tail-http"),
                exchange::Failure::TooLarge => anyhow::anyhow!("error encrypted-tail-limit"),
                exchange::Failure::RequestBodyLimit => {
                    anyhow::anyhow!("error sync-request-body-limit")
                }
                exchange::Failure::Refused { code, .. } => match Refusal::classify(code.as_deref())
                {
                    Refusal::Malformed => {
                        anyhow::anyhow!("error encrypted-tail-malformed")
                    }
                    Refusal::BatchKnown => {
                        anyhow::anyhow!("error encrypted-tail-batch-known")
                    }
                    Refusal::Stale => crate::sync::seed_claim::membership::StaleContext.into(),
                    // The server authenticated the credential but refused
                    // this device, as enrollment does for a removed device.
                    Refusal::Unauthorized => {
                        crate::sync::seed_claim::membership::Unauthorized.into()
                    }
                    Refusal::PrefixIdentityCollision => tail::PrefixIdentityCollision.into(),
                    Refusal::Quota => {
                        anyhow::anyhow!("error attachment-quota-exceeded")
                    }
                    Refusal::Unknown => {
                        anyhow::anyhow!("error encrypted-tail-refused outcome-unknown")
                    }
                },
            })?;
        let response: Envelope<R> = if compact {
            let reply: tail::batch::Envelope<R> = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error encrypted-tail-http"))?;
            Envelope {
                context: reply.context,
                correlation: reply.correlation,
                operation: reply.operation,
            }
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error encrypted-tail-http"))?
        };
        ensure!(
            response.context == *context && response.correlation == correlation,
            "error encrypted-tail-context"
        );
        Ok(response.operation)
    }
    pub async fn batch_exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        operation: tail::BatchOperation,
    ) -> Result<tail::BatchReply> {
        let limit = if matches!(operation, tail::BatchOperation::Append { .. }) {
            tail::BATCH_APPEND_LIMIT
        } else {
            tail::BATCH_CONTROL_LIMIT
        };
        self.exchange_to(
            BATCH_PATH,
            context,
            bearer,
            operation,
            limit,
            tail::BATCH_CONTROL_LIMIT,
        )
        .await
    }

    /// Every unresolved operation is looked up before any upload or resend.
    async fn reconcile_frozen(
        &self,
        a: &tail::Authority,
        bearer: &Secret,
        db: &Database,
        blob_dir: &std::path::Path,
        batch_count: usize,
    ) -> Result<bool> {
        let frozen = db.encrypted_tail_frozen_records(a).await?;
        let mut responses = Vec::with_capacity(frozen.len());
        if frozen.len() > 1 && batch_count > 1 {
            let tail::BatchReply::Resolved(resolutions) = self
                .batch_exchange(
                    &a.context,
                    bearer,
                    tail::BatchOperation::Resolve {
                        operation_ids: frozen.iter().map(|(id, _)| id.clone()).collect(),
                    },
                )
                .await?
            else {
                anyhow::bail!("error encrypted-tail-reply")
            };
            ensure!(
                resolutions.len() == frozen.len(),
                "error encrypted-tail-mapping"
            );
            for ((id, record), resolution) in frozen.iter().zip(resolutions) {
                responses.push(match resolution {
                    tail::Resolution::Found(mapping) => {
                        ensure!(mapping.operation_id == *id, "error encrypted-tail-mapping");
                        Reply::Found(Accepted {
                            mapping: mapping.into(),
                            record: record.clone(),
                        })
                    }
                    tail::Resolution::Absent { operation_id } => {
                        ensure!(operation_id == *id, "error encrypted-tail-mapping");
                        Reply::Absent
                    }
                    tail::Resolution::Bootstrap { .. } => {
                        anyhow::bail!("error encrypted-tail-accepted-identity")
                    }
                });
            }
        } else {
            for (id, _) in &frozen {
                responses.push(
                    self.exchange(
                        &a.context,
                        bearer,
                        Operation::Lookup {
                            operation_id: id.clone(),
                            expected: None,
                        },
                    )
                    .await?,
                );
            }
        }
        crate::sync::crash::Crash::Tail.at("after-resolve");
        let mut absences = Vec::new();
        for ((id, record), response) in frozen.iter().zip(responses) {
            match response {
                Reply::Found(mut accepted) => {
                    ensure!(
                        accepted.mapping.operation_id == *id,
                        "error encrypted-tail-accepted-identity"
                    );
                    db.observe_encrypted_tail(a, &accepted.mapping).await?;
                    crate::sync::crash::Crash::Tail.at("batch-observed");
                    use sha2::{Digest, Sha256};
                    if Sha256::digest(&accepted.record).as_slice() != accepted.mapping.commitment {
                        let Reply::Found(found) = self
                            .exchange(
                                &a.context,
                                bearer,
                                Operation::Lookup {
                                    operation_id: id.clone(),
                                    expected: Some(accepted.mapping.clone()),
                                },
                            )
                            .await?
                        else {
                            anyhow::bail!("error encrypted-tail-accepted-unavailable")
                        };
                        ensure!(
                            found.mapping == accepted.mapping,
                            "error encrypted-tail-mapping"
                        );
                        accepted = found;
                    }
                    db.verify_encrypted_tail_outcome(a, &accepted).await?;
                }
                Reply::Absent => absences.push(a.confirm_absent(record, &Reply::Absent)?),
                _ => anyhow::bail!("error encrypted-tail-accepted-identity"),
            }
        }
        if !absences.is_empty()
            && !db
                .reconcile_encrypted_tail_absences(a, &absences, blob_dir)
                .await?
        {
            return Ok(false);
        }
        Ok(!a.rotation_pending())
    }
    #[cfg(any(test, feature = "test-support"))]
    pub async fn push(
        &self,
        inputs: &TailSnapshot,
        db: &Database,
        blob_dir: &std::path::Path,
    ) -> Result<PushStep> {
        Ok(self
            .push_in_run(inputs, db, blob_dir, None, &mut 1, 1)
            .await?
            .0)
    }
    /// Dispatches the next ordered head. An unavailable local image source or
    /// failed image transfer leaves that head pending and stops this round's push
    /// phase instead of appending its Ref. Fails with `PublishingBlocked`
    /// before any upload or append while a withdrawal rotation is required.
    async fn push_in_run(
        &self,
        inputs: &TailSnapshot,
        db: &Database,
        blob_dir: &std::path::Path,
        preflight: Option<tail::Preflight>,
        batch_count: &mut usize,
        remaining: usize,
    ) -> Result<(PushStep, Option<tail::Preflight>)> {
        inputs.require_publishing_ready()?;
        let (a, bearer) = (&inputs.authority, &inputs.bearer);
        if *batch_count == 0 && db.encrypted_tail_has_batch_work(a).await? {
            *batch_count = match self.exchange(&a.context, bearer, Operation::Features).await {
                Ok(Reply::Features(features))
                    if features.count >= tail::BATCH_COUNT
                        && features.bytes >= tail::BATCH_BYTES =>
                {
                    tail::BATCH_COUNT
                }
                Ok(Reply::Features(_)) => 1,
                Err(error) if super::errors::has_code(&error, "encrypted-tail-malformed") => 1,
                Err(error) => return Err(error),
                _ => anyhow::bail!("error encrypted-tail-reply"),
            };
        }
        if !self
            .reconcile_frozen(a, bearer, db, blob_dir, *batch_count)
            .await?
        {
            return Ok((PushStep::Empty, preflight));
        }
        // A missing local source leaves its head pending without blocking pulls.
        let (prepared, preflight) = match db
            .prepare_encrypted_batch_in_run(
                a,
                blob_dir,
                preflight.clone(),
                (*batch_count).max(1).min(remaining),
            )
            .await
        {
            Err(error) if error.is::<tail::attachments::ImageSourceUnavailable>() => {
                return Ok((PushStep::Image(Some(ImageTransfer::Failed)), preflight));
            }
            prepared => prepared?,
        };
        let Some(tail::Push { record, upload }) = prepared else {
            return Ok((PushStep::Empty, preflight));
        };
        let frozen = db.encrypted_tail_frozen_records(a).await?;
        if frozen.len() > 1 && *batch_count > 1 {
            inputs.require_publishing_ready()?;
            let tail::BatchReply::Appended(mappings) = self
                .batch_exchange(
                    &a.context,
                    bearer,
                    tail::BatchOperation::Append {
                        records: frozen
                            .iter()
                            .map(|(_, r)| tail::BatchRecord(r.clone()))
                            .collect(),
                    },
                )
                .await?
            else {
                anyhow::bail!("error encrypted-tail-reply")
            };
            crate::sync::crash::Crash::Tail.at("after-append");
            crate::sync::crash::Crash::Tail.at("after-batch-append");
            ensure!(
                mappings.len() == frozen.len(),
                "error encrypted-tail-mapping"
            );
            use sha2::{Digest, Sha256};
            for ((id, record), mapping) in frozen.iter().zip(&mappings) {
                ensure!(
                    mapping.operation_id == *id
                        && Sha256::digest(record).as_slice() == mapping.commitment,
                    "error encrypted-tail-mapping"
                );
            }
            ensure!(
                mappings
                    .windows(2)
                    .all(|pair| pair[0].sequence.checked_add(1) == Some(pair[1].sequence)),
                "error encrypted-tail-mapping"
            );
            let mappings: Vec<tail::Mapping> = mappings.into_iter().map(Into::into).collect();
            db.accept_encrypted_tail_batch(a, &mappings).await?;
            return Ok((PushStep::BatchAppended(mappings.len()), preflight));
        }
        let is_image = upload.is_some();
        let ticket = match upload {
            Some(upload) => match self.upload_prepared_image(inputs, upload).await {
                Ok(ticket) => Some(ticket),
                Err(error)
                    if is_stale(&error)
                        || error.is::<PublishingBlocked>()
                        || super::errors::has_code(&error, "attachment-quota-exceeded")
                        // A proxy body limit refuses every retry identically.
                        || super::errors::has_code(&error, "sync-request-body-limit") =>
                {
                    return Err(error);
                }
                Err(_) => {
                    return Ok((PushStep::Image(Some(ImageTransfer::Failed)), preflight));
                }
            },
            None => None,
        };
        inputs.require_publishing_ready()?;
        let Reply::Appended(mapping) = self
            .exchange(&a.context, bearer, Operation::Append { ticket, record })
            .await?
        else {
            anyhow::bail!("error encrypted-tail-reply")
        };
        crate::sync::crash::Crash::Tail.at("after-append");
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
                    bearer,
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
        Ok((
            if is_image {
                PushStep::Image(None)
            } else {
                PushStep::Appended
            },
            preflight,
        ))
    }
    /// Reads one authorized page without preparing uploads. True refers only to
    /// this remote watermark, never to unresolved local work or overall readiness.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn pull_only_round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        let enrollment = self.enrollment()?;
        enrollment.refresh(store, db).await?;
        retry_stale!(
            self.pull_only_round_once(store, db).await,
            enrollment.refresh(store, db).await,
        )
    }
    #[cfg(any(test, feature = "test-support"))]
    async fn pull_only_round_once(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        let inputs = store.tail_inputs(db, &self.locator).await?;
        db.validate_encrypted_attachment_integrity(&inputs.authority)
            .await?;
        self.pull(&inputs.authority, &inputs.bearer, db).await
    }
    async fn pull(&self, a: &tail::Authority, bearer: &Secret, db: &Database) -> Result<bool> {
        let state = db.encrypted_round_state(a).await?;
        let (after, watermark) = (state.cursor, state.initial_watermark);
        let Reply::Page(page) = self
            .exchange(
                &a.context,
                bearer,
                Operation::Pull {
                    after,
                    limit: tail::PAGE_COUNT,
                    watermark,
                },
            )
            .await?
        else {
            anyhow::bail!("error encrypted-tail-reply")
        };
        ensure!(
            page.after == after && watermark.is_none_or(|w| page.watermark == w),
            "error encrypted-tail-cursor"
        );
        db.apply_encrypted_tail_page(a, &page).await?;
        Ok(!page.has_more)
    }
}
