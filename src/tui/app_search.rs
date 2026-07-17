use crate::ids::WorkspaceId;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::task::JoinHandle;

use crate::query::TaskSearchPreviewResultSet;
use crate::tui::app::{App, SEARCH_PREVIEW_LIMIT};
use crate::tui::overlay::{LineEdit, OverlayState, SearchPurpose, SearchResultItem, SearchState};

struct PendingSearchPreview {
    query: String,
    workspace_id: WorkspaceId,
    handle: JoinHandle<Result<TaskSearchPreviewResultSet>>,
}

enum SearchStateMachine {
    Idle,
    Running(PendingSearchPreview),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchControllerView<'a> {
    Idle,
    Running { query: &'a str },
}

struct CompletedSearchPreview {
    query: String,
    workspace_id: WorkspaceId,
    result_set: TaskSearchPreviewResultSet,
}

pub(super) struct SearchController {
    state: SearchStateMachine,
}

impl SearchController {
    pub(super) fn new() -> Self {
        Self {
            state: SearchStateMachine::Idle,
        }
    }

    pub(super) fn start(
        &mut self,
        query: String,
        workspace_id: WorkspaceId,
        handle: JoinHandle<Result<TaskSearchPreviewResultSet>>,
    ) {
        self.cancel();
        self.state = SearchStateMachine::Running(PendingSearchPreview {
            query,
            workspace_id,
            handle,
        });
    }

    pub(super) fn cancel(&mut self) {
        if let SearchStateMachine::Running(active) =
            std::mem::replace(&mut self.state, SearchStateMachine::Idle)
        {
            active.handle.abort();
        }
    }

    pub(super) fn view(&self) -> SearchControllerView<'_> {
        match &self.state {
            SearchStateMachine::Idle => SearchControllerView::Idle,
            SearchStateMachine::Running(active) => SearchControllerView::Running {
                query: &active.query,
            },
        }
    }

    pub(super) fn work_pending(&self) -> bool {
        matches!(self.state, SearchStateMachine::Running(_))
    }

    async fn poll(&mut self) -> Result<Option<CompletedSearchPreview>> {
        let SearchStateMachine::Running(active) = &self.state else {
            return Ok(None);
        };
        if !active.handle.is_finished() {
            return Ok(None);
        }

        let SearchStateMachine::Running(active) =
            std::mem::replace(&mut self.state, SearchStateMachine::Idle)
        else {
            unreachable!("search state checked above");
        };
        let result_set = match active.handle.await {
            Ok(result_set) => result_set?,
            Err(error) if error.is_cancelled() => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(CompletedSearchPreview {
            query: active.query,
            workspace_id: active.workspace_id,
            result_set,
        }))
    }
}

impl Drop for SearchController {
    fn drop(&mut self) {
        self.cancel();
    }
}

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
            KeyCode::Enter
                if open_search_results_key(key)
                    && matches!(state.purpose, SearchPurpose::Navigate) =>
            {
                self.clear_live_search_preview();
                self.accept_search_input(state.input.text).await?;
            }
            KeyCode::Tab if matches!(state.purpose, SearchPurpose::Navigate) => {
                self.clear_live_search_preview();
                self.accept_search_input(state.input.text).await?;
            }
            KeyCode::Tab => {
                self.overlay = Some(OverlayState::Search(state));
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
        let previous = self.store.view_state.clone();
        self.widgets
            .table
            .select(self.store.accept_search(&input).await?);
        self.push_navigation_state(previous);
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
        task_id: crate::ids::TaskId,
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
        self.search.start(query, workspace_id, handle);
    }

    fn schedule_search_preview(&mut self, state: &mut SearchState) {
        let query = state.current_query();
        if query.is_empty() {
            state.clear_results();
            self.clear_live_search_preview();
            return;
        }

        if matches!(
            self.search.view(),
            SearchControllerView::Running { query: active_query } if active_query == query
        ) {
            return;
        }

        self.clear_live_search_preview();
        self.start_search_preview(query);
    }

    pub(super) async fn poll_search_preview(&mut self) -> Result<bool> {
        let Some(completed) = self.search.poll().await? else {
            return Ok(false);
        };

        let mut changed = false;
        if let Some(OverlayState::Search(state)) = &mut self.overlay
            && self.store.active_workspace.id == completed.workspace_id
            && state.input.text.trim() == completed.query
        {
            Self::apply_search_preview_results(state, completed.query, completed.result_set);
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
                is_epic: result.is_epic,
            })
            .collect();
        state.normalize_selection();
    }

    fn select_task_by_id(&mut self, task_id: &crate::ids::TaskId) {
        if let Some(index) = self
            .store
            .tasks
            .iter()
            .position(|item| &item.task.id == task_id)
        {
            self.widgets.table.select(Some(index));
        }
    }
}
