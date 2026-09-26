//! Encrypted ordinary-task transport: record rounds and image transfers.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::exchange::{self, Link};
use super::keys::ProtectedLocalKeyStore;
use super::keys::peer::{PublishingBlocked, TailSnapshot};
use crate::db::Database;
use crate::sync::{
    encrypted_tail::{self as tail, Accepted, Context, Operation, Reply},
    seed_claim::Secret,
};
mod images;
pub use images::{DrainSnapshot, IMAGES_PATH, ImageTransfer, Round};
pub const PATH: &str = "/e2ee/tail/v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope<T> {
    pub context: Context,
    pub correlation: [u8; 32],
    pub operation: T,
}
pub struct Client {
    link: Link,
    origin: url::Url,
    locator: String,
}

pub enum PushStep {
    Appended,
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
    pub fn locator(&self) -> &str {
        &self.locator
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
            Operation::Append { .. } => tail::CONTROL_LIMIT,
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
        let bytes = serde_json::to_vec(&Envelope {
            context: context.clone(),
            correlation,
            operation,
        })?;
        ensure!(bytes.len() <= request_limit, "error encrypted-tail-limit");
        let bytes = exchange::post_json(&self.link, &endpoint, Some(bearer), bytes, response_limit)
            .await
            .map_err(|failure| match failure {
                exchange::Failure::Network => {
                    anyhow::anyhow!("error encrypted-tail-network outcome-unknown")
                }
                exchange::Failure::Malformed => anyhow::anyhow!("error encrypted-tail-http"),
                exchange::Failure::TooLarge => anyhow::anyhow!("error encrypted-tail-limit"),
                exchange::Failure::Refused { code, .. } => match code.as_deref() {
                    Some("membership-stale") => {
                        crate::sync::seed_claim::membership::StaleContext.into()
                    }
                    Some("encrypted-tail-prefix-identity-collision") => {
                        tail::PrefixIdentityCollision.into()
                    }
                    _ => anyhow::anyhow!("error encrypted-tail-refused outcome-unknown"),
                },
            })?;
        let response: Envelope<R> = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-http"))?;
        ensure!(
            response.context == *context && response.correlation == correlation,
            "error encrypted-tail-context"
        );
        Ok(response.operation)
    }
    /// Every frozen record is resolved by Lookup before any upload or resend.
    async fn reconcile_frozen(
        &self,
        a: &tail::Authority,
        bearer: &Secret,
        db: &Database,
        blob_dir: &std::path::Path,
    ) -> Result<bool> {
        if let Some((id, record)) = db.encrypted_tail_frozen_record(a).await? {
            let response = self
                .exchange(
                    &a.context,
                    bearer,
                    Operation::Lookup {
                        operation_id: id,
                        expected: None,
                    },
                )
                .await?;
            match &response {
                Reply::Found(accepted) => {
                    db.observe_encrypted_tail(a, &accepted.mapping).await?;
                    db.verify_encrypted_tail_outcome(a, accepted).await?;
                }
                Reply::Absent => {
                    let absence = a.confirm_absent(&record, &response)?;
                    return db
                        .reconcile_encrypted_tail_absence(a, &absence, blob_dir)
                        .await;
                }
                _ => anyhow::bail!("error encrypted-tail-accepted-identity"),
            }
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
        Ok(self.push_in_run(inputs, db, blob_dir, None).await?.0)
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
        preflight_local_seq: Option<i64>,
    ) -> Result<(PushStep, Option<i64>)> {
        inputs.require_publishing_ready()?;
        let (a, bearer) = (&inputs.authority, &inputs.bearer);
        if !self.reconcile_frozen(a, bearer, db, blob_dir).await? {
            return Ok((PushStep::Empty, preflight_local_seq));
        }
        // A missing local source leaves its head pending without blocking pulls.
        let (prepared, preflight_local_seq) = match db
            .prepare_encrypted_push_in_run(a, blob_dir, preflight_local_seq)
            .await
        {
            Err(error) if error.is::<tail::attachments::ImageSourceUnavailable>() => {
                return Ok((
                    PushStep::Image(Some(ImageTransfer::Failed)),
                    preflight_local_seq,
                ));
            }
            prepared => prepared?,
        };
        let Some(tail::Push { record, upload }) = prepared else {
            return Ok((PushStep::Empty, preflight_local_seq));
        };
        let is_image = upload.is_some();
        let ticket = match upload {
            Some(upload) => match self.upload_prepared_image(inputs, upload).await {
                Ok(ticket) => Some(ticket),
                Err(error) if is_stale(&error) || error.is::<PublishingBlocked>() => {
                    return Err(error);
                }
                Err(_) => {
                    return Ok((
                        PushStep::Image(Some(ImageTransfer::Failed)),
                        preflight_local_seq,
                    ));
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
        #[cfg(any(test, feature = "test-support"))]
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
            preflight_local_seq,
        ))
    }
    /// Reads one authorized page without preparing uploads. True refers only to
    /// this remote watermark, never to unresolved local work or overall readiness.
    pub async fn pull_only_round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        let enrollment = self.enrollment()?;
        enrollment.refresh(store, db).await?;
        match self.pull_only_round_once(store, db).await {
            Err(error) if is_stale(&error) => {
                enrollment.refresh(store, db).await?;
                self.pull_only_round_once(store, db).await
            }
            result => result,
        }
    }
    async fn pull_only_round_once(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        let inputs = store.tail_inputs(db, &self.locator).await?;
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

pub fn is_stale(error: &anyhow::Error) -> bool {
    error.is::<crate::sync::seed_claim::membership::StaleContext>()
}
