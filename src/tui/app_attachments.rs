use std::path::PathBuf;

use anyhow::Result;

use crate::attachments::optimization::ImageOptimizationPolicy;
use crate::config::resolve_blob_dir;
use crate::ids::new_id;
use crate::operations::AttachmentAddInput;
use crate::tui::app::App;
use crate::tui::attachment_controller::{
    AttachmentCompletion, AttachmentRequest, AttachmentSource,
};
use crate::tui::authoring::PendingTaskAttachment;
use crate::tui::overlay::{MultilineInputState, OverlayRoute, OverlayState};
use crate::tui::platform::{ClipboardImage, read_clipboard_image, read_clipboard_text};
use crate::tui::ui::attachment_is_locally_openable;

pub(crate) const DELETE_ATTACHMENT_TITLE: &str = "Remove image";

impl App {
    pub(super) fn begin_delete_attachment(&mut self, attachment_id: &str, scroll: u16) {
        let Some(attachment) = self
            .store
            .selected_task(self.widgets.table.selected())
            .and_then(|item| {
                item.attachments.iter().find(|attachment| {
                    attachment.attachment_id == attachment_id && !attachment.deleted
                })
            })
        else {
            self.set_warning("image attachment is unavailable");
            return;
        };
        let label = attachment
            .filename
            .as_deref()
            .or(attachment.alt_text.as_deref())
            .unwrap_or("attached image");
        self.pending_delete_attachment = Some(attachment_id.to_string());
        self.detail_context = true;
        self.detail_context_scroll = scroll;
        self.overlay = Some(OverlayState::confirm(
            OverlayRoute::DeleteAttachmentConfirm,
            DELETE_ATTACHMENT_TITLE,
            format!("Remove {label}?"),
        ));
    }

    pub(super) async fn submit_delete_attachment(&mut self) -> Result<()> {
        let Some(attachment_id) = self.pending_delete_attachment.take() else {
            self.set_warning("image removal confirmation is not active");
            self.restore_detail_overlay(true);
            return Ok(());
        };
        let replacement_attachment_id = self.attachment_focus_after_delete(&attachment_id);
        self.store.delete_attachment(&attachment_id).await?;
        self.selected_detail_attachment_id = None;
        self.external_image_exports
            .retain(|(retained_id, _)| retained_id != &attachment_id);
        self.refresh().await?;
        if replacement_attachment_id.is_some() {
            self.selected_detail_child_task_id = None;
            self.selected_detail_attachment_id = replacement_attachment_id;
        }
        self.set_success("removed image");
        self.restore_detail_overlay(true);
        Ok(())
    }

    fn attachment_focus_after_delete(&self, attachment_id: &str) -> Option<String> {
        let attachment_ids = self
            .store
            .selected_task(self.widgets.table.selected())?
            .attachments
            .iter()
            .filter(|attachment| attachment_is_locally_openable(attachment))
            .map(|attachment| attachment.attachment_id.clone())
            .collect::<Vec<_>>();
        let index = attachment_ids
            .iter()
            .position(|candidate| candidate == attachment_id)?;
        attachment_ids.get(index + 1).cloned().or_else(|| {
            index
                .checked_sub(1)
                .and_then(|previous| attachment_ids.get(previous).cloned())
        })
    }

    pub(super) async fn paste_detail_image_from_clipboard(&mut self) -> Result<()> {
        if !self.detail_accepts_image_paste() {
            self.set_info("open task detail to attach an image");
            return Ok(());
        }
        match read_clipboard_image()? {
            Some(image) => self.attach_clipboard_image(image).await?,
            None => {
                if let Some(text) = read_clipboard_text()?
                    && self.paste_detail_image_from_text(&text).await?
                {
                    return Ok(());
                }
                self.set_warning("clipboard does not contain an image");
            }
        }
        Ok(())
    }

    pub(super) async fn paste_detail_image_from_text(&mut self, text: &str) -> Result<bool> {
        if !self.detail_accepts_image_paste() {
            return Ok(false);
        }
        let Some(path) = pasted_image_path(text) else {
            return Ok(false);
        };
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("pasted-image")
            .to_string();
        self.attach_image_source(filename, AttachmentSource::Path(path))?;
        Ok(true)
    }

    pub(super) async fn paste_add_task_image_from_clipboard(&mut self) -> Result<()> {
        if !self.add_task_accepts_image_paste() {
            self.set_info("open add task composer to attach an image");
            return Ok(());
        }
        match read_clipboard_image()? {
            Some(image) => self.attach_add_task_image_bytes(image.filename, image.bytes)?,
            None => {
                if let Some(text) = read_clipboard_text()?
                    && self.paste_add_task_image_from_text(&text)?
                {
                    return Ok(());
                }
                self.set_warning("clipboard does not contain an image");
            }
        }
        Ok(())
    }

    pub(super) fn paste_add_task_image_from_text(&mut self, text: &str) -> Result<bool> {
        if !self.add_task_accepts_image_paste() {
            return Ok(false);
        }
        let Some(path) = pasted_image_path(text) else {
            return Ok(false);
        };
        let bytes = std::fs::read(&path)?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("pasted-image")
            .to_string();
        self.attach_add_task_image_bytes(filename, bytes)?;
        Ok(true)
    }

