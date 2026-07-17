use std::time::{Duration, Instant};

pub(super) const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(120);
pub(super) const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const TOAST_TTL: Duration = Duration::from_secs(4);

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event,
};
use crossterm::execute;
use ratatui::DefaultTerminal;

use crate::config::AppConfig;
use crate::tui::app::App;
use crate::tui::overlay::OverlayView::AddTask;
use crate::tui::overlay::{OverlayState, OverlayView};
use crate::tui::store::TaskView;
use crate::tui::ui::{self, ViewState, ViewSurface};

impl App {
    pub(crate) async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
        let result = self.run_loop(terminal).await;
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
                needs_redraw = false;
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
        }
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
