use super::*;
use crate::sync::encrypted_tail::attachments::{
    self as images, Operation as ImageOperation, Reply as ImageReply, Ticket,
};
pub use aven_protocol::wire::images::PATH as IMAGES_PATH;
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Bounds serial append work in one round while allowing a large offline backlog
/// to clear well within the drain's round budget.
const PUSH_LIMIT: usize = 2048;

struct ImageRoundLimits {
    objects: usize,
    elapsed: Duration,
    metadata_pull_skips: usize,
}

/// A round transfers one image at a time until either bound is reached, and
/// always attempts at least one transfer so slow pushes, pulls or stale-context
/// refreshes earlier in the round cannot starve image progress. A complete
/// metadata observation may be reused only for this many intervening
/// image-only rounds before another pull is required.
const IMAGE_ROUND_LIMITS: ImageRoundLimits = ImageRoundLimits {
    objects: 16,
    elapsed: Duration::from_secs(2),
    metadata_pull_skips: 3,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageTransfer {
    Complete,
    Pending,
    Failed,
    Unavailable,
}

impl std::fmt::Display for ImageTransfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Complete => "complete",
            Self::Pending => "pending",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Round {
    /// The server has no further page and no local change waits to upload.
    /// While publishing is blocked, withheld local changes do not count.
    pub metadata_caught_up: bool,
    pub images: ImageTransfer,
    /// Metadata records appended by this device during the round.
    pub sent_changes: usize,
    /// Metadata records applied from the server during the round.
    pub received_changes: usize,
    /// New encrypted content waits for a withdrawal rotation; this round
    /// uploaded nothing but still pulled and downloaded.
    pub publishing_blocked: bool,
}

pub struct DrainSnapshot {
    tail: TailSnapshot,
    pull: PullFreshness,
    batch_count: usize,
    /// Why this drain's withdrawal rotation failed, if it did.
    withdrawal: Option<anyhow::Error>,
}

#[derive(Default)]
struct PullFreshness {
    complete: bool,
    skipped_rounds: usize,
}
impl DrainSnapshot {
    /// The error reported for a drain that ended with publishing blocked.
    pub fn publishing_blocked_error(&mut self) -> anyhow::Error {
        match self.withdrawal.take() {
            Some(error) => error.context(PublishingBlocked),
            None => PublishingBlocked.into(),
        }
    }
}

async fn validated_tail_inputs(
    store: &ProtectedLocalKeyStore,
    db: &Database,
    locator: &str,
) -> Result<TailSnapshot> {
    let inputs = store.tail_inputs(db, locator).await?;
    db.validate_encrypted_attachment_integrity(&inputs.authority)
        .await?;
    Ok(inputs)
}

struct RoundProgress {
    pushes: usize,
    batch_collisions: usize,
    preflight: Option<tail::Preflight>,
    push_complete: bool,
    publishing_blocked: bool,
    page_complete: Option<bool>,
    pulled: bool,
    image_state: Option<ImageTransfer>,
    selected: bool,
    download: Option<images::Download>,
    image_started: Instant,
    image_transfers: usize,
}

impl Default for RoundProgress {
    fn default() -> Self {
        Self {
            pushes: 0,
            batch_collisions: 0,
            preflight: None,
            push_complete: false,
            publishing_blocked: false,
            page_complete: None,
            pulled: false,
            image_state: None,
            selected: false,
            download: None,
            image_started: Instant::now(),
            image_transfers: 0,
        }
    }
}

impl RoundProgress {
    fn image_budget_available(&self) -> bool {
        self.image_transfers == 0
            || (self.image_transfers < IMAGE_ROUND_LIMITS.objects
                && self.image_started.elapsed() < IMAGE_ROUND_LIMITS.elapsed)
    }

    fn transferred_image(&mut self) {
        self.image_transfers += 1;
    }
}
impl Client {
    pub async fn image_exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        operation: ImageOperation,
    ) -> Result<ImageReply> {
        let request_limit = if matches!(operation, ImageOperation::Put { .. }) {
            images::HTTP_LIMIT
        } else {
            tail::CONTROL_LIMIT
        };
        let response_limit = if matches!(operation, ImageOperation::Read { .. }) {
            images::HTTP_LIMIT
        } else {
            tail::CONTROL_LIMIT
        };
        self.exchange_to(
            IMAGES_PATH,
            context,
            bearer,
            operation,
            request_limit,
            response_limit,
        )
        .await
    }
    pub async fn start_drain(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<DrainSnapshot> {
        let enrollment = self.enrollment()?;
        enrollment.finish_pending_removal(store, db).await?;
        // A failed withdrawal keeps publishing blocked without stopping pulls.
        let withdrawal = Box::pin(enrollment.withdraw_expired_disclosure(store, db))
            .await
            .err();
        Ok(DrainSnapshot {
            tail: validated_tail_inputs(store, db, &self.locator).await?,
            pull: PullFreshness::default(),
            withdrawal,
            batch_count: 0,
        })
    }
    /// Pushes a bounded ordered prefix, applies at most one metadata page and
    /// transfers a bounded number of images serially. The caller owns the local
    /// blob root; committed metadata is independent of image transfer success.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::LifecyclePolicy,
    ) -> Result<Round> {
        let mut drain = Box::pin(self.start_drain(store, db)).await?;
        Box::pin(self.round_in_drain(store, db, blob_dir, lifecycle_policy, &mut drain)).await
    }
    pub async fn round_in_drain(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::LifecyclePolicy,
        drain: &mut DrainSnapshot,
    ) -> Result<Round> {
        if !drain.tail.is_current(db).await? {
            drain.tail = validated_tail_inputs(store, db, &self.locator).await?;
        }
        let mut progress = RoundProgress::default();
        retry_stale!(
            self.round_once(
                &drain.tail,
                &mut drain.pull,
                db,
                blob_dir,
                lifecycle_policy,
                &mut progress,
                &mut drain.batch_count
            )
            .await,
            async {
                self.enrollment()?
                    .refresh_and_finish_pending_removal(store, db)
                    .await?;
                drain.tail = validated_tail_inputs(store, db, &self.locator).await?;
                drain.pull = PullFreshness::default();
                progress.preflight = None;
                progress.page_complete = None;
                progress.pulled = false;
                anyhow::Ok(())
            }
            .await,
        )
    }
    #[allow(clippy::too_many_arguments)]
    async fn round_once(
        &self,
        inputs: &TailSnapshot,
        pull: &mut PullFreshness,
        db: &Database,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::LifecyclePolicy,
        progress: &mut RoundProgress,
        batch_count: &mut usize,
    ) -> Result<Round> {
        let a = &inputs.authority;
        while !progress.push_complete && progress.pushes < PUSH_LIMIT {
            let (step, preflight) = match self
                .push_in_run(
                    inputs,
                    db,
                    blob_dir,
                    progress.preflight.clone(),
                    batch_count,
                    PUSH_LIMIT - progress.pushes,
                )
                .await
            {
                Err(error)
                    if super::super::errors::has_code(&error, "encrypted-tail-batch-known")
                        && progress.batch_collisions < 3 =>
                {
                    progress.batch_collisions += 1;
                    continue;
                }
                Err(error) if error.is::<PublishingBlocked>() => {
                    progress.publishing_blocked = true;
                    break;
                }
                result => result?,
            };
            progress.preflight = preflight;
            match step {
                PushStep::Appended => progress.pushes += 1,
                PushStep::BatchAppended(count) => progress.pushes += count,
                PushStep::Image(Some(state)) => {
                    progress.image_state = Some(state);
                    progress.push_complete = true;
                }
                PushStep::Image(None) => {
                    progress.transferred_image();
                    if !progress.image_budget_available() {
                        progress.push_complete = true;
                    }
                }
                PushStep::Empty => progress.push_complete = true,
            }
        }
        // Reaching the cap completes only this round's push phase. The next
        // bounded round resumes from the next ordered frozen group.
        progress.push_complete = true;
        let state_before_pull = db.encrypted_round_state(a).await?;
        let cursor_before_pull = state_before_pull.cursor;
        let image_work_waiting = state_before_pull.upload_pending
            || state_before_pull
                .downloads
                .is_some_and(|downloads| downloads.pending);
        let may_skip_pull = pull.complete
            && progress.pushes == 0
            && image_work_waiting
            && pull.skipped_rounds < IMAGE_ROUND_LIMITS.metadata_pull_skips;
        if progress.page_complete.is_none() && !may_skip_pull {
            progress.page_complete = Some(self.pull(a, &inputs.bearer, db).await?);
            progress.pulled = true;
            pull.complete = progress.page_complete == Some(true);
            pull.skipped_rounds = 0;
        } else if progress.page_complete.is_none() {
            progress.page_complete = Some(true);
            pull.skipped_rounds += 1;
        }
        let state = db.encrypted_round_state(a).await?;
        let mut downloaded = None;
        if progress.image_state.is_none() && state.downloads.is_some() {
            while progress.image_budget_available() {
                if !progress.selected {
                    progress.download = db.prepare_encrypted_image_download(a).await?;
                    progress.selected = true;
                }
                if progress.download.is_none() {
                    break;
                }
                match self
                    .download_image(
                        a,
                        &inputs.bearer,
                        db,
                        blob_dir,
                        progress.download.as_ref(),
                        lifecycle_policy,
                    )
                    .await
                {
                    Ok(true) => {
                        progress.transferred_image();
                        progress.selected = false;
                        progress.download = None;
                    }
                    Ok(false) => {
                        downloaded = Some(ImageTransfer::Unavailable);
                        break;
                    }
                    Err(error)
                        if is_stale(&error) || super::super::errors::is_hosting_refusal(&error) =>
                    {
                        return Err(error);
                    }
                    Err(_) => {
                        downloaded = Some(ImageTransfer::Failed);
                        break;
                    }
                }
            }
        }
        let final_state = db.encrypted_round_state(a).await?;
        let images =
            downloaded.unwrap_or_else(|| settled(&final_state, progress.publishing_blocked));
        Ok(Round {
            metadata_caught_up: progress.pulled
                && progress.page_complete == Some(true)
                && (final_state.idle || progress.publishing_blocked),
            // A failed push outranks later download outcomes in this round.
            images: progress.image_state.unwrap_or(images),
            sent_changes: progress.pushes,
            received_changes: final_state.cursor.saturating_sub(cursor_before_pull) as usize,
            publishing_blocked: progress.publishing_blocked,
        })
    }
    pub async fn upload_prepared_image(
        &self,
        inputs: &TailSnapshot,
        upload: images::Upload,
    ) -> Result<Ticket> {
        let (a, bearer) = (&inputs.authority, &inputs.bearer);
        inputs.require_publishing_ready()?;
        let ImageReply::Status(status) = self
            .image_exchange(
                &a.context,
                bearer,
                ImageOperation::Declare {
                    workspace: upload.workspace.clone(),
                    descriptor: upload.descriptor.clone(),
                },
            )
            .await?
        else {
            anyhow::bail!("error encrypted-image-reply")
        };
        let mut indices = std::collections::HashSet::new();
        ensure!(
            status.missing.len() <= upload.records.len()
                && (!status.complete || status.missing.is_empty())
                && status.expires_at.is_some(),
            "error encrypted-image-status"
        );
        ensure!(
            status
                .missing
                .iter()
                .all(|i| *i < upload.records.len() && indices.insert(*i)),
            "error encrypted-image-status"
        );
        let ticket = Ticket {
            reservation: status
                .reservation
                .ok_or_else(|| anyhow::anyhow!("error encrypted-image-ticket"))?,
        };
        for index in status.missing {
            inputs.require_publishing_ready()?;
            let record = upload
                .records
                .get(index)
                .ok_or_else(|| anyhow::anyhow!("error encrypted-image-index"))?
                .clone();
            ensure!(
                matches!(
                    self.image_exchange(
                        &a.context,
                        bearer,
                        ImageOperation::Put {
                            workspace: upload.workspace.clone(),
                            object: upload.object,
                            descriptor_commitment: upload.commitment,
                            reservation: ticket.reservation,
                            index,
                            record
                        }
                    )
                    .await?,
                    ImageReply::Done
                ),
                "error encrypted-image-reply"
            );
        }
        crate::sync::crash::Crash::Tail.at("image-put");
        inputs.require_publishing_ready()?;
        ensure!(
            matches!(
                self.image_exchange(
                    &a.context,
                    bearer,
                    ImageOperation::Complete {
                        workspace: upload.workspace,
                        object: upload.object,
                        descriptor_commitment: upload.commitment,
                        reservation: ticket.reservation
                    }
                )
                .await?,
                ImageReply::Done
            ),
            "error encrypted-image-reply"
        );
        Ok(ticket)
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
        let enrollment = self.enrollment()?;
        enrollment.refresh(store, db).await?;
        retry_stale!(
            self.repair_attachment_once(store, db, blob_dir, workspace, reference)
                .await,
            enrollment.refresh(store, db).await,
        )
    }
    async fn repair_attachment_once(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        workspace: &str,
        reference: &str,
    ) -> Result<()> {
        let inputs = validated_tail_inputs(store, db, &self.locator).await?;
        let upload = db
            .repair_encrypted_image(&inputs.authority, blob_dir, workspace, reference)
            .await?;
        let object = upload.object;
        let commitment = upload.commitment;
        let ticket = self.upload_prepared_image(&inputs, upload).await?;
        ensure!(
            matches!(
                self.image_exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    ImageOperation::Release {
                        workspace: workspace.into(),
                        object,
                        descriptor_commitment: commitment,
                        reservation: ticket.reservation
                    }
                )
                .await?,
                ImageReply::Done
            ),
            "error encrypted-image-release"
        );
        Ok(())
    }
    /// False means the server no longer holds the selected object's bytes.
    async fn download_image(
        &self,
        a: &tail::Authority,
        bearer: &Secret,
        db: &Database,
        blob_dir: &Path,
        selected: Option<&images::Download>,
        lifecycle_policy: crate::attachments::LifecyclePolicy,
    ) -> Result<bool> {
        if let Some(download) = selected
            && !db
                .complete_encrypted_image_from_local(a, blob_dir, download, lifecycle_policy)
                .await?
        {
            let mut records = Vec::new();
            let mut total = 0;
            for index in 0..download.chunk_count {
                match self
                    .image_exchange(
                        &a.context,
                        bearer,
                        ImageOperation::Read {
                            workspace: download.workspace.clone(),
                            object: download.object,
                            descriptor_commitment: download.descriptor_commitment,
                            index,
                        },
                    )
                    .await?
                {
                    ImageReply::Chunk(record) => {
                        total += record.len();
                        ensure!(
                            total <= images::TRANSFER_BYTES,
                            "error encrypted-image-limit"
                        );
                        records.push(record);
                    }
                    ImageReply::Unavailable => return Ok(false),
                    _ => anyhow::bail!("error encrypted-image-reply"),
                }
            }
            db.install_encrypted_image(a, blob_dir, download, &records, lifecycle_policy)
                .await?;
        }
        Ok(true)
    }
}
/// Image availability after this round's transfer step. Uploads withheld
/// while publishing is blocked do not keep the drain going.
fn settled(state: &tail::RoundState, publishing_blocked: bool) -> ImageTransfer {
    match state.downloads {
        Some(d) if d.pending => ImageTransfer::Pending,
        Some(d) if d.unavailable => ImageTransfer::Unavailable,
        Some(_) if !state.upload_pending || publishing_blocked => ImageTransfer::Complete,
        _ => ImageTransfer::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_round_time_still_allows_the_first_image_transfer() {
        let mut progress = RoundProgress {
            image_started: Instant::now() - IMAGE_ROUND_LIMITS.elapsed,
            ..RoundProgress::default()
        };
        assert!(progress.image_budget_available());
        progress.transferred_image();
        assert!(!progress.image_budget_available());
    }
}
