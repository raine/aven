use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::attachments::AttachmentBytesState;
use crate::attachments::optimization::ImageOptimizationPolicy;
use crate::attachments::save::AttachmentSaveError;
use crate::config::resolve_blob_dir;
use crate::ids::new_id;
use crate::operations::AttachmentAddInput;
use crate::tui::app::{App, DetailTargetId};
use crate::tui::attachment_controller::{
    AttachmentCompletion, AttachmentRequest, AttachmentSource,
};
use crate::tui::authoring::PendingTaskAttachment;
use crate::tui::overlay::{
    ConfirmIntent, MultilineInputState, MultilineIntent, OverlayState, TextIntent,
};
use crate::tui::platform::{ClipboardImage, read_clipboard_image, read_clipboard_text};
use crate::tui::ui::attachment_is_locally_openable;

pub(crate) const DELETE_ATTACHMENT_TITLE: &str = "Remove image";

impl App {
    pub(super) fn attachment_bytes_are_available(
        &self,
        attachment: &crate::task_render::AttachmentMetadataJson,
    ) -> bool {
        if attachment.deleted
            || !attachment.has_blob
            || attachment.bytes_state != AttachmentBytesState::Present
        {
            return false;
        }
        let Some(db_path) = self.intake.db_path() else {
            return false;
        };
        let Ok(blob_dir) = resolve_blob_dir(db_path, self.intake.config()) else {
            return false;
        };
        crate::attachments::storage::object_path(&blob_dir, &attachment.sha256)
            .is_ok_and(|path| path.is_file())
    }

    pub(super) fn begin_delete_attachment(&mut self, attachment_id: &str, scroll: u16) {
        let attachment = self
            .store
            .selected_task(self.list.selected_task())
            .and_then(|item| {
                item.attachments.iter().find(|attachment| {
                    attachment.attachment_id == attachment_id && !attachment.deleted
                })
            })
            .cloned();
        let Some(attachment) = attachment else {
            self.set_warning("image attachment is unavailable");
            return;
        };
        self.begin_delete_attachment_metadata(&attachment, scroll);
    }

