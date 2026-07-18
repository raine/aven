use std::time::{Duration, Instant};

pub(super) const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(120);
pub(super) const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const TOAST_TTL: Duration = Duration::from_secs(4);

use anyhow::Result;
use crossterm::cursor::MoveTo;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event,
};
use crossterm::style::Print;
use crossterm::{execute, queue};
use ratatui::DefaultTerminal;
use std::io::Write;

use crate::config::{AppConfig, resolve_blob_dir};
use crate::tui::app::App;
use crate::tui::inline_images::{
    InlineImageBackend, active_backend_from_env, inline_image_delete_escape, inline_image_escape,
    kitty_image_identifiers,
};
use crate::tui::overlay::OverlayView::AddTask;
use crate::tui::overlay::{OverlayState, OverlayView};
use crate::tui::preview_controller::PreviewKey;
use crate::tui::store::TaskView;
use crate::tui::ui::{self, ViewState, ViewSurface};

fn write_inline_image(
    writer: &mut impl Write,
    placement: &ui::DetailInlineImagePlacement,
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

fn unique_inline_image_placements(
    placements: Vec<ui::DetailInlineImagePlacement>,
) -> Vec<ui::DetailInlineImagePlacement> {
    placements
        .into_iter()
        .fold(Vec::new(), |mut unique, placement| {
            if !unique.contains(&placement) {
                unique.push(placement);
            }
            unique
        })
}

fn inline_image_emissions_after_draw(
    placements: &[ui::DetailInlineImagePlacement],
    previous: &[ui::DetailInlineImagePlacement],
) -> Vec<ui::DetailInlineImagePlacement> {
    placements
        .iter()
        .filter(|placement| !previous.contains(placement))
        .cloned()
        .collect()
}

fn write_inline_image_cleanup(
    writer: &mut impl Write,
    placements: &[ui::DetailInlineImagePlacement],
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

impl App {
    pub(crate) async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
        let result = self.run_loop(terminal).await;
        let _ = self.erase_previous_inline_images();
        execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture
        )?;
        result
    }

    pub(crate) async fn run_add_task_only(
        mut self,
        terminal: &mut DefaultTerminal,
        natural: bool,
        config: AppConfig,
    ) -> Result<Option<String>> {
        self.intake.enter_add_task_only(config);
        self.open_add_task_on_start(natural).await?;
        execute!(std::io::stdout(), EnableBracketedPaste)?;
        let result = self.run_loop(terminal).await;
        let _ = self.erase_previous_inline_images();
        execute!(std::io::stdout(), DisableBracketedPaste)?;
        result.map(|()| self.intake.take_message())
    }

    pub(crate) async fn open_add_task_on_start(&mut self, natural: bool) -> Result<()> {
        self.begin_add_task().await?;
        if natural {
            self.begin_add_task_natural();
        }
        Ok(())
    }

    async fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut needs_redraw = true;
        while !self.should_quit {
            if self.poll_pending_task_intake().await? {
                needs_redraw = true;
            }

            if self.poll_search_preview().await? {
                needs_redraw = true;
            }

            if self.preview_controller.poll() {
                needs_redraw = true;
            }

            if self.poll_update().await {
                needs_redraw = true;
            }

            match self.refresh_if_due().await {
                Ok(true) => needs_redraw = true,
                Ok(false) => {}
                Err(error) => {
                    self.set_error(format!("refresh failed: {error:#}"));
                    needs_redraw = true;
                }
            }

            if self.clear_expired_notification() {
                needs_redraw = true;
            }

            if needs_redraw {
                let view = self.view();
                terminal.draw(|frame| ui::render(frame, &self.store, &mut self.widgets, &view))?;
                needs_redraw = self.render_inline_images_after_draw(terminal).is_err();
            }

            let timeout = self.next_poll_timeout();
            if event::poll(timeout)? {
                needs_redraw = true;
                match event::read()? {
                    Event::Key(key) => {
                        let result = self.dispatch_key(key, terminal.size()?).await;
                        if let Err(error) = result {
                            self.set_error(format!("{error:#}"));
                        }
                        if self.needs_terminal_clear {
                            self.needs_terminal_clear = false;
                            self.preview_controller.set_desired([]);
                            let _ = self.erase_previous_inline_images();
                            terminal.clear()?;
                        }
                    }
                    Event::Paste(text) => {
                        if let Err(error) = self.dispatch_paste(&text).await {
                            self.set_error(format!("{error:#}"));
                        }
                    }
                    Event::Mouse(mouse) => {
                        let result = self.dispatch_mouse(mouse, terminal.size()?).await;
                        if let Err(error) = result {
                            self.set_error(format!("{error:#}"));
                        }
                    }
                    _ => {}
                }
            } else if self.has_time_based_redraw() {
                needs_redraw = true;
            }
        }
        Ok(())
    }

    fn render_inline_images_after_draw(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let backend = active_backend_from_env(self.intake.config().local.inline_images);
        let blob_dir = self
            .intake
            .db_path()
            .map(|db_path| resolve_blob_dir(db_path, self.intake.config()))
            .transpose()?;
        let current = unique_inline_image_placements(
            if backend == InlineImageBackend::None || blob_dir.is_none() {
                Vec::new()
            } else {
                self.widgets.inline_image_placements.clone()
            },
        );
        let preview_quota_bytes = self
            .intake
            .config()
            .local
            .attachment_lifecycle
            .preview_quota_bytes;
        let keys = blob_dir
            .as_deref()
            .into_iter()
            .flat_map(|blob_dir| {
                current.iter().map(move |placement| {
                    PreviewKey::new(blob_dir, &placement.source_hash, preview_quota_bytes)
                })
            })
            .collect::<Vec<_>>();
        self.preview_controller.set_desired(keys);

        let backend_changed = self.previous_inline_image_backend != backend;
        let stale = if backend_changed {
            self.previous_inline_image_placements.clone()
        } else {
            self.previous_inline_image_placements
                .iter()
                .filter(|placement| !current.contains(placement))
                .cloned()
                .collect::<Vec<_>>()
        };
        let repaint =
            self.erase_inline_image_placements(&stale, self.previous_inline_image_backend)?;
        if backend_changed {
            self.previous_inline_image_placements.clear();
        } else {
            self.previous_inline_image_placements
                .retain(|placement| current.contains(placement));
        }
        self.previous_inline_image_backend = backend;

        if repaint {
            let view = self.view();
            terminal.draw(|frame| ui::render(frame, &self.store, &mut self.widgets, &view))?;
        }
        if backend == InlineImageBackend::None {
            return Ok(());
        }
        let Some(blob_dir) = blob_dir else {
            return Ok(());
        };

        let pending_emissions =
            inline_image_emissions_after_draw(&current, &self.previous_inline_image_placements);
        let mut stdout = std::io::stdout();
        for placement in pending_emissions {
            let key = PreviewKey::new(&blob_dir, &placement.source_hash, preview_quota_bytes);
            let Some(lease) = self.preview_controller.lease(&key) else {
                continue;
            };
            if !self.previous_inline_image_placements.contains(&placement) {
                self.previous_inline_image_placements
                    .push(placement.clone());
            }
            let payload = lease.payload();
            if let Err(error) = write_inline_image(
                &mut stdout,
                &placement,
                payload.encoded_png(),
                payload.byte_len(),
                backend,
            ) {
                let repaint = self.erase_previous_inline_images().unwrap_or(false);
                if repaint {
                    let view = self.view();
                    let _ = terminal
                        .draw(|frame| ui::render(frame, &self.store, &mut self.widgets, &view));
                }
                return Err(error.into());
            }
        }
        if let Err(error) = stdout.flush() {
            let repaint = self.erase_previous_inline_images().unwrap_or(false);
            if repaint {
                let view = self.view();
                let _ =
                    terminal.draw(|frame| ui::render(frame, &self.store, &mut self.widgets, &view));
            }
            return Err(error.into());
        }
        Ok(())
    }

    fn erase_previous_inline_images(&mut self) -> Result<bool> {
        if self.previous_inline_image_placements.is_empty() {
            return Ok(false);
        }
        let placements = self.previous_inline_image_placements.clone();
        let repaint =
            self.erase_inline_image_placements(&placements, self.previous_inline_image_backend)?;
        self.previous_inline_image_placements.clear();
        self.previous_inline_image_backend = InlineImageBackend::None;
        Ok(repaint)
    }

    fn erase_inline_image_placements(
        &self,
        placements: &[ui::DetailInlineImagePlacement],
        backend: InlineImageBackend,
    ) -> Result<bool> {
        if placements.is_empty() || backend == InlineImageBackend::None {
            return Ok(false);
        }
        let mut stdout = std::io::stdout();
        write_inline_image_cleanup(&mut stdout, placements, backend)
    }

    pub(crate) fn view(&self) -> ViewState {
        let mut overlay = self.overlay.as_ref().map(OverlayView::from);
        if let Some(AddTask(state)) = &mut overlay {
            state.status_prefix_active = self.pending_shortcut.has_add_task_status_prefix();
            state.priority_prefix_active = self.pending_shortcut.has_add_task_priority_prefix();
        }

        let selected_task = self.store.selected_task(self.widgets.table.selected());
        ViewState {
            focus: self.focus,
            overlay,
            detail_underlay: self.detail_underlay(),
            detail_underlay_scroll: self.detail_context_scroll,
            hovered_detail_child_task_id: self.hovered_detail_child_task_id.clone(),
            selected_detail_child_task_id: self
                .selected_detail_child_task_id
                .as_ref()
                .filter(|task_id| {
                    selected_task.is_some_and(|task| {
                        task.epic_children
                            .iter()
                            .any(|child| &child.task_id == *task_id)
                    })
                })
                .cloned(),
            detail_text_selection: self
                .detail_text_selection
                .as_ref()
                .filter(|selection| {
                    selected_task.is_some_and(|task| task.task.id == selection.task_id)
                })
                .cloned(),
            notification: self
                .notification
                .as_ref()
                .map(|notification| notification.toast_view()),
            pending_shortcut: self.pending_shortcut.labels(),
            pending_shortcut_scroll: self.pending_shortcut_scroll,
            copy_description_available: selected_task
                .is_some_and(|task| !task.task.description.is_empty()),
            copy_notes_available: selected_task.is_some_and(|task| !task.notes.is_empty()),
            footer_choice_mode: self.footer_choice_mode,
            sidebar_visible: self.sidebar_visible,
            update_badge: self.update.badge(),
            surface: if self.intake.view().add_task_only {
                ViewSurface::AddTask
            } else {
                ViewSurface::Main
            },
            inline_images: self.inline_image_context(),
        }
    }

    fn inline_image_context(&self) -> Option<ui::DetailInlineImageContext> {
        if self.intake.view().add_task_only
            || self.notification.is_some()
            || !self.pending_shortcut.is_empty()
            || !self.detail_surface_accepts_inline_images()
        {
            return None;
        }
        let backend = active_backend_from_env(self.intake.config().local.inline_images);
        if backend == InlineImageBackend::None {
            return None;
        }
        let db_path = self.intake.db_path()?;
        let blob_dir = resolve_blob_dir(db_path, self.intake.config()).ok()?;
        Some(ui::DetailInlineImageContext {
            unavailable_hashes: self.preview_controller.suppressed_hashes(&blob_dir),
        })
    }

    fn detail_surface_accepts_inline_images(&self) -> bool {
        matches!(self.overlay, None | Some(OverlayState::Detail { .. }))
    }

    pub(super) fn detail_underlay(&self) -> bool {
        self.detail_context
            || matches!(
                self.overlay,
                Some(OverlayState::Detail { .. } | OverlayState::DetailHelp { .. })
            )
            || self.authoring.detail_underlay()
    }

    pub(super) async fn refresh(&mut self) -> Result<()> {
        let selected = self.widgets.table.selected();
        let recent_action_selection =
            (self.store.view_state.view == TaskView::RecentActions).then(|| {
                (
                    selected,
                    self.store
                        .selected_recent_action(selected)
                        .map(|action| action.change_id.clone()),
                )
            });
        let selected_id = if self.store.view_state.view == TaskView::RecentActions {
            None
        } else {
            self.store
                .selected_task(selected)
                .map(|item| item.task.id.clone())
        };
        let detail_task = self
            .detail_underlay()
            .then(|| self.store.selected_task(selected).cloned())
            .flatten();
        let result = self
            .store
            .refresh_with_scope_fallback(selected_id.as_ref())
            .await?;
        let selected = recent_action_selection
            .map(|(selected, change_id)| {
                self.restored_recent_action_selection(selected, change_id.as_deref())
            })
            .unwrap_or_else(|| {
                if let Some(item) = detail_task
                    && self
                        .store
                        .tasks
                        .iter()
                        .all(|candidate| candidate.task.id != item.task.id)
                {
                    let index = selected
                        .unwrap_or(self.store.tasks.len())
                        .min(self.store.tasks.len());
                    self.store.tasks.insert(index, item);
                    return Some(index);
                }
                result.selected
            });
        self.widgets.table.select(selected);
        self.preserve_or_restore_sidebar_selection();
        self.prune_task_marks();
        if let Some(project) = result.fallback_scope {
            self.set_warning(format!("project scope {project} is no longer available"));
        }
        Ok(())
    }

    fn restored_recent_action_selection(
        &self,
        selected: Option<usize>,
        change_id: Option<&str>,
    ) -> Option<usize> {
        if self.store.recent_actions.is_empty() {
            return None;
        }
        change_id
            .and_then(|id| {
                self.store
                    .recent_actions
                    .iter()
                    .position(|action| action.change_id == id)
            })
            .or_else(|| selected.map(|index| index.min(self.store.recent_actions.len() - 1)))
    }

    pub(super) fn clear_expired_notification(&mut self) -> bool {
        if matches!(
            self.notification,
            Some(crate::tui::app::Notification::Toast { created_at, .. })
                if created_at.elapsed() >= TOAST_TTL
        ) {
            self.notification = None;
            return true;
        }
        false
    }

    pub(super) fn has_time_based_redraw(&self) -> bool {
        self.notification.is_some() || self.refresh_is_due()
    }

    pub(super) fn next_poll_timeout(&self) -> Duration {
        let mut timeout = self.refresh_timeout();

        match &self.notification {
            Some(crate::tui::app::Notification::Toast { created_at, .. }) => {
                timeout = timeout.min(
                    TOAST_TTL
                        .checked_sub(created_at.elapsed())
                        .unwrap_or_default(),
                );
            }
            Some(crate::tui::app::Notification::Loading { .. }) => {
                timeout = timeout.min(INPUT_POLL_INTERVAL);
            }
            None => {}
        }

        if self.intake.work_pending()
            || self.search_preview_work_pending()
            || self.preview_controller.work_pending()
            || self.update.work_pending()
        {
            timeout = timeout.min(INPUT_POLL_INTERVAL);
        }

        timeout
    }

    pub(super) async fn refresh_if_due(&mut self) -> Result<bool> {
        if !self.refresh_is_due() {
            return Ok(false);
        }
        let result = self.refresh().await;
        self.schedule_next_refresh();
        result?;
        Ok(true)
    }

    pub(super) fn refresh_is_due(&self) -> bool {
        Instant::now() >= self.next_refresh_at
    }

    pub(super) fn refresh_timeout(&self) -> Duration {
        self.next_refresh_at
            .checked_duration_since(Instant::now())
            .unwrap_or_default()
    }

    pub(super) fn schedule_next_refresh(&mut self) {
        self.next_refresh_at = Instant::now() + REFRESH_INTERVAL;
    }
}

