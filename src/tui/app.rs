use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use anyhow::Result;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::config::AppConfig;
use crate::operations::TaskDraft;
use crate::tui::authoring::{
    ADD_NOTE_TITLE, ADD_TASK_TITLE_PROJECT_TITLE, AddNoteSubmit, AddTaskStep, AddTaskTitleSubmit,
    AuthoringState,
};
use crate::tui::config_overlay::{
    config_info_overlay, config_init_overlay, config_paths_overlay, config_status_overlay,
    database_stats_overlay,
};
use crate::tui::conflict_flow::ConflictFlowState;
use crate::tui::natural_add_runtime::{spawn_add_task_only_natural, task_intake_log_path};
use crate::tui::navigation::{next_index, next_selectable_sidebar};
use crate::tui::overlay::{
    AddTaskState, CommandState, LineEdit, MultilineInputState, OverlayRoute, OverlayState,
    PickerItem,
};
use crate::tui::platform::{copy_to_clipboard, edit_text_externally};
use crate::tui::shortcut_buffer::ShortcutBuffer;
use crate::tui::store::{TaskOrder, TaskScope, TaskView, TuiStore};
use crate::tui::toast::{Toast, ToastSeverity};

const ADD_PROJECT_TITLE: &str = "Add project";
const RENAME_PROJECT_TITLE: &str = "Rename project";
const DELETE_PROJECT_TITLE: &str = "Delete project";
const DELETE_TASK_TITLE: &str = "Delete task";
const ADD_LABEL_TITLE: &str = "Add label";
const ADD_TASK_NATURAL_TITLE: &str = "Add task: natural language";
pub(crate) const TASK_ROW_DOUBLE_CLICK: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRowClick {
    pub(crate) task_id: String,
    pub(crate) viewport_row: u16,
    pub(crate) at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskRefKind {
    Short,
    Durable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NaturalRetry {
    AddTask,
    Dialog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Tasks,
}

pub(super) struct PendingTaskIntake {
    handle: JoinHandle<Result<TaskDraft>>,
    retry: NaturalRetry,
    value: String,
    create_on_success: bool,
}

pub(super) struct ReadyTaskIntake {
    outcome: Result<TaskDraft>,
    retry: NaturalRetry,
    value: String,
    create_on_success: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notification {
    Toast {
        toast: Toast,
        created_at: Instant,
    },
    Loading {
        message: String,
        started_at: Instant,
    },
}

impl Notification {
    pub(crate) fn toast(message: impl Into<String>, severity: ToastSeverity) -> Self {
        Self::Toast {
            toast: Toast::new(message, severity),
            created_at: Instant::now(),
        }
    }

    pub(crate) fn loading(message: impl Into<String>) -> Self {
        Self::Loading {
            message: message.into(),
            started_at: Instant::now(),
        }
    }

    pub(crate) fn toast_view(&self) -> Toast {
        match self {
            Self::Toast { toast, .. } => toast.clone(),
            Self::Loading {
                message,
                started_at,
            } => Toast::new(
                format!("{} {message}", loading_frame(*started_at)),
                ToastSeverity::Info,
            )
            .without_icon(),
        }
    }
}

fn loading_frame(started_at: Instant) -> &'static str {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let elapsed = started_at.elapsed().as_millis() as usize;
    frames[(elapsed / 120) % frames.len()]
}

pub(crate) struct WidgetState {
    pub(crate) sidebar: ListState,
    pub(crate) table: TableState,
}

pub(crate) struct App {
    pub(crate) store: TuiStore,
    pub(crate) should_quit: bool,
    pub(crate) focus: Focus,
    add_task_db_path: Option<PathBuf>,
    pub(crate) widgets: WidgetState,
    pub(crate) overlay: Option<OverlayState>,
    pub(crate) notification: Option<Notification>,
    pub(super) pending_shortcut: ShortcutBuffer,
    pub(super) detail_context: bool,
    pub(super) authoring: AuthoringState,
    pub(super) conflict_flow: ConflictFlowState,
    pending_rename_project: Option<String>,
    pending_delete_project: Option<String>,
    pub(super) needs_terminal_clear: bool,
    pub(super) add_task_only: bool,
    pub(super) add_task_only_message: Option<String>,
    pub(super) add_task_config: AppConfig,
    pub(super) pending_task_intake: Option<PendingTaskIntake>,
    pub(super) ready_task_intake: Option<ReadyTaskIntake>,
    pub(super) next_refresh_at: Instant,
    pub(crate) last_task_click: Option<TaskRowClick>,
}

impl App {
    pub(crate) async fn new(pool: SqlitePool, project: Option<&str>) -> Result<Self> {
        let store = match project {
            Some("") => TuiStore::new_for_inferred_project(pool).await?,
            Some(project) => TuiStore::new_for_project(pool, project).await?,
            None => TuiStore::new(pool).await?,
        };
        Self::new_with_store(store)
    }

    #[cfg(test)]
    pub(crate) async fn new_for_tests(pool: SqlitePool) -> Result<Self> {
        let store = TuiStore::new(pool).await?;
        Self::new_with_store(store)
    }

    fn new_with_store(store: TuiStore) -> Result<Self> {
        let next_refresh_at = store.last_refresh + crate::tui::app_lifecycle::REFRESH_INTERVAL;
        let mut app = Self {
            store,
            should_quit: false,
            focus: Focus::Tasks,
            widgets: WidgetState {
                sidebar: ListState::default(),
                table: TableState::default(),
            },
            overlay: None,
            notification: None,
            pending_shortcut: ShortcutBuffer::default(),
            detail_context: false,
            authoring: AuthoringState::default(),
            conflict_flow: ConflictFlowState::default(),
            pending_rename_project: None,
            pending_delete_project: None,
            needs_terminal_clear: false,
            add_task_only: false,
            add_task_only_message: None,
            add_task_db_path: None,
            add_task_config: AppConfig::default(),
            pending_task_intake: None,
            ready_task_intake: None,
            next_refresh_at,
            last_task_click: None,
        };
        app.widgets.sidebar.select(app.store.sidebar_selection());
        app.widgets
            .table
            .select(Some(0).filter(|_| !app.store.tasks.is_empty()));
        Ok(app)
    }

    pub(crate) fn set_config(&mut self, config: AppConfig) {
        self.add_task_config = config;
    }

    pub(crate) fn set_add_task_db_path(&mut self, db_path: PathBuf) {
        self.add_task_db_path = Some(db_path);
    }

    pub(super) async fn move_selection(&mut self, delta: isize) -> Result<()> {
        match self.focus {
            Focus::Tasks => {
                let next = next_index(
                    self.widgets.table.selected(),
                    self.store.tasks.len(),
                    delta,
                    true,
                );
                self.widgets.table.select(next);
            }
            Focus::Sidebar => {
                let next = next_selectable_sidebar(
                    self.widgets.sidebar.selected(),
                    &self.store.sidebar_entries,
                    delta,
                    true,
                );
                self.widgets.sidebar.select(next);
            }
        }
        Ok(())
    }

    pub(super) async fn select_edge(&mut self, last: bool) -> Result<()> {
        match self.focus {
            Focus::Tasks => {
                if self.store.tasks.is_empty() {
                    self.widgets.table.select(None);
                } else {
                    self.widgets.table.select(Some(if last {
                        self.store.tasks.len() - 1
                    } else {
                        0
                    }));
                }
            }
            Focus::Sidebar => {
                let next = if last {
                    self.store
                        .sidebar_entries
                        .iter()
                        .rposition(|entry| entry.target.is_some())
                } else {
                    self.store
                        .sidebar_entries
                        .iter()
                        .position(|entry| entry.target.is_some())
                };
                self.widgets.sidebar.select(next);
            }
        }
        Ok(())
    }

    pub(super) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => {
                self.widgets.sidebar.select(self.store.sidebar_selection());
                Focus::Tasks
            }
            Focus::Tasks => Focus::Sidebar,
        };
    }

    pub(super) fn move_left(&mut self) {
        self.focus = Focus::Sidebar;
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.overlay = None;
    }

    pub(super) fn move_right(&mut self) {
        self.focus = Focus::Tasks;
        self.overlay = None;
    }

    pub(super) fn previous_item(&mut self) {
        if self.store.view_state.view == TaskView::Conflicts {
            self.move_to_conflict(-1);
        } else {
            self.set_info("previous item is available in conflict flows");
        }
    }

    pub(super) fn next_item(&mut self) {
        if self.store.view_state.view == TaskView::Conflicts {
            self.move_to_conflict(1);
        } else {
            self.set_info("next item is available in conflict flows");
        }
    }

    pub(super) fn select_detail_task(&mut self, delta: isize) {
        let current = self.widgets.table.selected();
        let next = next_index(current, self.store.tasks.len(), delta, true);
        self.widgets.table.select(next);
        self.focus = Focus::Tasks;
        if current != next {
            let message = if delta > 0 {
                "selected next task"
            } else {
                "selected previous task"
            };
            self.set_info(message);
        }
    }

    pub(super) async fn activate_or_toggle_detail(&mut self) -> Result<()> {
        if self.focus == Focus::Sidebar {
            self.apply_sidebar_selection().await?;
        } else if matches!(self.overlay, Some(OverlayState::Detail { .. })) {
            self.overlay = None;
        } else {
            self.overlay = Some(OverlayState::Detail { scroll: 0 });
        }
        Ok(())
    }

    pub(super) async fn apply_sidebar_selection(&mut self) -> Result<()> {
        self.store
            .apply_sidebar_selection(self.widgets.sidebar.selected())
            .await?;
        self.focus = Focus::Tasks;
        self.overlay = None;
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.widgets
            .table
            .select(Some(0).filter(|_| !self.store.tasks.is_empty()));
        Ok(())
    }

    pub(crate) fn begin_search(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Search {
            input: LineEdit::new(
                self.store
                    .view_state
                    .filter_modifiers
                    .search
                    .clone()
                    .unwrap_or_default(),
            ),
        });
    }

    pub(super) async fn accept_search_input(&mut self, input: String) -> Result<()> {
        self.widgets
            .table
            .select(self.store.accept_search(&input).await?);
        Ok(())
    }

    pub(crate) fn begin_command(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Command {
            state: CommandState::blank(),
        });
    }

    pub(super) fn begin_add_project(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::blank_text_input(
            OverlayRoute::AddProject,
            ADD_PROJECT_TITLE,
            "project name:",
        ));
    }

    pub(super) fn begin_add_label(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::blank_text_input(
            OverlayRoute::AddLabel,
            ADD_LABEL_TITLE,
            "label name:",
        ));
    }

    pub(super) async fn begin_add_task(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        let active_project = match &self.store.view_state.scope {
            TaskScope::Project(project) => Some(project.clone()),
            TaskScope::Workspace => None,
        };
        let inferred_project = if active_project.is_none() {
            self.store.inferred_add_project().await?
        } else {
            None
        };
        self.authoring
            .begin_add_task(active_project, inferred_project);
        self.begin_add_task_title();
        Ok(())
    }

    pub(super) fn begin_add_task_title(&mut self) {
        self.begin_add_task_overlay();
    }

    fn begin_add_task_overlay(&mut self) {
        let Some(context) = self.authoring.add_task_context() else {
            return;
        };
        self.overlay = Some(OverlayState::AddTask(AddTaskState {
            title: LineEdit::new(context.title),
            description: MultilineInputState::from_value(
                OverlayRoute::AddTaskDescription,
                "Add task: description",
                "",
                context.description,
            ),
            focus: context.step,
            project: context.project,
            status: context.status,
            priority: context.priority,
        }));
    }

    pub(super) fn begin_add_task_step(&mut self) {
        self.begin_add_task_overlay();
    }

    pub(super) fn open_add_task_description_editor(&mut self) {
        let Some(context) = self.authoring.add_task_context() else {
            return;
        };
        self.needs_terminal_clear = true;
        match edit_text_externally(context.description.clone(), "description.md") {
            Ok(value) => {
                self.authoring.capture_add_task_fields(
                    context.title,
                    value,
                    AddTaskStep::Description,
                );
                self.begin_add_task_step();
            }
            Err(error) => {
                self.set_error(format!("editor failed: {error:#}"));
                self.begin_add_task_step();
            }
        }
    }

    pub(super) fn capture_add_task_state(&mut self, state: &AddTaskState) -> bool {
        self.authoring.capture_add_task_fields(
            state.title.text.clone(),
            state.description.lines.join("\n"),
            state.focus,
        )
    }

    pub(super) fn set_add_task_status(&mut self, status: &str) {
        if let Some(status) = self.authoring.apply_add_task_status(status) {
            if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                state.status = status.clone();
            } else {
                self.begin_add_task_step();
            }
            self.set_info(format!("add task status={status}"));
        }
    }

    pub(super) fn set_add_task_priority(&mut self, priority: &str) {
        if let Some(priority) = self.authoring.apply_add_task_priority_value(priority) {
            if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                state.priority = priority.clone();
            } else {
                self.begin_add_task_step();
            }
            self.set_info(format!("add task priority={priority}"));
        }
    }

    pub(super) async fn submit_add_task_from_authoring(&mut self) -> Result<()> {
        match self.authoring.submit_add_task() {
            AddTaskTitleSubmit::ReopenTitle { message } => {
                self.set_warning(message);
                self.begin_add_task_title();
            }
            AddTaskTitleSubmit::Create(draft) => {
                self.submit_created_task(draft).await?;
            }
            AddTaskTitleSubmit::Inactive => {}
        }
        Ok(())
    }

    pub(super) fn begin_add_note(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self
            .store
            .selected_task(self.widgets.table.selected())
            .cloned()
        else {
            self.set_info("no selected task for note");
            return;
        };
        let return_to_detail =
            self.detail_context || matches!(self.overlay, Some(OverlayState::Detail { .. }));
        self.authoring.begin_add_note(
            item.task.id.clone(),
            item.display_ref.clone(),
            return_to_detail,
        );
        self.detail_context = return_to_detail;
        self.overlay = Some(OverlayState::blank_multiline_input(
            OverlayRoute::AddNote,
            ADD_NOTE_TITLE,
            "note body:",
        ));
    }

    pub(super) fn begin_add_task_title_project(&mut self) {
        let Some(selected) = self.authoring.selected_add_task_project() else {
            return;
        };
        let items = self.store.project_picker_items(selected.as_deref());
        self.open_picker_overlay(
            OverlayRoute::AddTaskTitleProject,
            ADD_TASK_TITLE_PROJECT_TITLE,
            items,
            false,
        );
    }

    pub(super) fn begin_add_task_natural(&mut self) {
        self.begin_add_task_natural_with_value(String::new());
    }

    fn begin_add_task_natural_with_value(&mut self, value: String) {
        self.overlay = Some(OverlayState::multiline_input(
            OverlayRoute::AddTaskNatural,
            ADD_TASK_NATURAL_TITLE,
            "",
            value,
        ));
    }

    pub(super) async fn submit_add_task_title_natural(
        &mut self,
        title: String,
        description: String,
    ) -> Result<()> {
        let value = add_task_natural_intake(&title, &description);
        if self.add_task_only {
            self.submit_add_task_only_natural(value, NaturalRetry::AddTask)
                .await
        } else {
            self.submit_add_task_natural_with_retry(value, NaturalRetry::AddTask, true)
                .await
        }
    }

    async fn submit_add_task_only_natural(
        &mut self,
        value: String,
        retry: NaturalRetry,
    ) -> Result<()> {
        let raw = value.trim();
        if raw.is_empty() {
            self.set_warning("task description is required");
            self.retry_add_task_natural(value, retry);
            return Ok(());
        }
        let project = self.add_task_project_context();
        spawn_add_task_only_natural(
            raw,
            self.store.active_workspace.id.as_str(),
            self.add_task_db_path.as_deref(),
            project.as_deref(),
        )?;
        self.add_task_only_message = Some("adding task in background".to_string());
        self.should_quit = true;
        Ok(())
    }

    pub(super) async fn submit_add_task_natural(&mut self, value: String) -> Result<()> {
        if self.add_task_only {
            self.submit_add_task_only_natural(value, NaturalRetry::Dialog)
                .await
        } else {
            self.submit_add_task_natural_with_retry(value, NaturalRetry::Dialog, false)
                .await
        }
    }

    async fn submit_add_task_natural_with_retry(
        &mut self,
        value: String,
        retry: NaturalRetry,
        create_on_success: bool,
    ) -> Result<()> {
        let raw = value.trim();
        if raw.is_empty() {
            self.set_warning("task description is required");
            self.retry_add_task_natural(value, retry);
            return Ok(());
        }
        let project = self.add_task_project_context();
        let handle = self.store.spawn_task_intake(
            self.add_task_config.agent.task_intake.clone(),
            raw.to_string(),
            project,
        );
        self.notification = Some(Notification::loading(if create_on_success {
            "adding task with LLM"
        } else {
            "parsing task with LLM"
        }));
        self.pending_task_intake = Some(PendingTaskIntake {
            handle,
            retry,
            value: value.clone(),
            create_on_success,
        });
        if create_on_success {
            self.overlay = None;
        } else {
            self.retry_add_task_natural(value, retry);
        }
        Ok(())
    }

    pub(super) async fn poll_pending_task_intake(&mut self) -> Result<bool> {
        if let Some(ready) = self.ready_task_intake.take() {
            self.finish_ready_task_intake(ready).await?;
            return Ok(true);
        }

        let Some(pending) = self
            .pending_task_intake
            .take_if(|pending| pending.handle.is_finished())
        else {
            return Ok(false);
        };
        let ready = ReadyTaskIntake {
            outcome: pending.handle.await?,
            retry: pending.retry,
            value: pending.value,
            create_on_success: pending.create_on_success,
        };
        if ready.outcome.is_err() {
            self.set_error("task intake failed");
        }
        self.ready_task_intake = Some(ready);
        Ok(true)
    }

    async fn finish_ready_task_intake(&mut self, ready: ReadyTaskIntake) -> Result<()> {
        match ready.outcome {
            Ok(draft) if ready.create_on_success => {
                self.submit_created_task(draft).await?;
            }
            Ok(draft) => {
                if self.authoring.apply_add_task_draft(draft) {
                    self.set_success("parsed task draft, review and save");
                    self.begin_add_task_step();
                }
            }
            Err(error) => {
                let log_path = task_intake_log_path();
                tracing::warn!(error = %error, "task intake failed");
                self.set_error(format!(
                    "task intake failed: {error:#}; logged to {}",
                    log_path.display()
                ));
                self.retry_add_task_natural(ready.value, ready.retry);
            }
        }
        Ok(())
    }

    fn retry_add_task_natural(&mut self, value: String, retry: NaturalRetry) {
        match retry {
            NaturalRetry::AddTask => self.begin_add_task_step(),
            NaturalRetry::Dialog => self.begin_add_task_natural_with_value(value),
        }
    }

    fn add_task_project_context(&self) -> Option<String> {
        self.authoring
            .selected_add_task_project()
            .flatten()
            .or_else(|| match &self.store.view_state.scope {
                TaskScope::Project(project) => Some(project.clone()),
                TaskScope::Workspace => None,
            })
    }

    async fn submit_created_task(&mut self, draft: TaskDraft) -> Result<()> {
        let current_selected = self.widgets.table.selected();
        let (message, selected) = self.store.create_task(draft, current_selected).await?;
        self.widgets.table.select(selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        if selected.is_none() {
            self.restore_selection_after_mutation();
        }
        self.set_success(message.clone());
        if self.add_task_only {
            self.add_task_only_message = Some(message);
            self.should_quit = true;
        }
        Ok(())
    }

    pub(super) async fn submit_add_note(&mut self, body: String) -> Result<()> {
        match self.authoring.submit_add_note(body) {
            AddNoteSubmit::Create {
                task_id,
                display_ref,
                body,
                return_to_detail,
            } => {
                let note_id = self.store.add_note_to_task(&task_id, body).await?;
                self.refresh().await?;
                self.restore_detail_overlay(return_to_detail);
                self.set_success(format!("added note {note_id} to {display_ref}"));
            }
            AddNoteSubmit::Blank {
                return_to_detail,
                message,
            } => {
                self.restore_detail_overlay(return_to_detail);
                self.set_warning(message);
            }
            AddNoteSubmit::Inactive { message } => {
                self.set_info(message);
            }
        }
        Ok(())
    }

    pub(super) fn cancel_authoring_overlay(&mut self) {
        self.pending_shortcut.clear();
        let return_to_detail = self.authoring.cancel() || self.detail_context;
        self.overlay = None;
        self.conflict_flow.clear();
        self.pending_rename_project = None;
        self.pending_delete_project = None;
        self.detail_context = false;
        self.restore_detail_overlay(return_to_detail);
    }

    pub(super) fn restore_detail_overlay(&mut self, return_to_detail: bool) {
        self.restore_detail_overlay_at_scroll(return_to_detail, 0);
    }

    pub(super) fn restore_detail_overlay_at_scroll(&mut self, return_to_detail: bool, scroll: u16) {
        if return_to_detail
            && self
                .store
                .selected_task(self.widgets.table.selected())
                .is_some()
        {
            self.detail_context = false;
            self.overlay = Some(OverlayState::Detail { scroll });
        }
    }

    pub(super) fn cancel_overlay(&mut self) {
        self.pending_shortcut.clear();
        self.authoring.clear();
        self.conflict_flow.clear();
        self.pending_rename_project = None;
        self.pending_delete_project = None;
        let had_overlay = self.overlay.take().is_some();
        self.detail_context = false;
        if !had_overlay && self.focus == Focus::Sidebar {
            self.focus = Focus::Tasks;
            self.widgets.sidebar.select(self.store.sidebar_selection());
        }
    }

    pub(super) async fn set_sort(&mut self, sort: TaskOrder) -> Result<()> {
        let selected = self.store.set_order(sort).await?;
        self.apply_filter_selection(selected);
        self.set_info(format!(
            "order {} {}",
            self.store.sort_label(),
            self.store.sort_direction_label()
        ));
        Ok(())
    }

    pub(super) async fn reverse_sort(&mut self) -> Result<()> {
        let selected = self.store.reverse_sort().await?;
        self.apply_filter_selection(selected);
        self.set_info(format!(
            "order {} {}",
            self.store.sort_label(),
            self.store.sort_direction_label()
        ));
        Ok(())
    }

    pub(super) fn apply_mutation_result(&mut self, result: crate::tui::store::MutationMessage) {
        self.widgets.table.select(result.selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.set_success(result.message);
    }

    pub(super) fn open_picker_overlay(
        &mut self,
        route: OverlayRoute,
        title: &str,
        items: Vec<PickerItem>,
        multi: bool,
    ) {
        self.overlay = Some(OverlayState::picker(route, title, items, multi));
    }

    pub(super) fn require_picker_value(
        &mut self,
        values: Vec<String>,
        message: &str,
    ) -> Option<String> {
        match values.first().cloned() {
            Some(value) => Some(value),
            None => {
                self.set_warning(message);
                None
            }
        }
    }

    pub(super) fn begin_delete_task(&mut self) {
        self.pending_shortcut.clear();
        let Some(task) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_info("no selected task to edit");
            return;
        };
        self.detail_context =
            self.detail_context || matches!(self.overlay, Some(OverlayState::Detail { .. }));
        self.overlay = Some(OverlayState::confirm(
            OverlayRoute::DeleteTaskConfirm,
            DELETE_TASK_TITLE,
            format!("Delete {} {}?", task.display_ref, task.task.title),
        ));
    }

    pub(super) fn begin_rename_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = if self.focus == Focus::Sidebar {
            self.selected_sidebar_project()
        } else {
            None
        };
        let items = self
            .store
            .existing_project_picker_items(selected.as_deref().unwrap_or_default());
        self.open_picker_overlay(
            OverlayRoute::RenameProjectPicker,
            RENAME_PROJECT_TITLE,
            items,
            false,
        );
    }

    pub(super) fn begin_delete_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = if self.focus == Focus::Sidebar {
            self.selected_sidebar_project()
        } else {
            None
        };
        let items = self
            .store
            .existing_project_picker_items(selected.as_deref().unwrap_or_default());
        self.open_picker_overlay(
            OverlayRoute::DeleteProjectPicker,
            DELETE_PROJECT_TITLE,
            items,
            false,
        );
    }

    fn selected_sidebar_project(&self) -> Option<String> {
        self.widgets
            .sidebar
            .selected()
            .and_then(|index| self.store.sidebar_entries.get(index))
            .and_then(|entry| entry.target.as_ref())
            .and_then(|target| match target {
                crate::tui::store::SidebarEntryTarget::Scope(
                    crate::tui::store::TaskScopeTarget::Project(project),
                ) => Some(project.clone()),
                _ => None,
            })
    }

    pub(super) fn copy_selected_ref(&mut self, kind: TaskRefKind) {
        let Some(task) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_info("no selected task to copy");
            return;
        };
        let (value, message_ref) = match kind {
            TaskRefKind::Short => (task.display_ref.clone(), task.display_ref.clone()),
            TaskRefKind::Durable => (task.task.id.clone(), task.display_ref.clone()),
        };
        match copy_to_clipboard(&value) {
            Ok(()) => self.set_success(format!("copied {message_ref}")),
            Err(error) => self.set_error(format!("copy failed: {error}")),
        }
    }

    pub(super) fn restore_selection_after_mutation(&mut self) {
        self.widgets.sidebar.select(self.store.sidebar_selection());
        if self.store.tasks.is_empty() {
            self.widgets.table.select(None);
        } else if self
            .widgets
            .table
            .selected()
            .is_none_or(|index| index >= self.store.tasks.len())
        {
            self.widgets.table.select(Some(0));
        }
    }

    pub(super) fn set_info(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Info);
    }

    pub(super) fn set_warning(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Warning);
    }

    pub(super) fn set_error(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Error);
    }

    pub(super) fn set_success(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Success);
    }

    fn set_toast(&mut self, message: impl Into<String>, severity: ToastSeverity) {
        self.notification = Some(Notification::toast(message, severity));
    }

    pub(super) fn show_config_status(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_status_overlay(&self.store)?);
        Ok(())
    }

    pub(super) fn show_config_info(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_info_overlay(&self.store)?);
        Ok(())
    }

    pub(super) fn show_config_paths(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_paths_overlay(&self.store)?);
        Ok(())
    }

    pub(super) async fn show_database_stats(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.store.load_database_stats().await?;
        self.overlay = Some(database_stats_overlay(&self.store)?);
        Ok(())
    }

    pub(super) fn begin_config_init(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_init_overlay()?);
        Ok(())
    }

    pub(super) fn submit_config_init(&mut self) -> Result<()> {
        let message = self.store.init_config()?;
        self.set_success(message);
        Ok(())
    }

    pub(super) fn open_description_external_editor(&mut self, state: MultilineInputState) {
        self.needs_terminal_clear = true;
        match edit_text_externally(state.lines.join("\n"), "description.md") {
            Ok(value) => self.overlay = Some(description_overlay_from_value(value)),
            Err(error) => {
                self.set_error(format!("editor failed: {error:#}"));
                self.overlay = Some(OverlayState::MultilineInput(state));
            }
        }
    }

    pub(super) fn submit_rename_project_picker(&mut self, values: Vec<String>) {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_rename_project();
            return;
        };
        self.pending_rename_project = Some(project.clone());
        self.overlay = Some(OverlayState::text_input(
            OverlayRoute::RenameProjectName,
            RENAME_PROJECT_TITLE,
            "new project name:",
            project,
        ));
    }

    pub(super) async fn submit_rename_project(&mut self, value: String) -> Result<()> {
        let Some(project) = self.pending_rename_project.clone() else {
            self.set_warning("project rename is not active");
            return Ok(());
        };
        match self.store.rename_project(&project, value.clone()).await {
            Ok(result) => {
                self.pending_rename_project = None;
                self.apply_mutation_result(result);
            }
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.overlay = Some(OverlayState::text_input(
                    OverlayRoute::RenameProjectName,
                    RENAME_PROJECT_TITLE,
                    "new project name:",
                    value,
                ));
            }
        }
        Ok(())
    }

    pub(super) fn submit_delete_project_picker(&mut self, values: Vec<String>) {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_delete_project();
            return;
        };
        self.pending_delete_project = Some(project.clone());
        self.overlay = Some(OverlayState::text_input(
            OverlayRoute::DeleteProjectNameConfirm,
            DELETE_PROJECT_TITLE,
            format!("Type {project} to delete project:"),
            String::new(),
        ));
    }

    pub(super) async fn submit_delete_project_name(&mut self, value: String) -> Result<()> {
        let Some(project) = self.pending_delete_project.clone() else {
            self.set_warning("project delete confirmation is not active");
            return Ok(());
        };
        if value.trim() != project {
            self.set_warning("project name does not match");
            self.overlay = Some(OverlayState::text_input(
                OverlayRoute::DeleteProjectNameConfirm,
                DELETE_PROJECT_TITLE,
                format!("Type {project} to delete project:"),
                value,
            ));
            return Ok(());
        }
        self.overlay = Some(OverlayState::confirm(
            OverlayRoute::DeleteProjectConfirm,
            DELETE_PROJECT_TITLE,
            format!("Delete project {project}?"),
        ));
        Ok(())
    }

    pub(super) async fn submit_delete_project(&mut self) -> Result<()> {
        let Some(project) = self.pending_delete_project.take() else {
            self.set_warning("project delete confirmation is not active");
            return Ok(());
        };
        match self.store.delete_project(&project).await {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => self.set_error(format!("{error:#}")),
        }
        Ok(())
    }
}

fn description_overlay_from_value(value: String) -> OverlayState {
    OverlayState::multiline_input(OverlayRoute::EditDescription, "Edit description", "", value)
}

fn add_task_natural_intake(title: &str, description: &str) -> String {
    let title = title.trim();
    let description = description.trim();
    match (title.is_empty(), description.is_empty()) {
        (false, false) => format!("Title:\n{title}\n\nDescription:\n{description}"),
        (false, true) => title.to_string(),
        (true, false) => format!("Description:\n{description}"),
        (true, true) => String::new(),
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
