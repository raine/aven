use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::App;
use crate::tui::overlay::{OverlayState, SearchResultItem, SearchState};

fn open_search_results_key(key: KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
}

impl App {
    pub(crate) fn begin_search(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Search(SearchState::blank()));
    }

    pub(super) async fn handle_search_paste(&mut self, state: &mut SearchState) -> Result<()> {
        self.refresh_search_results(state).await
    }

    pub(super) async fn handle_search_key(
        &mut self,
        mut state: SearchState,
        key: KeyEvent,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Enter if open_search_results_key(key) => {
                self.accept_search_input(state.input.text).await?;
            }
            KeyCode::Tab => {
                self.accept_search_input(state.input.text).await?;
            }
            KeyCode::Enter => {
                if let Some(result) = state.selected_result() {
                    self.accept_search_input(state.input.text.clone()).await?;
                    self.select_task_by_id(&result.task_id);
                    self.overlay = Some(OverlayState::Detail { scroll: 0 });
                } else {
                    self.accept_search_input(state.input.text).await?;
                }
            }
            KeyCode::Down if !state.results.is_empty() => {
                state.selected = (state.selected + 1) % state.results.len();
                self.overlay = Some(OverlayState::Search(state));
            }
            KeyCode::Char('n')
                if key.modifiers.contains(KeyModifiers::CONTROL) && !state.results.is_empty() =>
            {
                state.selected = (state.selected + 1) % state.results.len();
                self.overlay = Some(OverlayState::Search(state));
            }
            KeyCode::Up if !state.results.is_empty() => {
                state.selected = state
                    .selected
                    .checked_sub(1)
                    .unwrap_or(state.results.len().saturating_sub(1));
                self.overlay = Some(OverlayState::Search(state));
            }
            KeyCode::Char('p')
                if key.modifiers.contains(KeyModifiers::CONTROL) && !state.results.is_empty() =>
            {
                state.selected = state
                    .selected
                    .checked_sub(1)
                    .unwrap_or(state.results.len().saturating_sub(1));
                self.overlay = Some(OverlayState::Search(state));
            }
            _ => {
                state.input.handle_key(key);
                self.refresh_search_results(&mut state).await?;
                self.overlay = Some(OverlayState::Search(state));
            }
        }
        Ok(())
    }

    async fn refresh_search_results(&mut self, state: &mut SearchState) -> Result<()> {
        let text = state.input.text.trim();
        if text.is_empty() {
            state.results.clear();
            state.selected = 0;
            state.total_matches = 0;
            return Ok(());
        }
        let result_set = self.store.search_preview(text, 8).await?;
        state.total_matches = result_set.total_matches;
        state.results = result_set
            .items
            .into_iter()
            .map(|result| SearchResultItem {
                task_id: result.item.task.id,
                display_ref: result.item.display_ref,
                title: result.item.task.title,
                description: result.item.task.description,
                project_key: result.item.task.project_key,
                status: result.item.task.status,
                priority: result.item.task.priority,
                created_at: result.item.task.created_at,
                labels: result.item.labels,
                matched_field: result.matched_field,
                snippet: result.snippet,
                score: result.score,
                deleted: result.item.task.deleted,
            })
            .collect();
        state.normalize_selection();
        Ok(())
    }

    async fn accept_search_input(&mut self, input: String) -> Result<()> {
        self.widgets
            .table
            .select(self.store.accept_search(&input).await?);
        Ok(())
    }

    fn select_task_by_id(&mut self, task_id: &str) {
        if let Some(index) = self
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
        {
            self.widgets.table.select(Some(index));
        }
    }
}
