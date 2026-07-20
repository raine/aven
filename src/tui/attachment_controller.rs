use std::path::PathBuf;

use anyhow::{Result, bail};
use chrono::{SecondsFormat, TimeDelta, Utc};
use tokio::task::JoinSet;

use crate::config::AttachmentLifecycleConfig;
use crate::ids::TaskId;
use crate::operations::{AttachmentAddInput, TaskAttachmentAddInput};
use crate::tui::store::AttachmentWorkerContext;

const MAX_PENDING_ATTACHMENTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingAttachmentStatus {
    Preparing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingAttachmentView {
    pub(crate) attachment_id: String,
    pub(crate) task_id: TaskId,
    pub(crate) status: PendingAttachmentStatus,
}

pub(crate) enum AttachmentSource {
    Bytes(Vec<u8>),
    Path(PathBuf),
}

pub(crate) struct AttachmentRequest {
    pub(crate) attachment_id: String,
    pub(crate) task_id: TaskId,
    pub(crate) source: AttachmentSource,
    pub(crate) input: AttachmentAddInput,
    pub(crate) blob_dir: PathBuf,
    pub(crate) lifecycle: AttachmentLifecycleConfig,
    pub(crate) store: AttachmentWorkerContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachmentCompletion {
    Success,
    Duplicate,
    Failure,
}

pub(crate) struct AttachmentWorkResult {
    pub(crate) attachment_id: String,
    pub(crate) completion: AttachmentCompletion,
}

struct PendingAttachment {
    view: PendingAttachmentView,
}

pub(crate) struct AttachmentController {
    pending: Vec<PendingAttachment>,
    tasks: JoinSet<AttachmentWorkResult>,
    last_created_at: Option<chrono::DateTime<Utc>>,
}

impl AttachmentController {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::new(),
            tasks: JoinSet::new(),
            last_created_at: None,
        }
    }

    pub(crate) fn start(&mut self, request: AttachmentRequest) -> Result<()> {
        if self.pending.len() >= MAX_PENDING_ATTACHMENTS
            && let Some(index) = self
                .pending
                .iter()
                .position(|pending| pending.view.status == PendingAttachmentStatus::Failed)
        {
            self.pending.remove(index);
        }
        if self.pending.len() >= MAX_PENDING_ATTACHMENTS {
            bail!("too many image attachments are pending");
        }
        let created_at = self.next_created_at();
        self.pending.push(PendingAttachment {
            view: PendingAttachmentView {
                attachment_id: request.attachment_id.clone(),
                task_id: request.task_id.clone(),
                status: PendingAttachmentStatus::Preparing,
            },
        });
        self.tasks.spawn(run_attachment(request, created_at));
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Vec<AttachmentWorkResult> {
        let mut results = Vec::new();
        while let Some(joined) = self.tasks.try_join_next() {
            let Ok(result) = joined else {
                continue;
            };
            if result.completion == AttachmentCompletion::Failure {
                if let Some(pending) = self
                    .pending
                    .iter_mut()
                    .find(|pending| pending.view.attachment_id == result.attachment_id)
                {
                    pending.view.status = PendingAttachmentStatus::Failed;
                }
            } else {
                self.pending
                    .retain(|pending| pending.view.attachment_id != result.attachment_id);
            }
            results.push(result);
        }
        results
    }

    pub(crate) fn views(&self) -> Vec<PendingAttachmentView> {
        self.pending
            .iter()
            .map(|pending| pending.view.clone())
            .collect()
    }

    pub(crate) fn work_pending(&self) -> bool {
        !self.tasks.is_empty()
    }

    pub(crate) async fn shutdown(&mut self) {
        while let Some(result) = self.tasks.join_next().await {
            if let Ok(result) = result
                && result.completion != AttachmentCompletion::Failure
            {
                self.pending
                    .retain(|pending| pending.view.attachment_id != result.attachment_id);
            }
        }
        self.pending.clear();
    }

    fn next_created_at(&mut self) -> String {
        let current = Utc::now();
        let timestamp = match self.last_created_at {
            Some(previous) if current <= previous => previous + TimeDelta::nanoseconds(1),
            _ => current,
        };
        self.last_created_at = Some(timestamp);
        timestamp.to_rfc3339_opts(SecondsFormat::Nanos, true)
    }
}

async fn run_attachment(
    mut request: AttachmentRequest,
    created_at: String,
) -> AttachmentWorkResult {
    let attachment_id = request.attachment_id.clone();
    let completion = async {
        request.input.bytes = match request.source {
            AttachmentSource::Bytes(bytes) => bytes,
            AttachmentSource::Path(path) => tokio::fs::read(path).await?,
        };
        let created = request
            .store
            .add_ordered_attachment(
                &request.blob_dir,
                request.lifecycle.policy(),
                &request.task_id,
                created_at,
                TaskAttachmentAddInput {
                    attachment_id: request.attachment_id,
                    input: request.input,
                },
            )
            .await?;
        Ok::<_, anyhow::Error>(if created {
            AttachmentCompletion::Success
        } else {
            AttachmentCompletion::Duplicate
        })
    }
    .await
    .unwrap_or(AttachmentCompletion::Failure);
    AttachmentWorkResult {
        attachment_id,
        completion,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_order_timestamps_are_strictly_increasing() {
        let mut controller = AttachmentController::new();
        let first = controller.next_created_at();
        let second = controller.next_created_at();
        assert!(first < second);
    }
}
