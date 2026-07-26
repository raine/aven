use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Print;

use crate::tui::inline_images::{
    InlineImageBackend, inline_image_delete_escape, inline_image_escape, kitty_image_identifiers,
};
use crate::tui::ui::DetailInlineImagePlacement;

const EMISSION_DELAY: Duration = Duration::from_millis(50);
const MAX_EXTERNAL_EXPORTS: usize = 8;

type ImageViewerLauncher = fn(&Path) -> Result<()>;

#[cfg(not(test))]
fn default_image_viewer_launcher() -> ImageViewerLauncher {
    crate::tui::platform::open_image_in_default_viewer
}

#[cfg(test)]
fn default_image_viewer_launcher() -> ImageViewerLauncher {
    |_| Ok(())
}

pub(crate) struct InlineImageSurface {
    displayed_placements: Vec<DetailInlineImagePlacement>,
    displayed_backend: InlineImageBackend,
    deferred_placements: Vec<DetailInlineImagePlacement>,
    emission_at: Option<Instant>,
    image_viewer_launcher: ImageViewerLauncher,
    external_exports: Vec<(String, tempfile::TempDir)>,
    #[cfg(test)]
    context_override: Option<crate::tui::ui::DetailInlineImageContext>,
}

impl InlineImageSurface {
    pub(crate) fn new() -> Self {
        Self {
            displayed_placements: Vec::new(),
            displayed_backend: InlineImageBackend::None,
            deferred_placements: Vec::new(),
            emission_at: None,
            image_viewer_launcher: default_image_viewer_launcher(),
            external_exports: Vec::new(),
            #[cfg(test)]
            context_override: None,
        }
    }

    pub(crate) fn unique_placements(
        placements: Vec<DetailInlineImagePlacement>,
    ) -> Vec<DetailInlineImagePlacement> {
        placements
            .into_iter()
            .fold(Vec::new(), |mut unique, placement| {
                if !unique.contains(&placement) {
                    unique.push(placement);
                }
                unique
            })
    }

    pub(crate) fn stale_placements(
        &self,
        current: &[DetailInlineImagePlacement],
        backend: InlineImageBackend,
    ) -> Vec<DetailInlineImagePlacement> {
        if self.displayed_backend != backend {
            return self.displayed_placements.clone();
        }
        self.displayed_placements
            .iter()
            .filter(|placement| !current.contains(placement))
            .cloned()
            .collect()
    }

    pub(crate) fn displayed_backend(&self) -> InlineImageBackend {
        self.displayed_backend
    }

    pub(crate) fn reconcile_displayed(
        &mut self,
        current: &[DetailInlineImagePlacement],
        backend: InlineImageBackend,
    ) {
        if self.displayed_backend != backend {
            self.displayed_placements.clear();
        } else {
            self.displayed_placements
                .retain(|placement| current.contains(placement));
        }
        self.displayed_backend = backend;
    }

    pub(crate) fn pending_emissions(
        &self,
        current: &[DetailInlineImagePlacement],
    ) -> Vec<DetailInlineImagePlacement> {
        current
            .iter()
            .filter(|placement| !self.displayed_placements.contains(placement))
            .cloned()
            .collect()
    }

    pub(crate) fn emission_is_deferred(
        &mut self,
        current: &[DetailInlineImagePlacement],
        now: Instant,
    ) -> bool {
        if current.is_empty() {
            self.cancel_deferred_emission();
            return false;
        }
        if current != self.deferred_placements {
            current.clone_into(&mut self.deferred_placements);
            self.emission_at = Some(now + EMISSION_DELAY);
            return true;
        }
        if self.emission_at.is_some_and(|deadline| now < deadline) {
            return true;
        }
        self.cancel_deferred_emission();
        false
    }

    pub(crate) fn cancel_deferred_emission(&mut self) {
        self.deferred_placements.clear();
        self.emission_at = None;
    }

    pub(crate) fn emission_at(&self) -> Option<Instant> {
        self.emission_at
    }

    pub(crate) fn record_emitted(&mut self, placement: DetailInlineImagePlacement) {
        if !self.displayed_placements.contains(&placement) {
            self.displayed_placements.push(placement);
        }
    }

    pub(crate) fn displayed_placements(&self) -> &[DetailInlineImagePlacement] {
        &self.displayed_placements
    }

