use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event};
use crossterm::execute;
use ratatui::DefaultTerminal;

use crate::config::AppConfig;
use crate::tui::app::App;
use crate::tui::overlay::OverlayView::AddTask;
use crate::tui::overlay::{OverlayState, OverlayView};
use crate::tui::ui::{self, ViewState, ViewSurface};

impl App {
    pub(crate) async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        execute!(std::io::stdout(), EnableBracketedPaste)?;
        let result = self.run_loop(terminal).await;
        execute!(std::io::stdout(), DisableBracketedPaste)?;
        result
    }

    pub(crate) async fn run_add_task_only(
        mut self,
        terminal: &mut DefaultTerminal,
        natural: bool,
        config: AppConfig,
    ) -> Result<Option<String>> {
        self.add_task_only = true;
        self.add_task_config = config;
        self.open_add_task_on_start(natural).await?;
        execute!(std::io::stdout(), EnableBracketedPaste)?;
        let result = self.run_loop(terminal).await;
        execute!(std::io::stdout(), DisableBracketedPaste)?;
        result.map(|()| self.add_task_only_message)
    }

    pub(crate) async fn open_add_task_on_start(&mut self, natural: bool) -> Result<()> {
        self.begin_add_task().await?;
        if natural {
            self.begin_add_task_natural();
        }
        Ok(())
    }

    async fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            self.poll_pending_task_intake().await?;
            let view = self.view();
            terminal.draw(|frame| ui::render(frame, &self.store, &mut self.widgets, &view))?;

            if event::poll(Duration::from_millis(120))? {
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
                    Event::Paste(text) => self.dispatch_paste(&text),
                    _ => {}
                }
            }

            if self.store.last_refresh.elapsed() >= Duration::from_secs(5)
                && let Err(error) = self.refresh().await
            {
                self.set_error(format!("refresh failed: {error:#}"));
            }

            self.clear_expired_message();
        }
        Ok(())
    }

    pub(crate) fn view(&self) -> ViewState {
        let mut overlay = self.overlay.as_ref().map(OverlayView::from);
        if let Some(AddTask(state)) = &mut overlay {
            state.status_prefix_active = self.pending_shortcut.has_add_task_status_prefix();
            state.priority_prefix_active = self.pending_shortcut.has_add_task_priority_prefix();
        }

        ViewState {
            focus: self.focus,
            overlay,
            detail_underlay: self.detail_underlay(),
            message: self.message.clone(),
            pending_shortcut: self.pending_shortcut.labels(),
            loading: self.loading.clone(),
            surface: if self.add_task_only {
                ViewSurface::AddTask
            } else {
                ViewSurface::Main
            },
        }
    }

    fn detail_underlay(&self) -> bool {
        self.detail_context
            || matches!(
                self.overlay,
                Some(OverlayState::Detail { .. } | OverlayState::DetailHelp { .. })
            )
            || self.authoring.detail_underlay()
    }

    pub(super) async fn refresh(&mut self) -> Result<()> {
        let selected_id = self
            .store
            .selected_task(self.widgets.table.selected())
            .map(|item| item.task.id.clone());
        self.widgets
            .table
            .select(self.store.refresh(selected_id.as_deref()).await?);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        Ok(())
    }

    fn clear_expired_message(&mut self) {
        if self
            .message_at
            .is_some_and(|time| time.elapsed() >= Duration::from_secs(4))
        {
            self.message = None;
            self.message_at = None;
        }
    }
}
