use std::time::{Duration, Instant};

pub(super) const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(120);
pub(super) const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const TOAST_TTL: Duration = Duration::from_secs(4);

#[derive(Clone, Copy)]
enum RefreshKind {
    User,
    Background,
}

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, MouseEventKind,
};
use crossterm::execute;
use ratatui::DefaultTerminal;
use std::io::Write;

use crate::config::resolve_blob_dir;
use crate::tui::app::App;
use crate::tui::inline_image_surface::InlineImageSurface;
use crate::tui::inline_images::InlineImageBackend;
use crate::tui::overlay::OverlayView::AddTask;
use crate::tui::overlay::{OverlayState, OverlayView};
use crate::tui::preview_controller::PreviewKey;
use crate::tui::store::{MainRowSelection, TaskView};
use crate::tui::ui::{self, ViewState, ViewSurface};

fn event_after_queued_wheel(direction: MouseEventKind) -> Result<Option<Event>> {
    while event::poll(Duration::ZERO)? {
        let queued = event::read()?;
        if matches!(&queued, Event::Mouse(mouse) if mouse.kind == direction) {
            continue;
        }
        return Ok(Some(queued));
    }
    Ok(None)
}

impl App {
    pub(crate) async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
        let result = self.run_loop(terminal).await;
        self.attachment_controller.shutdown().await;
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
    ) -> Result<Option<String>> {
        self.intake.enter_add_task_only();
        self.open_add_task_on_start(natural).await?;
        execute!(std::io::stdout(), EnableBracketedPaste)?;
        let result = self.run_loop(terminal).await;
        self.attachment_controller.shutdown().await;
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
        let mut pending_event = None;
        while !self.should_quit {
            if self.finish_onboarding_intro_if_elapsed() {
                needs_redraw = true;
            }

            if self.poll_pending_task_intake().await? {
                needs_redraw = true;
            }

            if self.poll_sync().await? {
                needs_redraw = true;
            }

            if self.poll_search_preview().await? {
                needs_redraw = true;
            }

            if self.poll_attachment_work().await? {
                needs_redraw = true;
            }

            if self.preview_controller.poll() {
                needs_redraw = true;
            }

            if self.poll_update().await {
                needs_redraw = true;
            }

            if self.poll_changelog().await {
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
                terminal.draw(|frame| {
                    ui::render(frame, &self.store, &mut self.widgets, &mut self.list, &view)
                })?;
                needs_redraw = self.render_inline_images_after_draw(terminal).is_err();
            }

            let timeout = self.next_poll_timeout();
            if pending_event.is_some() || event::poll(timeout)? {
                let input = if let Some(input) = pending_event.take() {
                    input
                } else {
                    event::read()?
                };
                match input {
                    Event::Key(key) => {
                        needs_redraw = true;
                        if self.skip_onboarding_intro() {
                            continue;
                        }
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
                        needs_redraw = true;
                        if let Err(error) = self.dispatch_paste(&text).await {
                            self.set_error(format!("{error:#}"));
                        }
                    }
                    Event::Mouse(mouse) => {
                        match self.dispatch_mouse(mouse, terminal.size()?).await {
                            Ok(changed) => {
                                needs_redraw |= changed;
                                if !changed
                                    && matches!(
                                        mouse.kind,
                                        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                                    )
                                {
                                    pending_event = event_after_queued_wheel(mouse.kind)?;
                                }
                            }
                            Err(error) => {
                                self.set_error(format!("{error:#}"));
                                needs_redraw = true;
                            }
                        }
                    }
                    _ => needs_redraw = true,
                }
            } else if self.has_time_based_redraw() {
                needs_redraw = true;
            }
        }
        Ok(())
    }

    fn render_inline_images_after_draw(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        if self.widgets.inline_image_placements.is_empty()
            && self.inline_images.displayed_placements().is_empty()
            && !self.preview_controller.has_desired()
        {
            self.inline_images.cancel_deferred_emission();
            return Ok(());
        }
        let backend = self.inline_image_backend;
        let blob_dir = self
            .intake
            .db_path()
            .map(|db_path| resolve_blob_dir(db_path, self.intake.config()))
            .transpose()?;
        let current = InlineImageSurface::unique_placements(
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

        let previous_backend = self.inline_images.displayed_backend();
        let stale = self.inline_images.stale_placements(&current, backend);
        let repaint = self.erase_inline_image_placements(&stale, previous_backend)?;
        self.inline_images.reconcile_displayed(&current, backend);

        if repaint {
            let view = self.view();
            terminal.draw(|frame| {
                ui::render(frame, &self.store, &mut self.widgets, &mut self.list, &view)
            })?;
        }
        if backend == InlineImageBackend::None {
            return Ok(());
        }
        let Some(blob_dir) = blob_dir else {
            return Ok(());
        };

        let pending_emissions = self.inline_images.pending_emissions(&current);
        if pending_emissions.is_empty() {
            if current.is_empty() {
                self.inline_images.cancel_deferred_emission();
            }
            return Ok(());
        }
        if self
            .inline_images
            .emission_is_deferred(&current, Instant::now())
        {
            return Ok(());
        }
        let mut stdout = std::io::stdout();
        for placement in pending_emissions {
            let key = PreviewKey::new(&blob_dir, &placement.source_hash, preview_quota_bytes);
            let Some(lease) = self.preview_controller.lease(&key) else {
                continue;
            };
            self.inline_images.record_emitted(placement.clone());
            let payload = lease.payload();
            if let Err(error) = InlineImageSurface::write_placement(
                &mut stdout,
                &placement,
                payload.encoded_png(),
                payload.byte_len(),
                backend,
            ) {
                let repaint = self.erase_previous_inline_images().unwrap_or(false);
                if repaint {
                    let view = self.view();
                    let _ = terminal.draw(|frame| {
                        ui::render(frame, &self.store, &mut self.widgets, &mut self.list, &view)
                    });
                }
                return Err(error.into());
            }
        }
        if let Err(error) = stdout.flush() {
            let repaint = self.erase_previous_inline_images().unwrap_or(false);
            if repaint {
                let view = self.view();
                let _ = terminal.draw(|frame| {
                    ui::render(frame, &self.store, &mut self.widgets, &mut self.list, &view)
                });
            }
            return Err(error.into());
        }
        Ok(())
    }

    fn erase_previous_inline_images(&mut self) -> Result<bool> {
        let placements = self.inline_images.displayed_placements().to_vec();
        if placements.is_empty() {
            self.inline_images.clear_displayed();
            return Ok(false);
        }
        let repaint = self
            .erase_inline_image_placements(&placements, self.inline_images.displayed_backend())?;
        self.inline_images.clear_displayed();
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
        InlineImageSurface::write_cleanup(&mut stdout, placements, backend)
    }

    pub(crate) fn view(&self) -> ViewState {
        let detail = self.detail.state();
        let mut overlay = self.overlay.as_ref().map(OverlayView::from);
        if let Some(AddTask(state)) = &mut overlay {
            state.status_prefix_active = self.pending_shortcut.has_add_task_status_prefix();
            state.priority_prefix_active = self.pending_shortcut.has_add_task_priority_prefix();
        }

        let selected_task = self.store.selected_task(self.list.selected_task());
        let detail_focus = detail.and_then(|detail| detail.focused_target()).filter(|focused| {
            selected_task.is_some_and(|item| {
                ui::detail_target_is_actionable(item, focused)
                    || matches!(
                        focused,
                        crate::tui::app::DetailTargetId::Task {
                            section: crate::tui::app::DetailSection::EpicChildren,
                            task_id,
                        } if detail.and_then(|detail| detail.removed_epic_child()).is_some_and(|removed| {
                            removed.epic_id == item.task.id && removed.child.task_id == *task_id
                        })
                    )
            })
        });
        let inline_images = self.inline_image_context();
        ViewState {
            focus: self.list.focus(),
            overlay,
            onboarding_intro: self.onboarding_intro_visual(),
            detail_underlay: self.detail_underlay(),
            detail_underlay_scroll: detail.map_or(0, |detail| detail.scroll()),
            detail_has_parent: self.detail_has_parent(),
            detail_focus: detail_focus.cloned(),
            detail_hover: detail.and_then(|detail| detail.hovered_target()).cloned(),
            detail_expanded_sections: detail
                .map(|detail| detail.expanded_sections().clone())
                .unwrap_or_default(),
            removed_epic_child: detail
                .and_then(|detail| detail.removed_epic_child())
                .cloned(),
            detail_text_selection: detail
                .and_then(|detail| detail.text_selection())
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
            marked_task_count: self.marked_task_ids_in_view().len(),
            footer_choice_mode: self.footer_choice.as_ref().map(|choice| choice.mode),
            sidebar_visible: self.list.sidebar_visible(),
            update_badge: self.update.badge(),
            surface: if self.intake.view().add_task_only {
                ViewSurface::AddTask
            } else {
                ViewSurface::Main
            },
            inline_images,
            pending_attachments: self.attachment_controller.views(),
        }
    }

    pub(super) fn inline_image_context(&self) -> Option<ui::DetailInlineImageContext> {
        if self.intake.view().add_task_only || !self.detail_surface_accepts_inline_images() {
            return None;
        }
        if !self.pending_shortcut.is_empty() {
            return Some(ui::DetailInlineImageContext {
                previews_enabled: false,
                unavailable_hashes: Default::default(),
                focused_attachment_id: self
                    .detail
                    .state()
                    .and_then(|detail| detail.focused_target())
                    .and_then(crate::tui::app::DetailTargetId::attachment_id)
                    .map(str::to_string),
            });
        }
        #[cfg(test)]
        if let Some(context) = self.inline_images.context_override() {
            return Some(context.clone());
        }
        let backend = self.inline_image_backend;
        if backend == InlineImageBackend::None {
            return Some(ui::DetailInlineImageContext {
                previews_enabled: false,
                unavailable_hashes: Default::default(),
                focused_attachment_id: self
                    .detail
                    .state()
                    .and_then(|detail| detail.focused_target())
                    .and_then(crate::tui::app::DetailTargetId::attachment_id)
                    .map(str::to_string),
            });
        }
        let db_path = self.intake.db_path()?;
        let blob_dir = resolve_blob_dir(db_path, self.intake.config()).ok()?;
        Some(ui::DetailInlineImageContext {
            previews_enabled: true,
            unavailable_hashes: self.preview_controller.suppressed_hashes(&blob_dir),
            focused_attachment_id: self
                .detail
                .state()
                .and_then(|detail| detail.focused_target())
                .and_then(crate::tui::app::DetailTargetId::attachment_id)
                .map(str::to_string),
        })
    }

    fn detail_surface_accepts_inline_images(&self) -> bool {
        matches!(
            self.overlay,
            None | Some(OverlayState::AttachmentPreview { .. })
        )
    }

    pub(super) fn detail_underlay(&self) -> bool {
        self.detail.is_active()
            && !matches!(self.overlay, Some(OverlayState::AttachmentPreview { .. }))
    }

    pub(super) async fn refresh(&mut self) -> Result<()> {
        self.refresh_view(RefreshKind::User).await
    }

    async fn refresh_view(&mut self, kind: RefreshKind) -> Result<()> {
        let selected = self.list.selected_task();
        let recent_action_selection =
            (self.store.view_state.view == TaskView::RecentActions).then(|| {
                (
                    selected,
                    self.store
                        .selected_recent_action(selected)
                        .map(|action| action.change_id.clone()),
                )
            });
        let selected_id = match self.store.view_state.view {
            TaskView::RecentActions => None,
            TaskView::Recurring => self
                .store
                .selected_recurrence_series(selected)
                .map(|item| MainRowSelection::RecurrenceSeries(item.series.id.clone())),
            _ => self
                .store
                .selected_task(selected)
                .map(|item| MainRowSelection::Task(item.task.id.clone())),
        };
        let recurrence_detail_id = (self.store.view_state.view == TaskView::Recurring
            && self.detail.is_active())
        .then(|| {
            self.store
                .recurrence_detail
                .as_ref()
                .map(|detail| detail.series.id.clone())
        })
        .flatten();
        let detail_task = (self.detail_underlay()
            || matches!(self.overlay, Some(OverlayState::AttachmentPreview { .. })))
        .then(|| self.store.selected_task(selected).cloned())
        .flatten();
        let previous_detail_targets = detail_task
            .as_ref()
            .map(|_| self.detail_focus_targets(ratatui::layout::Size::new(80, 24)))
            .unwrap_or_default();
        let result = match kind {
            RefreshKind::User => {
                self.store
                    .refresh_with_scope_fallback(selected_id.as_ref())
                    .await?
            }
            RefreshKind::Background => {
                self.store
                    .refresh_preserving_visible_deleted(selected_id.as_ref())
                    .await?
            }
        };
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
        self.list.select_task(selected);
        if let Some(series_id) = recurrence_detail_id {
            let selected_matches = self
                .store
                .selected_recurrence_series(selected)
                .is_some_and(|item| item.series.id == series_id);
            if selected_matches {
                let detail_matches = self
                    .store
                    .recurrence_detail
                    .as_ref()
                    .is_some_and(|detail| detail.series.id == series_id);
                if !detail_matches {
                    self.store.load_recurrence_series_detail(&series_id).await?;
                }
            } else {
                self.store.recurrence_detail = None;
                self.detail.close();
                self.overlay = None;
                self.set_warning("recurring series is no longer visible");
            }
        }
        let removed_epic_child = self
            .detail
            .state()
            .and_then(|detail| detail.removed_epic_child());
        if let Some(removed) = removed_epic_child
            && self.store.tasks.iter().any(|item| {
                item.task.id == removed.epic_id
                    && item
                        .epic_children
                        .iter()
                        .any(|child| child.task_id == removed.child.task_id)
            })
            && let Some(detail) = self.detail.state_mut()
        {
            detail.set_removed_epic_child(None);
        }
        self.reconcile_detail_focus(&previous_detail_targets);
        self.preserve_or_restore_sidebar_selection();
        self.prune_task_marks();
        if let Some(project) = result.fallback_scope {
            self.set_warning(format!("project scope {project} is no longer available"));
        }
        Ok(())
    }

    fn reconcile_detail_focus(&mut self, previous_targets: &[crate::tui::app::DetailTargetId]) {
        let Some(focused) = self
            .detail
            .state()
            .and_then(|detail| detail.focused_target().cloned())
        else {
            return;
        };
        let Some(_item) = self.store.selected_task(self.list.selected_task()) else {
            if let Some(detail) = self.detail.state_mut() {
                detail.set_focused_target(None);
            }
            return;
        };
        let targets = self.detail_focus_targets(ratatui::layout::Size::new(80, 24));
        if targets.contains(&focused) {
            return;
        }
        let section = focused.section();
        let prior_section_index = previous_targets
            .iter()
            .filter(|target| target.section() == section)
            .position(|target| target == &focused)
            .unwrap_or(0);
        let same_section = targets
            .iter()
            .filter(|target| target.section() == section)
            .collect::<Vec<_>>();
        if !same_section.is_empty() {
            if let Some(detail) = self.detail.state_mut() {
                detail.set_focused_target(Some(
                    (*same_section[prior_section_index.min(same_section.len() - 1)]).clone(),
                ));
            }
            return;
        }
        let previous_index = previous_targets
            .iter()
            .position(|target| target == &focused)
            .unwrap_or(0);
        let replacement = previous_targets
            .iter()
            .skip(previous_index.saturating_add(1))
            .map(crate::tui::app::DetailTargetId::section)
            .find_map(|candidate_section| {
                targets
                    .iter()
                    .find(|target| target.section() == candidate_section)
                    .cloned()
            })
            .or_else(|| targets.first().cloned());
        if let Some(detail) = self.detail.state_mut() {
            detail.set_focused_target(replacement);
        }
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
        self.notification.is_some()
            || self.refresh_is_due()
            || self.onboarding_intro.is_some()
            || self
                .inline_images
                .emission_at()
                .is_some_and(|deadline| Instant::now() >= deadline)
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

        if let Some(intro_timeout) = self.onboarding_intro_timeout() {
            timeout = timeout.min(intro_timeout);
        }

        if let Some(emission_at) = self.inline_images.emission_at() {
            timeout = timeout.min(
                emission_at
                    .checked_duration_since(Instant::now())
                    .unwrap_or_default(),
            );
        }

        if self.intake.work_pending()
            || self.search_preview_work_pending()
            || self.preview_controller.work_pending()
            || self.attachment_controller.work_pending()
            || self.sync.work_pending()
            || self.update.work_pending()
            || self.changelog.work_pending()
        {
            timeout = timeout.min(INPUT_POLL_INTERVAL);
        }

        timeout
    }

    pub(super) async fn refresh_if_due(&mut self) -> Result<bool> {
        if !self.refresh_is_due() {
            return Ok(false);
        }
        let result = self.refresh_view(RefreshKind::Background).await;
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
