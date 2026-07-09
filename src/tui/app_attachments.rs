use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::attachments::optimization::ImageOptimizationPolicy;
use crate::config::resolve_blob_dir;
use crate::ids::new_id;
use crate::operations::{AttachmentAddInput, append_attachment_ref};
use crate::tui::app::App;
use crate::tui::authoring::PendingTaskAttachment;
use crate::tui::overlay::{MultilineInputState, OverlayRoute, OverlayState};
use crate::tui::platform::{ClipboardImage, read_clipboard_image, read_clipboard_text};

const IMAGE_EXTENSIONS: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

impl App {
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
        let media_type = infer_image_media_type(&path)?;
        let bytes = std::fs::read(&path)?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("pasted-image")
            .to_string();
        self.attach_image_bytes(filename, media_type, bytes).await?;
        Ok(true)
    }

    pub(super) async fn paste_add_task_image_from_clipboard(&mut self) -> Result<()> {
        if !self.add_task_accepts_image_paste() {
            self.set_info("open add task composer to attach an image");
            return Ok(());
        }
        match read_clipboard_image()? {
            Some(image) => {
                self.attach_add_task_image_bytes(image.filename, image.media_type, image.bytes)?
            }
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
        let media_type = infer_image_media_type(&path)?;
        let bytes = std::fs::read(&path)?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("pasted-image")
            .to_string();
        self.attach_add_task_image_bytes(filename, media_type, bytes)?;
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

    fn attach_add_task_image_bytes(
        &mut self,
        filename: String,
        media_type: String,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let pending = PendingTaskAttachment::new(
            new_id(),
            AttachmentAddInput {
                filename: Some(filename),
                alt_text: Some("pasted image".to_string()),
                media_type,
                width: None,
                height: None,
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
        let Some((ref_text, is_new)) = self.authoring.add_pending_add_task_attachment(pending)
        else {
            self.set_info("open add task composer to attach an image");
            return Ok(());
        };
        let inserted = self.insert_add_task_attachment_ref(&ref_text);
        if inserted {
            let state = self.overlay.as_ref().cloned();
            if let Some(OverlayState::AddTask(state)) = state.as_ref() {
                self.capture_add_task_state(state);
            }
            self.set_success("attached image");
        } else if is_new {
            self.set_success("attached image");
        } else {
            self.set_info("image already attached");
        }
        Ok(())
    }

    fn insert_add_task_attachment_ref(&mut self, ref_text: &str) -> bool {
        let Some(overlay) = self.overlay.as_mut() else {
            return false;
        };
        match overlay {
            OverlayState::AddTask(state) => {
                let description = state.description.lines.join("\n");
                if description.contains(ref_text) {
                    return false;
                }
                let description = append_attachment_ref(&description, ref_text);
                state.description = MultilineInputState::from_value(
                    OverlayRoute::AddTaskDescription,
                    "Add task: description",
                    "",
                    description,
                );
                true
            }
            OverlayState::MultilineInput(state) if state.route == OverlayRoute::AddTaskNatural => {
                let value = state.lines.join("\n");
                if value.contains(ref_text) {
                    return false;
                }
                let value = append_attachment_ref(&value, ref_text);
                *state = MultilineInputState::from_value(
                    OverlayRoute::AddTaskNatural,
                    state.title.clone(),
                    state.prompt.clone(),
                    value,
                );
                true
            }
            _ => false,
        }
    }

    fn detail_accepts_image_paste(&self) -> bool {
        matches!(self.overlay, Some(OverlayState::Detail { .. }))
            && self.pending_shortcut.is_empty()
            && self.footer_choice_mode.is_none()
    }

    async fn attach_clipboard_image(&mut self, image: ClipboardImage) -> Result<()> {
        self.attach_image_bytes(image.filename, image.media_type, image.bytes)
            .await
    }

    async fn attach_image_bytes(
        &mut self,
        filename: String,
        media_type: String,
        bytes: Vec<u8>,
    ) -> Result<()> {
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
        let result = self
            .store
            .add_attachment(
                self.widgets.table.selected(),
                &blob_dir,
                AttachmentAddInput {
                    filename: Some(filename),
                    alt_text: Some("pasted image".to_string()),
                    media_type,
                    width: None,
                    height: None,
                    bytes,
                    optimization_policy,
                    dedupe_existing: true,
                },
            )
            .await?;
        if let Some(result) = result {
            self.apply_mutation_result(result);
            if self.overlay.is_none() {
                self.overlay = Some(OverlayState::Detail { scroll: 0 });
            }
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }
}

fn infer_image_media_type(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let Some(ext) = ext else {
        bail!("unsupported image type");
    };
    for (key, mime) in IMAGE_EXTENSIONS {
        if *key == ext {
            return Ok((*mime).to_string());
        }
    }
    bail!("unsupported image type")
}

fn pasted_image_path(text: &str) -> Option<PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.lines().count() != 1 {
        return None;
    }
    let trimmed = trimmed
        .strip_prefix("file://")
        .unwrap_or(trimmed)
        .trim_matches('"')
        .trim_matches('\'');
    let path = PathBuf::from(trimmed);
    if !path.is_file() || infer_image_media_type(&path).is_err() {
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
    fn pasted_image_path_ignores_plain_text() {
        assert_eq!(pasted_image_path("not an image"), None);
    }
}