    pub(crate) fn clear_displayed(&mut self) {
        self.cancel_deferred_emission();
        self.displayed_placements.clear();
        self.displayed_backend = InlineImageBackend::None;
    }

    pub(crate) fn launch_external_viewer(&self, path: &Path) -> Result<()> {
        (self.image_viewer_launcher)(path)
    }

    pub(crate) fn retain_export(&mut self, attachment_id: String, directory: tempfile::TempDir) {
        self.remove_exports_for(&attachment_id);
        if self.external_exports.len() >= MAX_EXTERNAL_EXPORTS {
            self.external_exports.remove(0);
        }
        self.external_exports.push((attachment_id, directory));
    }

    pub(crate) fn remove_exports_for(&mut self, attachment_id: &str) {
        self.external_exports
            .retain(|(retained_id, _)| retained_id != attachment_id);
    }

    pub(crate) fn write_placement(
        writer: &mut impl Write,
        placement: &DetailInlineImagePlacement,
        encoded_png: &str,
        byte_len: usize,
        backend: InlineImageBackend,
    ) -> std::io::Result<()> {
        queue!(writer, MoveTo(placement.x, placement.y))?;
        let kitty_ids = kitty_image_identifiers(
            &placement.source_hash,
            placement.x,
            placement.y,
            placement.width,
            placement.height,
        );
        let escape = inline_image_escape(
            encoded_png,
            byte_len,
            placement.width,
            placement.height,
            backend,
            kitty_ids,
        );
        write!(writer, "{escape}")
    }

    pub(crate) fn write_cleanup(
        writer: &mut impl Write,
        placements: &[DetailInlineImagePlacement],
        backend: InlineImageBackend,
    ) -> Result<bool> {
        let mut repaint = false;
        for placement in placements {
            let kitty_ids = kitty_image_identifiers(
                &placement.source_hash,
                placement.x,
                placement.y,
                placement.width,
                placement.height,
            );
            if let Some(escape) = inline_image_delete_escape(kitty_ids, backend) {
                write!(writer, "{escape}")?;
                continue;
            }
            repaint = true;
            let blank = " ".repeat(placement.width as usize);
            for row in 0..placement.height {
                queue!(
                    writer,
                    MoveTo(placement.x, placement.y.saturating_add(row)),
                    Print(&blank)
                )?;
            }
        }
        writer.flush()?;
        Ok(repaint)
    }

    #[cfg(test)]
    pub(crate) fn set_context_override(
        &mut self,
        context: crate::tui::ui::DetailInlineImageContext,
    ) {
        self.context_override = Some(context);
    }

