use crate::ids::WorkspaceId;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::task::JoinHandle;

use crate::query::{SearchMatchedField, TaskSearchPreviewResultSet};
use crate::tui::app::{App, SEARCH_PREVIEW_LIMIT};
use crate::tui::overlay::{LineEdit, OverlayState, SearchIntent, SearchResultItem, SearchState};

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

    pub(super) fn begin_add_epic_child(&mut self) {
        let selected = self.list.selected_task();
        let Some(context) = self.store.resolve_add_epic_child_context(selected) else {
            self.set_warning("Select a task to add a child");
            return;
        };
        self.pending_shortcut.clear();
        match context {
            crate::tui::store::AddEpicChildContext::Existing(epic) => {
                self.open_add_epic_child_search(epic);
            }
            crate::tui::store::AddEpicChildContext::Promote(epic) => {
                self.clear_live_search_preview();
                self.overlay = Some(OverlayState::confirm(
                    crate::tui::overlay::ConfirmIntent::PromoteTaskForChild { epic: epic.clone() },
                    "Promote task to epic",
                    format!(
                        "Adding a child will promote {} to an epic. Continue?",
                        epic.display_ref
                    ),
                ));
            }
        }
    }

    pub(super) fn open_add_epic_child_search(&mut self, epic: crate::tui::store::EpicContext) {
        self.clear_live_search_preview();
        let mut state = SearchState::for_intent(SearchIntent::AddEpicChild {
            epic_id: epic.epic_id,
            display_ref: epic.display_ref,
            project_key: epic.project_key,
        });
        Self::set_create_epic_child_result(&mut state);
        self.overlay = Some(OverlayState::Search(state));
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
                    && matches!(state.intent, SearchIntent::Navigate) =>
            {
                self.clear_live_search_preview();
                self.accept_search_input(state.input.text).await?;
            }
            KeyCode::Tab if matches!(state.intent, SearchIntent::Navigate) => {
                self.clear_live_search_preview();
                self.accept_search_input(state.input.text).await?;
            }
            KeyCode::Tab => {
                self.overlay = Some(OverlayState::Search(state));
            }
            KeyCode::Enter => {
                self.clear_live_search_preview();
                if let Some(result) = state.selected_current_result().cloned() {
                    self.accept_search_result(state.intent.clone(), state.input.text, result)
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
        let selected = if self.store.view_state.view == crate::tui::store::TaskView::Recurring {
            let selected_id = self
                .store
                .selected_recurrence_series(self.list.selected_task())
                .map(|item| item.series.id.clone());
            self.store
                .set_recurring_search(input, selected_id.as_ref())
                .await?
        } else {
            self.store.accept_search(&input).await?
        };
        self.list.select_task(selected);
        self.push_navigation_state(previous);
        Ok(())
    }

    async fn accept_search_result(
        &mut self,
        intent: SearchIntent,
        input: String,
        result: SearchResultItem,
    ) -> Result<()> {
        match intent {
            SearchIntent::Navigate => {
                self.accept_search_input(result.display_ref.clone()).await?;
                self.select_task_by_id(&result.task_id);
                self.detail = crate::tui::detail_session::DetailSession::open(0);
                self.show_detail(0);
            }
            SearchIntent::AddDependency {
                selection,
                display_ref,
            } => {
                let task_id = selection.single_id().cloned().expect("single selection");
                if task_id == result.task_id {
                    self.set_warning(format!("{display_ref} cannot depend on itself"));
                    self.reopen_add_dependency_search(selection, display_ref, input);
                    return Ok(());
                }
                match self
                    .store
                    .add_dependency_to_selection(&selection, &result.task_id)
                    .await
                {
                    Ok(result) => self.apply_mutation_result(result),
                    Err(error) => {
                        let committed = crate::tui::store::mutation_committed(&error);
                        self.set_error(format!("{error:#}"));
                        if !committed {
                            self.reopen_add_dependency_search(selection, display_ref, input);
                        }
                    }
                }
            }
            SearchIntent::AddEpicChild {
                epic_id,
                display_ref,
                project_key,
            } => {
                let epic = crate::tui::store::EpicContext {
                    epic_id,
                    display_ref,
                    project_key,
                };
                if result.create_new {
                    let mut return_search = SearchState::for_intent(SearchIntent::AddEpicChild {
                        epic_id: epic.epic_id.clone(),
                        display_ref: epic.display_ref.clone(),
                        project_key: epic.project_key.clone(),
                    });
                    return_search.input = LineEdit::new(input.clone());
                    self.authoring.begin_epic_child(epic, return_search, input);
                    self.begin_add_task_title();
                    return Ok(());
                }
                if let Some(reason) = result.unavailable_reason {
                    self.set_warning(reason);
                    self.reopen_add_epic_child_search(epic, input);
                    return Ok(());
                }
                match self
                    .store
                    .add_epic_child(epic.clone(), result.task_id)
                    .await
                {
                    Ok(mutation) => {
                        let child_id = mutation.child.task_id.clone();
                        let selected = if self.detail.is_active() {
                            self.store
                                .tasks
                                .iter()
                                .position(|item| item.task.id == epic.epic_id)
                        } else {
                            mutation.message.selected
                        };
                        self.list.select_task(selected);
                        if let Some(detail) = self.detail.state_mut() {
                            detail.set_focused_target(Some(
                                crate::tui::app::DetailTargetId::Task {
                                    section: crate::tui::app::DetailSection::EpicChildren,
                                    task_id: child_id,
                                },
                            ));
                            detail.set_removed_epic_child(None);
                            detail.set_scroll(0);
                        }
                        self.set_success(mutation.message.message);
                    }
                    Err(error) => {
                        self.set_warning(epic_child_error_message(&error, &epic));
                        self.reopen_add_epic_child_search(epic, input);
                    }
                }
            }
        }
        Ok(())
    }

    fn reopen_add_dependency_search(
        &mut self,
        selection: crate::tui::task_selection::TaskSelection,
        display_ref: String,
        input: String,
    ) {
        let mut state = SearchState::for_intent(SearchIntent::AddDependency {
            selection,
            display_ref,
        });
        state.input = LineEdit::new(input);
        self.schedule_search_preview(&mut state);
        self.overlay = Some(OverlayState::Search(state));
    }

    fn reopen_add_epic_child_search(
        &mut self,
        epic: crate::tui::store::EpicContext,
        input: String,
    ) {
        let mut state = SearchState::for_intent(SearchIntent::AddEpicChild {
            epic_id: epic.epic_id,
            display_ref: epic.display_ref,
            project_key: epic.project_key,
        });
        state.input = LineEdit::new(input);
        self.schedule_search_preview(&mut state);
        self.overlay = Some(OverlayState::Search(state));
    }

    fn set_create_epic_child_result(state: &mut SearchState) {
        let query = state.current_query();
        state.results = vec![create_epic_child_result(&query)];
        state.selected = 0;
        state.results_query = Some(query);
    }

    fn start_search_preview(&mut self, query: String) {
        let workspace_id = self.store.active_workspace.id.clone();
        let handle = self
            .store
            .spawn_search_preview(query.clone(), SEARCH_PREVIEW_LIMIT);
        self.search.start(query, workspace_id, handle);
    }

    pub(super) fn schedule_search_preview(&mut self, state: &mut SearchState) {
        if self.store.view_state.view == crate::tui::store::TaskView::Recurring {
            state.clear_results();
            self.clear_live_search_preview();
            return;
        }
        let query = state.current_query();
        if query.is_empty() {
            if matches!(state.intent, SearchIntent::AddEpicChild { .. }) {
                Self::set_create_epic_child_result(state);
            } else {
                state.clear_results();
            }
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
        state.results_query = Some(query.clone());
        state.total_matches = result_set.total_matches;
        let intent = state.intent.clone();
        state.results = result_set
            .items
            .into_iter()
            .filter(|result| match &intent {
                SearchIntent::Navigate => true,
                SearchIntent::AddDependency { selection, .. } => {
                    selection.single_id() != Some(&result.task_id)
                }
                SearchIntent::AddEpicChild { project_key, .. } => {
                    result.project_key == *project_key
                }
            })
            .map(|result| {
                let unavailable_reason = match &intent {
                    SearchIntent::AddEpicChild {
                        epic_id,
                        display_ref,
                        ..
                    } if result.task_id == *epic_id => {
                        Some(format!("{display_ref} cannot be its own child"))
                    }
                    SearchIntent::AddEpicChild { .. } if result.is_epic => Some(format!(
                        "{} is unavailable: epics cannot be children",
                        result.display_ref
                    )),
                    SearchIntent::AddEpicChild { .. } if result.deleted => Some(format!(
                        "{} is unavailable: task is deleted",
                        result.display_ref
                    )),
                    SearchIntent::AddEpicChild { display_ref, .. }
                        if result
                            .epic_parent_display_ref
                            .as_ref()
                            .is_some_and(|parent| parent != display_ref) =>
                    {
                        Some(format!(
                            "{} is unavailable: belongs to {}",
                            result.display_ref,
                            result
                                .epic_parent_display_ref
                                .as_deref()
                                .unwrap_or_default()
                        ))
                    }
                    _ => None,
                };
                SearchResultItem {
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
                    unavailable_reason,
                    create_new: false,
                }
            })
            .collect();
        if matches!(intent, SearchIntent::AddEpicChild { .. }) {
            state.results.insert(0, create_epic_child_result(&query));
        }
        state.normalize_selection();
    }
}

fn create_epic_child_result(query: &str) -> SearchResultItem {
    SearchResultItem {
        task_id: crate::ids::TaskId::new(),
        display_ref: String::new(),
        title: if query.is_empty() {
            "Create a new child task...".to_string()
        } else {
            format!("Create a child using \"{query}\"...")
        },
        description: String::new(),
        project_key: String::new(),
        status: String::new(),
        priority: String::new(),
        created_at: String::new(),
        labels: Vec::new(),
        matched_field: SearchMatchedField::Title,
        snippet: None,
        score: i64::MAX,
        deleted: false,
        is_epic: false,
        unavailable_reason: None,
        create_new: true,
    }
}

fn epic_child_error_message(
    error: &anyhow::Error,
    epic: &crate::tui::store::EpicContext,
) -> String {
    let message = format!("{error:#}");
    if message.contains("epic-child-already-linked") {
        "Selected task already belongs to another epic".to_string()
    } else if message.contains("epic-cross-project") {
        format!(
            "Selected task must belong to the same project as {}",
            epic.display_ref
        )
    } else if message.contains("epic-child-is-epic") {
        "Epics cannot be children".to_string()
    } else if message.contains("epic-child-deleted") {
        "Deleted tasks cannot be children".to_string()
    } else {
        message
    }
}
