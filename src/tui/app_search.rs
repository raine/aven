use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::{App, PendingSearchPreview, SEARCH_PREVIEW_LIMIT};
use crate::tui::overlay::{OverlayState, SearchResultItem, SearchState};

fn open_search_results_key(key: KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
}

impl App {
    pub(crate) fn begin_search(&mut self) {
        self.pending_shortcut.clear();
        self.clear_live_search_preview();
        self.overlay = Some(OverlayState::Search(SearchState::blank()));
    }

    pub(super) async fn handle_search_paste(&mut self, state: &mut SearchState) -> Result<()> {
        self.schedule_search_preview(state);
        Ok(())
    }

    pub(super) async fn handle_search_key(
        &mut self,
        mut state: SearchState,
        key: KeyEvent,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.clear_live_search_preview();
            }
            KeyCode::Enter if open_search_results_key(key) => {
                self.clear_live_search_preview();
                self.accept_search_input(state.input.text).await?;
            }
            KeyCode::Tab => {
                self.clear_live_search_preview();
                self.accept_search_input(state.input.text).await?;
            }
            KeyCode::Enter => {
                self.clear_live_search_preview();
                if let Some(result) = state.selected_current_result() {
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
                self.schedule_search_preview(&mut state);
                self.overlay = Some(OverlayState::Search(state));
            }
        }
        Ok(())
    }

    async fn accept_search_input(&mut self, input: String) -> Result<()> {
        self.widgets
            .table
            .select(self.store.accept_search(&input).await?);
        Ok(())
    }

    fn start_search_preview(&mut self, query: String) {
        let workspace_id = self.store.active_workspace.id.clone();
        let handle = self
            .store
            .spawn_search_preview(query.clone(), SEARCH_PREVIEW_LIMIT);
        self.live_search.active = Some(PendingSearchPreview {
            query,
            workspace_id,
            handle,
        });
    }

    fn schedule_search_preview(&mut self, state: &mut SearchState) {
        let query = state.current_query();
        if query.is_empty() {
            state.clear_results();
            self.clear_live_search_preview();
            return;
        }

        if state.results_query.as_deref() != Some(query.as_str()) {
            state.clear_results();
        }

        match self
            .live_search
            .active
            .as_ref()
            .map(|active| active.query.as_str())
        {
            None => self.start_search_preview(query),
            Some(active) if active == query => self.live_search.pending = None,
            Some(_) => self.live_search.pending = Some(query),
        }
    }

    pub(super) async fn poll_search_preview(&mut self) -> Result<bool> {
        let Some(active) = self.live_search.active.as_ref() else {
            return Ok(false);
        };
        if !active.handle.is_finished() {
            return Ok(false);
        }

        let active = self
            .live_search
            .active
            .take()
            .expect("active search preview");
        let result_set = match active.handle.await {
            Ok(result_set) => result_set?,
            Err(error) if error.is_cancelled() => return Ok(false),
            Err(error) => return Err(error.into()),
        };

        let mut changed = false;
        if let Some(OverlayState::Search(state)) = &mut self.overlay
            && self.store.active_workspace.id == active.workspace_id
            && state.input.text.trim() == active.query
        {
            Self::apply_search_preview_results(state, active.query, result_set);
            changed = true;
        }

        if let Some(query) = self.live_search.pending.take() {
            self.start_search_preview(query);
        }

        Ok(changed)
    }

    fn apply_search_preview_results(
        state: &mut SearchState,
        query: String,
        result_set: crate::query::TaskSearchPreviewResultSet,
    ) {
        state.results_query = Some(query);
        state.total_matches = result_set.total_matches;
        state.results = result_set
            .items
            .into_iter()
            .map(|result| SearchResultItem {
                task_id: result.task_id,
                display_ref: result.display_ref,
                title: result.title,
                description: String::new(),
                project_key: result.project_key,
                status: result.status,
                priority: result.priority,
                created_at: result.created_at,
                labels: result.labels,
                matched_field: result.matched_field,
                snippet: result.snippet,
                score: result.score,
                deleted: result.deleted,
            })
            .collect();
        state.normalize_selection();
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