    #[cfg(test)]
    pub(crate) fn context_override(&self) -> Option<&crate::tui::ui::DetailInlineImageContext> {
        self.context_override.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_image_viewer_launcher(&mut self, launcher: ImageViewerLauncher) {
        self.image_viewer_launcher = launcher;
    }

    #[cfg(test)]
    pub(crate) fn export_count(&self) -> usize {
        self.external_exports.len()
    }

    #[cfg(test)]
    pub(crate) fn export_directory(&self, index: usize) -> &Path {
        self.external_exports[index].1.path()
    }

    #[cfg(test)]
    pub(crate) fn clear_exports(&mut self) {
        self.external_exports.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    struct FailingWriter {
        fail_write: bool,
        fail_flush: bool,
        bytes: Vec<u8>,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                return Err(io::Error::other("injected write failure"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                return Err(io::Error::other("injected flush failure"));
            }
            Ok(())
        }
    }

    fn placement() -> DetailInlineImagePlacement {
        DetailInlineImagePlacement {
            attachment_id: "ATTACHMENT000001".to_string(),
            source_hash: "0".repeat(64),
            x: 4,
            y: 7,
            width: 20,
            height: 6,
        }
    }

    #[test]
    fn changed_placements_wait_for_scroll_to_settle() {
        let first = placement();
        let mut second = first.clone();
        second.y += 1;
        let start = Instant::now();
        let mut surface = InlineImageSurface::new();

        assert!(surface.emission_is_deferred(std::slice::from_ref(&first), start));
        assert!(
            surface.emission_is_deferred(std::slice::from_ref(&first), start + EMISSION_DELAY / 2)
        );
        assert!(
            surface.emission_is_deferred(std::slice::from_ref(&second), start + EMISSION_DELAY)
        );
        assert!(surface.emission_is_deferred(
            std::slice::from_ref(&second),
            start + EMISSION_DELAY * 3 / 2
        ));
        assert!(
            !surface
                .emission_is_deferred(std::slice::from_ref(&second), start + EMISSION_DELAY * 2)
        );
        assert!(surface.emission_at().is_none());
    }

    #[test]
    fn empty_view_cancels_deferred_emission() {
        let start = Instant::now();
        let mut surface = InlineImageSurface::new();
        assert!(surface.emission_is_deferred(std::slice::from_ref(&placement()), start));

        assert!(!surface.emission_is_deferred(&[], start));
        assert!(surface.emission_at().is_none());
    }

    #[test]
    fn backend_change_reconciles_all_displayed_placements() {
        let placement = placement();
        let mut surface = InlineImageSurface::new();
        surface.reconcile_displayed(&[], InlineImageBackend::Kitty);
        surface.record_emitted(placement.clone());

        assert_eq!(
            surface.stale_placements(std::slice::from_ref(&placement), InlineImageBackend::Iterm2),
            vec![placement]
        );
        surface.reconcile_displayed(&[], InlineImageBackend::Iterm2);
        assert!(surface.displayed_placements().is_empty());
        assert_eq!(surface.displayed_backend(), InlineImageBackend::Iterm2);
    }

    #[test]
    fn pending_emissions_exclude_displayed_placements() {
        let placement = placement();
        let mut surface = InlineImageSurface::new();
        assert_eq!(
            surface.pending_emissions(std::slice::from_ref(&placement)),
            vec![placement.clone()]
        );

        surface.record_emitted(placement.clone());
        assert!(
            surface
                .pending_emissions(std::slice::from_ref(&placement))
                .is_empty()
        );
    }

    #[test]
    fn write_failure_is_returned_to_lifecycle_owner() {
        let mut writer = FailingWriter {
            fail_write: true,
            fail_flush: false,
            bytes: Vec::new(),
        };

        assert!(
            InlineImageSurface::write_placement(
                &mut writer,
                &placement(),
                "cG5n",
                3,
                InlineImageBackend::Kitty,
            )
            .is_err()
        );
    }

    #[test]
    fn cleanup_flush_failure_is_returned_to_lifecycle_owner() {
        let mut writer = FailingWriter {
            fail_write: false,
            fail_flush: true,
            bytes: Vec::new(),
        };

        assert!(
            InlineImageSurface::write_cleanup(
                &mut writer,
                &[placement()],
                InlineImageBackend::Kitty,
            )
            .is_err()
        );
    }

    #[test]
    fn kitty_cleanup_deletes_owned_placement_without_repaint() {
        let mut writer = FailingWriter {
            fail_write: false,
            fail_flush: false,
            bytes: Vec::new(),
        };

        let repaint = InlineImageSurface::write_cleanup(
            &mut writer,
            &[placement()],
            InlineImageBackend::Kitty,
        )
        .unwrap();

        assert!(!repaint);
        let placement = placement();
        let (image_number, placement_id) = kitty_image_identifiers(
            &placement.source_hash,
            placement.x,
            placement.y,
            placement.width,
            placement.height,
        );
        assert_eq!(
            String::from_utf8(writer.bytes).unwrap(),
            format!("\x1b_Ga=d,d=N,q=2,I={image_number},p={placement_id}\x1b\\")
        );
    }

    #[test]
    fn iterm_cleanup_repaints_reserved_cells() {
        let mut writer = FailingWriter {
            fail_write: false,
            fail_flush: false,
            bytes: Vec::new(),
        };

        let repaint = InlineImageSurface::write_cleanup(
            &mut writer,
            &[placement()],
            InlineImageBackend::Iterm2,
        )
        .unwrap();

        assert!(repaint);
        assert!(!writer.bytes.is_empty());
    }

    #[test]
    fn retained_exports_replace_attachment_and_obey_capacity() {
        let mut surface = InlineImageSurface::new();
        for index in 0..=MAX_EXTERNAL_EXPORTS {
            surface.retain_export(index.to_string(), tempfile::tempdir().unwrap());
        }
        assert_eq!(surface.export_count(), MAX_EXTERNAL_EXPORTS);

        surface.retain_export(
            MAX_EXTERNAL_EXPORTS.to_string(),
            tempfile::tempdir().unwrap(),
        );
        assert_eq!(surface.export_count(), MAX_EXTERNAL_EXPORTS);
    }
}
