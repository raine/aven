use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::{App, PendingSearchPreview, SEARCH_PREVIEW_LIMIT};
use crate::tui::overlay::{LineEdit, OverlayState, SearchPurpose, SearchResultItem, SearchState};

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
                if let Some(result) = state.selected_current_result().cloned() {
                    self.accept_search_result(state.purpose.clone(), state.input.text, result)
                        .await?;
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

    async fn accept_search_result(
        &mut self,
        purpose: SearchPurpose,
        input: String,
        result: SearchResultItem,
    ) -> Result<()> {
        match purpose {
            SearchPurpose::Navigate => {
                self.accept_search_input(result.display_ref.clone()).await?;
                self.select_task_by_id(&result.task_id);
                self.overlay = Some(OverlayState::Detail { scroll: 0 });
            }
            SearchPurpose::AddDependency {
                task_id,
                display_ref,
            } => {
                if task_id == result.task_id {
                    self.set_warning(format!("{display_ref} cannot depend on itself"));
                    self.reopen_add_dependency_search(task_id, display_ref, input);
                    return Ok(());
                }
                match self
                    .store
                    .add_dependency_to_task(&task_id, &result.task_id)
                    .await
                {
                    Ok(result) => self.apply_mutation_result(result),
                    Err(error) => {
                        self.set_error(format!("{error:#}"));
                        self.reopen_add_dependency_search(task_id, display_ref, input);
                    }
                }
            }
        }
        Ok(())
    }

    fn reopen_add_dependency_search(
        &mut self,
        task_id: String,
        display_ref: String,
        input: String,
    ) {
        let mut state = SearchState::for_purpose(SearchPurpose::AddDependency {
            task_id,
            display_ref,
        });
        state.input = LineEdit::new(input);
        self.schedule_search_preview(&mut state);
        self.overlay = Some(OverlayState::Search(state));
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

        if self
            .live_search
            .active
            .as_ref()
            .is_some_and(|active| active.query == query)
        {
            return;
        }

        self.clear_live_search_preview();
        self.start_search_preview(query);
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
            .filter(|result| match &state.purpose {
                SearchPurpose::Navigate => true,
                SearchPurpose::AddDependency { task_id, .. } => result.task_id != *task_id,
            })
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