    fn add_task_accepts_image_paste(&self) -> bool {
        matches!(
            self.overlay,
            Some(OverlayState::AddTask(_))
                | Some(OverlayState::MultilineInput(MultilineInputState {
                    route: OverlayRoute::AddTaskNatural,
                    ..
                }))
        ) && self.pending_shortcut.is_empty()
            && self.footer_choice_mode.is_none()
    }

    fn attach_add_task_image_bytes(&mut self, filename: String, bytes: Vec<u8>) -> Result<()> {
        let pending = PendingTaskAttachment::new(
            new_id(),
            AttachmentAddInput {
                filename: Some(filename),
                alt_text: Some("pasted image".to_string()),
                declared_media_type: None,
                bytes,
                optimization_policy: if self
                    .intake
                    .config()
                    .local
                    .image_optimization
                    .optimizes_pasted_images()
                {
                    ImageOptimizationPolicy::Optimize
                } else {
                    ImageOptimizationPolicy::Preserve
                },
                dedupe_existing: false,
            },
        );
        let Some(is_new) = self.authoring.add_pending_add_task_attachment(pending) else {
            self.set_info("open add task composer to attach an image");
            return Ok(());
        };
        if is_new {
            self.set_success("attached image");
        } else {
            self.set_info("image already attached");
        }
        Ok(())
    }

    fn detail_accepts_image_paste(&self) -> bool {
        matches!(self.overlay, Some(OverlayState::Detail { .. }))
            && self.pending_shortcut.is_empty()
            && self.footer_choice_mode.is_none()
    }

    async fn attach_clipboard_image(&mut self, image: ClipboardImage) -> Result<()> {
        self.attach_image_source(image.filename, AttachmentSource::Bytes(image.bytes))
    }

    fn attach_image_source(&mut self, filename: String, source: AttachmentSource) -> Result<()> {
        let Some(item) = self
            .store
            .selected_task(self.widgets.table.selected())
            .cloned()
        else {
            self.set_info("no selected task to edit");
            return Ok(());
        };
        let db_path = self
            .intake
            .db_path()
            .ok_or_else(|| anyhow::anyhow!("database path is not available"))?;
        let blob_dir = resolve_blob_dir(db_path, self.intake.config())?;
        let optimization_policy = if self
            .intake
            .config()
            .local
            .image_optimization
            .optimizes_pasted_images()
        {
            ImageOptimizationPolicy::Optimize
        } else {
            ImageOptimizationPolicy::Preserve
        };
        let attachment_id = new_id();
        let store = self.store.attachment_worker_context();
        self.attachment_controller.start(AttachmentRequest {
            attachment_id,
            task_id: item.task.id,
            source,
            input: AttachmentAddInput {
                filename: Some(filename),
                alt_text: Some("pasted image".to_string()),
                declared_media_type: None,
                bytes: Vec::new(),
                optimization_policy,
                dedupe_existing: true,
            },
            blob_dir,
            lifecycle: self.intake.config().local.attachment_lifecycle,
            store,
        })?;
        self.set_info("preparing image attachment");
        Ok(())
    }

    pub(super) async fn poll_attachment_work(&mut self) -> Result<bool> {
        let results = self.attachment_controller.poll();
        if results.is_empty() {
            return Ok(false);
        }
        if results.iter().any(|result| {
            matches!(
                result.completion,
                AttachmentCompletion::Success | AttachmentCompletion::Duplicate
            )
        }) {
            self.refresh().await?;
        }
        let completion = if results
            .iter()
            .any(|result| result.completion == AttachmentCompletion::Failure)
        {
            AttachmentCompletion::Failure
        } else if results
            .iter()
            .any(|result| result.completion == AttachmentCompletion::Duplicate)
        {
            AttachmentCompletion::Duplicate
        } else {
            AttachmentCompletion::Success
        };
        match completion {
            AttachmentCompletion::Success => self.set_success("attached image"),
            AttachmentCompletion::Duplicate => self.set_info("image already attached"),
            AttachmentCompletion::Failure => self.set_error("image attachment failed"),
        }
        Ok(true)
    }
}

fn pasted_image_path(text: &str) -> Option<PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.lines().count() != 1 {
        return None;
    }
    let trimmed = trimmed.trim_matches('"').trim_matches('\'');
    let path = if trimmed.starts_with("file:") {
        url::Url::parse(trimmed).ok()?.to_file_path().ok()?
    } else {
        PathBuf::from(trimmed)
    };
    if !path.is_file() {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_image_path_accepts_supported_single_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("chart.png");
        std::fs::write(&path, b"png bytes").unwrap();

        assert_eq!(pasted_image_path(path.to_str().unwrap()), Some(path));
    }

    #[test]
    fn pasted_image_path_decodes_file_urls() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("chart image.png");
        std::fs::write(&path, b"png bytes").unwrap();
        let url = url::Url::from_file_path(&path).unwrap();

        assert_eq!(pasted_image_path(url.as_str()), Some(path));
    }

    #[test]
    fn pasted_image_path_ignores_plain_text() {
        assert_eq!(pasted_image_path("not an image"), None);
    }
}