#[cfg(test)]
mod inline_image_lifecycle_tests {
    use std::io::{self, Write};

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

    fn placement() -> ui::DetailInlineImagePlacement {
        ui::DetailInlineImagePlacement {
            source_hash: "0".repeat(64),
            x: 4,
            y: 7,
            width: 20,
            height: 6,
        }
    }

    #[test]
    fn duplicate_placements_are_emitted_once() {
        let placement = placement();

        assert_eq!(
            unique_inline_image_placements(vec![placement.clone(), placement.clone()]),
            vec![placement]
        );
    }

    #[test]
    fn new_placements_are_emitted_after_frame_draw() {
        let placement = placement();

        assert_eq!(
            inline_image_emissions_after_draw(std::slice::from_ref(&placement), &[]),
            vec![placement]
        );
    }

    #[test]
    fn unchanged_placements_are_not_retransmitted_after_frame_draw() {
        let placement = placement();

        assert!(
            inline_image_emissions_after_draw(
                std::slice::from_ref(&placement),
                std::slice::from_ref(&placement),
            )
            .is_empty()
        );
    }

    #[test]
    fn inline_image_write_failure_is_returned_to_lifecycle_owner() {
        let mut writer = FailingWriter {
            fail_write: true,
            fail_flush: false,
            bytes: Vec::new(),
        };

        assert!(
            write_inline_image(
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
    fn inline_image_flush_failure_is_returned_to_lifecycle_owner() {
        let mut writer = FailingWriter {
            fail_write: false,
            fail_flush: true,
            bytes: Vec::new(),
        };

        assert!(
            write_inline_image_cleanup(&mut writer, &[placement()], InlineImageBackend::Kitty)
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

        let repaint =
            write_inline_image_cleanup(&mut writer, &[placement()], InlineImageBackend::Kitty)
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

        let repaint =
            write_inline_image_cleanup(&mut writer, &[placement()], InlineImageBackend::Iterm2)
                .unwrap();

        assert!(repaint);
        assert!(!writer.bytes.is_empty());
    }
}