    pub(super) fn begin_delete_attachment_metadata(
        &mut self,
        attachment: &crate::task_render::AttachmentMetadataJson,
        scroll: u16,
    ) {
        let label = attachment
            .filename
            .as_deref()
            .or(attachment.alt_text.as_deref())
            .unwrap_or("attached image");
        if let Some(detail) = self.detail.state_mut() {
            detail.set_scroll(scroll);
        }
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::DeleteAttachment {
                attachment_id: attachment.attachment_id.clone(),
            },
            DELETE_ATTACHMENT_TITLE,
            format!("Remove {label}?"),
        ));
    }

    pub(super) fn begin_save_attachment(&mut self, attachment_id: &str, scroll: u16) {
        let attachment = self
            .store
            .selected_task(self.list.selected_task())
            .and_then(|item| {
                item.attachments.iter().find(|attachment| {
                    attachment.attachment_id == attachment_id && !attachment.deleted
                })
            })
            .cloned();
        let Some(attachment) = attachment else {
            self.set_warning("attachment is no longer available");
            return;
        };
        self.begin_save_attachment_metadata(&attachment, scroll);
    }

    pub(super) fn begin_save_attachment_metadata(
        &mut self,
        attachment: &crate::task_render::AttachmentMetadataJson,
        scroll: u16,
    ) {
        if !self.attachment_bytes_are_available(attachment) {
            self.set_warning("attachment bytes are unavailable");
            return;
        }
        let filename = attachment_default_filename(attachment);
        let Ok(current_dir) = std::env::current_dir() else {
            self.set_error("could not resolve the save destination");
            return;
        };
        if let Some(detail) = self.detail.state_mut() {
            detail.set_scroll(scroll);
        }
        self.overlay = Some(OverlayState::text_input(
            TextIntent::SaveAttachment {
                attachment_id: attachment.attachment_id.clone(),
                filename: filename.clone(),
                scroll,
            },
            "Save attachment as",
            "destination path:",
            current_dir.join(filename).display().to_string(),
        ));
    }

    pub(super) async fn submit_save_attachment(
        &mut self,
        attachment_id: String,
        filename: String,
        scroll: u16,
        value: String,
    ) -> Result<()> {
        let entered = value.trim();
        if entered.is_empty() {
            self.reopen_save_attachment_input(
                attachment_id,
                filename,
                scroll,
                value,
                "enter a destination path",
                false,
            );
            return Ok(());
        }
        let mut destination = match crate::config::expand_tilde(Path::new(entered)) {
            Ok(path) => path,
            Err(_) => {
                self.reopen_save_attachment_input(
                    attachment_id,
                    filename,
                    scroll,
                    value,
                    "could not resolve the destination path",
                    true,
                );
                return Ok(());
            }
        };
        if destination.is_relative() {
            destination = match std::env::current_dir() {
                Ok(current_dir) => current_dir.join(destination),
                Err(_) => {
                    self.reopen_save_attachment_input(
                        attachment_id,
                        filename,
                        scroll,
                        value,
                        "could not resolve the destination path",
                        true,
                    );
                    return Ok(());
                }
            };
        }
        if destination.is_dir() {
            destination = destination.join(&filename);
        }
        let Some(db_path) = self.intake.db_path() else {
            self.show_detail(scroll);
            self.set_error("could not resolve attachment storage");
            return Ok(());
        };
        let Ok(blob_dir) = resolve_blob_dir(db_path, self.intake.config()) else {
            self.show_detail(scroll);
            self.set_error("could not resolve attachment storage");
            return Ok(());
        };
        match self
            .store
            .save_attachment(&blob_dir, &attachment_id, destination.clone())
            .await
        {
            Ok(outcome) => {
                self.show_detail(scroll);
                if outcome.lease_release_failed {
                    self.set_warning(format!(
                        "saved attachment to {}; read protection expires automatically",
                        outcome.destination.display()
                    ));
                } else {
                    self.set_success(format!(
                        "saved attachment to {}",
                        outcome.destination.display()
                    ));
                }
            }
            Err(AttachmentSaveError::Invalidated) => {
                self.show_detail(scroll);
                self.set_warning("attachment is no longer available");
            }
            Err(AttachmentSaveError::BlobUnavailable) => {
                self.show_detail(scroll);
                self.set_warning("attachment bytes are unavailable");
            }
            Err(AttachmentSaveError::StorageUnavailable) => {
                self.show_detail(scroll);
                self.set_error("could not read attachment storage");
            }
            Err(AttachmentSaveError::DestinationExists(path)) => {
                self.reopen_save_attachment_input(
                    attachment_id,
                    filename,
                    scroll,
                    value,
                    format!("destination already exists: {}", path.display()),
                    false,
                );
            }
            Err(AttachmentSaveError::DestinationParentUnavailable(path)) => {
                self.reopen_save_attachment_input(
                    attachment_id,
                    filename,
                    scroll,
                    value,
                    format!("destination directory is unavailable: {}", path.display()),
                    false,
                );
            }
            Err(AttachmentSaveError::WriteFailed(path)) => {
                self.reopen_save_attachment_input(
                    attachment_id,
                    filename,
                    scroll,
                    value,
                    format!("could not save attachment to {}", path.display()),
                    true,
                );
            }
        }
        Ok(())
    }

    fn reopen_save_attachment_input(
        &mut self,
        attachment_id: String,
        filename: String,
        scroll: u16,
        value: String,
        message: impl Into<String>,
        error: bool,
    ) {
        self.overlay = Some(OverlayState::text_input(
            TextIntent::SaveAttachment {
                attachment_id,
                filename,
                scroll,
            },
            "Save attachment as",
            "destination path:",
            value,
        ));
        if error {
            self.set_error(message);
        } else {
            self.set_warning(message);
        }
    }

    pub(super) async fn submit_delete_attachment(&mut self, attachment_id: String) -> Result<()> {
        let replacement_attachment_id = self.attachment_focus_after_delete(&attachment_id);
        self.store.delete_attachment(&attachment_id).await?;
        if let Some(detail) = self.detail.state_mut() {
            detail.set_focused_target(
                replacement_attachment_id
                    .map(|attachment_id| DetailTargetId::Attachment { attachment_id }),
            );
        }
        self.inline_images.remove_exports_for(&attachment_id);
        self.refresh().await?;
        self.set_success("removed image");
        Ok(())
    }

    fn attachment_focus_after_delete(&self, attachment_id: &str) -> Option<String> {
        let attachment_ids = self
            .store
            .selected_task(self.list.selected_task())?
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

    pub(super) async fn paste_image_from_empty_terminal_paste(&mut self) -> Result<bool> {
        if self.detail_accepts_image_paste() {
            self.paste_detail_image_from_clipboard().await?;
            return Ok(true);
        }
        if self.add_task_accepts_image_paste() {
            self.paste_add_task_image_from_clipboard().await?;
            return Ok(true);
        }
        Ok(false)
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
                    intent: MultilineIntent::AddTaskNatural,
                    ..
                }))
        ) && self.pending_shortcut.is_empty()
            && self.footer_choice.is_none()
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
            self.sync_add_task_attachments();
            if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                state.selected_attachment = state.attachments.len().saturating_sub(1);
            }
            self.set_success("attached image");
        } else {
            self.set_info("image already attached");
        }
        Ok(())
    }

    pub(super) fn remove_selected_add_task_image(&mut self) {
        let Some(index) = self.overlay.as_ref().and_then(|overlay| match overlay {
            OverlayState::AddTask(state) => Some(state.selected_attachment),
            _ => None,
        }) else {
            self.set_info("focus draft images to remove one");
            return;
        };
        let Some(filename) = self.authoring.remove_add_task_attachment(index) else {
            self.set_info("no draft images to remove");
            return;
        };
        self.sync_add_task_attachments();
        self.set_success(format!("removed draft image {filename}"));
    }

    fn sync_add_task_attachments(&mut self) {
        let attachments = self.authoring.add_task_attachment_summaries();
        if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
            state.selected_attachment = state
                .selected_attachment
                .min(attachments.len().saturating_sub(1));
            if attachments.is_empty() && state.focus == crate::tui::authoring::AddTaskStep::Images {
                state.focus = crate::tui::authoring::AddTaskStep::Title;
            }
            state.attachments = attachments;
        }
    }

    fn detail_accepts_image_paste(&self) -> bool {
        self.detail.is_active()
            && self.overlay.is_none()
            && self.pending_shortcut.is_empty()
            && self.footer_choice.is_none()
    }

    async fn attach_clipboard_image(&mut self, image: ClipboardImage) -> Result<()> {
        self.attach_image_source(image.filename, AttachmentSource::Bytes(image.bytes))
    }

    fn attach_image_source(&mut self, filename: String, source: AttachmentSource) -> Result<()> {
        let Some(item) = self.store.selected_task(self.list.selected_task()).cloned() else {
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

fn attachment_default_filename(attachment: &crate::task_render::AttachmentMetadataJson) -> String {
    if let Some(filename) = attachment
        .filename
        .as_deref()
        .and_then(|filename| Path::new(filename).file_name())
        .and_then(|filename| filename.to_str())
        .filter(|filename| !filename.is_empty() && *filename != "." && *filename != "..")
    {
        return filename.to_string();
    }
    let extension = match attachment.media_type.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "image",
    };
    format!("attachment-{}.{}", attachment.attachment_id, extension)
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
