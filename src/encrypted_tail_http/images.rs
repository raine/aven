use super::*;
use aven_core::sync::encrypted_tail::attachments::{
    self as images, Operation as ImageOperation, Reply as ImageReply, Ticket,
};
use std::path::Path;
pub(super) const PATH: &str = "/e2ee/images/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageTransfer {
    Complete,
    Pending,
    Failed,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Round {
    pub metadata_caught_up: bool,
    pub images: ImageTransfer,
}

pub(super) async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let mut response = match server.gate.try_acquire() {
        Ok(_permit) => match tokio::time::timeout(
            REQUEST_TIMEOUT,
            dispatch(&server.db, request, server.image_policy),
        )
        .await
        {
            Ok(Ok(reply)) => match serde_json::to_vec(&reply) {
                Ok(bytes) if bytes.len() <= images::HTTP_LIMIT => {
                    ([(header::CONTENT_TYPE, "application/json")], bytes).into_response()
                }
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "encrypted_image_refused").into_response(),
            },
            Ok(Err(error)) if is_stale(&error) => {
                (StatusCode::CONFLICT, "membership-stale").into_response()
            }
            Ok(Err(_)) => (StatusCode::CONFLICT, "encrypted_image_refused").into_response(),
            Err(_) => (StatusCode::REQUEST_TIMEOUT, "encrypted_image_timeout").into_response(),
        },
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "encrypted_image_busy").into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
async fn dispatch(
    db: &Database,
    request: Request,
    policy: aven_core::attachments::LifecyclePolicy,
) -> Result<Envelope<ImageReply>> {
    ensure!(
        request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            == Some("application/json")
            && !request.headers().contains_key(header::CONTENT_ENCODING),
        "error encrypted-image-http"
    );
    let auth = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| anyhow::anyhow!("error encrypted-image-credential"))?;
    ensure!(
        auth.len() == 64
            && auth
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "error encrypted-image-credential"
    );
    let bearer = Secret::new(
        hex::decode(auth)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("error encrypted-image-credential"))?,
    );
    let bytes = to_bytes(request.into_body(), images::HTTP_LIMIT)
        .await
        .map_err(|_| anyhow::anyhow!("error encrypted-image-limit"))?;
    let input: Envelope<ImageOperation> = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("error encrypted-image-http"))?;
    ensure!(
        matches!(input.operation, ImageOperation::Put { .. }) || bytes.len() <= tail::CONTROL_LIMIT,
        "error encrypted-image-limit"
    );
    let operation = db
        .encrypted_image_exchange(&input.context, &bearer, input.operation, policy)
        .await?;
    Ok(Envelope {
        context: input.context,
        correlation: input.correlation,
        operation,
    })
}
#[derive(Default)]
struct RoundProgress {
    pushed: bool,
    page_complete: Option<bool>,
    image_state: Option<ImageTransfer>,
    selected: bool,
    download: Option<images::Download>,
}
impl Client {
    pub(super) async fn image_exchange(
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
            PATH,
            context,
            bearer,
            operation,
            request_limit,
            response_limit,
        )
        .await
    }
    /// Resolves at most one ordered local head, applies one metadata page and
    /// downloads at most one image. The caller owns the local blob root;
    /// committed metadata is independent of image transfer success.
    pub async fn round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
    ) -> Result<Round> {
        let enrollment = crate::peer_enrollment_http::Client::new(&self.locator)?;
        enrollment.finish_pending_management(store, db).await?;
        let mut progress = RoundProgress::default();
        match self.round_once(store, db, blob_dir, &mut progress).await {
            Err(error) if is_stale(&error) => {
                enrollment.refresh(store, db).await?;
                self.round_once(store, db, blob_dir, &mut progress).await
            }
            result => result,
        }
    }
    async fn round_once(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        progress: &mut RoundProgress,
    ) -> Result<Round> {
        let inputs = store.tail_inputs(db, &self.locator).await?;
        let a = &inputs.authority;
        if !progress.pushed {
            progress.image_state = self.push(a, &inputs.bearer, db, blob_dir).await?;
            progress.pushed = true;
        }
        if progress.page_complete.is_none() {
            progress.page_complete = Some(self.pull(a, &inputs.bearer, db).await?);
        }
        let state = db.encrypted_round_state(a).await?;
        let images = if state.downloads.is_some() {
            if !progress.selected {
                progress.download = db.prepare_encrypted_image_download(a).await?;
                progress.selected = true;
            }
            match self
                .download_image(a, &inputs.bearer, db, blob_dir, progress.download.as_ref())
                .await
            {
                Ok(true) => settled(&db.encrypted_round_state(a).await?),
                Ok(false) => ImageTransfer::Unavailable,
                Err(error) if is_stale(&error) => return Err(error),
                Err(_) => ImageTransfer::Failed,
            }
        } else {
            ImageTransfer::Pending
        };
        Ok(Round {
            metadata_caught_up: progress.page_complete == Some(true) && state.idle,
            // A failed push outranks later download outcomes in this round.
            images: progress.image_state.unwrap_or(images),
        })
    }
    pub(super) async fn upload_prepared_image(
        &self,
        a: &tail::Authority,
        bearer: &Secret,
        upload: images::Upload,
    ) -> Result<Ticket> {
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
            status.epoch > 0
                && status.missing.len() <= upload.records.len()
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
            epoch: status.epoch,
            reservation: status
                .reservation
                .ok_or_else(|| anyhow::anyhow!("error encrypted-image-ticket"))?,
        };
        for index in status.missing {
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
                            epoch: ticket.epoch,
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
        #[cfg(test)]
        if std::env::var("AVEN_TAIL_CRASH").as_deref() == Ok("image-put") {
            std::process::exit(84);
        }
        ensure!(
            matches!(
                self.image_exchange(
                    &a.context,
                    bearer,
                    ImageOperation::Complete {
                        workspace: upload.workspace,
                        object: upload.object,
                        descriptor_commitment: upload.commitment,
                        epoch: ticket.epoch,
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
        let enrollment = crate::peer_enrollment_http::Client::new(&self.locator)?;
        enrollment.refresh(store, db).await?;
        match self
            .repair_attachment_once(store, db, blob_dir, workspace, reference)
            .await
        {
            Err(error) if is_stale(&error) => {
                enrollment.refresh(store, db).await?;
                self.repair_attachment_once(store, db, blob_dir, workspace, reference)
                    .await
            }
            result => result,
        }
    }
    async fn repair_attachment_once(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        workspace: &str,
        reference: &str,
    ) -> Result<()> {
        let inputs = store.tail_inputs(db, &self.locator).await?;
        let upload = db
            .repair_encrypted_image(&inputs.authority, blob_dir, workspace, reference)
            .await?;
        let object = upload.object;
        let commitment = upload.commitment;
        let ticket = self
            .upload_prepared_image(&inputs.authority, &inputs.bearer, upload)
            .await?;
        ensure!(
            matches!(
                self.image_exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    ImageOperation::Release {
                        workspace: workspace.into(),
                        object,
                        descriptor_commitment: commitment,
                        epoch: ticket.epoch,
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
    ) -> Result<bool> {
        if let Some(download) = selected
            && !db
                .complete_encrypted_image_from_local(
                    a,
                    blob_dir,
                    download,
                    crate::config::AttachmentLifecycleConfig::default().policy(),
                )
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
            db.install_encrypted_image(
                a,
                blob_dir,
                download,
                &records,
                crate::config::AttachmentLifecycleConfig::default().policy(),
            )
            .await?;
        }
        Ok(true)
    }
}
/// Image availability after this round's transfer step.
fn settled(state: &tail::RoundState) -> ImageTransfer {
    match state.downloads {
        Some(d) if d.pending => ImageTransfer::Pending,
        Some(d) if d.unavailable => ImageTransfer::Unavailable,
        Some(_) if !state.upload_pending => ImageTransfer::Complete,
        _ => ImageTransfer::Pending,
    }
}
