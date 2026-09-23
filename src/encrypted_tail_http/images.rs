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
pub struct AttachmentRound {
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
    caught_up: Option<bool>,
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
    /// One metadata round and at most one image per direction. The caller owns
    /// the local blob root; committed metadata is independent of download success.
    pub async fn attachment_round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
    ) -> Result<AttachmentRound> {
        let enrollment = crate::peer_enrollment_http::Client::new(&self.locator)?;
        enrollment.refresh(store, db).await?;
        let mut progress = RoundProgress::default();
        match self
            .attachment_round_once(store, db, blob_dir, &mut progress)
            .await
        {
            Err(error) if is_stale(&error) => {
                enrollment.refresh(store, db).await?;
                self.attachment_round_once(store, db, blob_dir, &mut progress)
                    .await
            }
            result => result,
        }
    }
    async fn attachment_round_once(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        progress: &mut RoundProgress,
    ) -> Result<AttachmentRound> {
        let inputs = store.tail_inputs(db, &self.locator).await?;
        let a = &inputs.authority;
        if !progress.pushed {
            if let Some((id, _)) = db.encrypted_tail_frozen_record(a).await? {
                match self
                    .exchange(
                        &a.context,
                        &inputs.bearer,
                        Operation::Lookup {
                            operation_id: id,
                            expected: None,
                        },
                    )
                    .await?
                {
                    Reply::Found(accepted) => {
                        db.observe_encrypted_tail(a, &accepted.mapping).await?;
                        db.verify_encrypted_tail_outcome(a, &accepted).await?;
                    }
                    Reply::Absent => {}
                    _ => anyhow::bail!("error encrypted-image-accepted-identity"),
                }
            }
            let upload = self.upload_image(a, &inputs.bearer, db, blob_dir).await;
            progress.image_state = match upload {
                Ok(ticket) => {
                    self.push(a, &inputs.bearer, db, ticket).await?;
                    None
                }
                Err(error) if is_stale(&error) => return Err(error),
                Err(_) => Some(ImageTransfer::Failed),
            };
            progress.pushed = true;
        }
        let caught_up = match progress.caught_up {
            Some(value) => value,
            None => {
                let value =
                    self.pull(a, &inputs.bearer, db).await? && db.encrypted_tail_idle(a).await?;
                progress.caught_up = Some(value);
                value
            }
        };
        let image_state = progress.image_state;
        let images = if !db.encrypted_images_initial_catch_up_complete(a).await? {
            image_state.unwrap_or(ImageTransfer::Pending)
        } else {
            if !progress.selected {
                progress.download = db.prepare_encrypted_image_download(a).await?;
                progress.selected = true;
            }
            match self
                .download_image(a, &inputs.bearer, db, blob_dir, progress.download.as_ref())
                .await
            {
                Ok(ImageTransfer::Complete) if db.encrypted_image_upload_pending(a).await? => {
                    image_state.unwrap_or(ImageTransfer::Pending)
                }
                Ok(status) => image_state.unwrap_or(status),
                Err(error) if is_stale(&error) => return Err(error),
                Err(_) => ImageTransfer::Failed,
            }
        };
        Ok(AttachmentRound {
            metadata_caught_up: caught_up,
            images,
        })
    }
    async fn upload_image(
        &self,
        a: &tail::Authority,
        bearer: &Secret,
        db: &Database,
        blob_dir: &Path,
    ) -> Result<Option<Ticket>> {
        let Some(upload) = db.prepare_encrypted_image(a, blob_dir).await? else {
            return Ok(None);
        };
        self.upload_prepared_image(a, bearer, upload)
            .await
            .map(Some)
    }
    async fn upload_prepared_image(
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
    async fn download_image(
        &self,
        a: &tail::Authority,
        bearer: &Secret,
        db: &Database,
        blob_dir: &Path,
        selected: Option<&images::Download>,
    ) -> Result<ImageTransfer> {
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
                    ImageReply::Unavailable => return Ok(ImageTransfer::Unavailable),
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
        if db.encrypted_image_download_pending(a).await? {
            Ok(ImageTransfer::Pending)
        } else if db.encrypted_images_unavailable(a).await? {
            Ok(ImageTransfer::Unavailable)
        } else {
            Ok(ImageTransfer::Complete)
        }
    }
}
