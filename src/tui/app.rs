use std::time::Instant;

use anyhow::Result;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::config::AppConfig;
use crate::operations::TaskDraft;
use crate::query::TaskSort;
use crate::tui::authoring::{
    ADD_NOTE_TITLE, ADD_TASK_TITLE_PRIORITY_TITLE, ADD_TASK_TITLE_PROJECT_TITLE, AddNoteSubmit,
    AddTaskStep, AddTaskTitleSubmit, AuthoringState,
};
use crate::tui::config_overlay::{
    config_info_overlay, config_init_overlay, config_paths_overlay, config_status_overlay,
};
use crate::tui::conflict_flow::ConflictFlowState;
use crate::tui::navigation::{next_index, next_selectable_sidebar};
use crate::tui::overlay::{
    AddTaskState, LineEdit, MultilineInputState, OverlayRoute, OverlayState, PickerItem,
};
use crate::tui::platform::{copy_to_clipboard, edit_text_externally};
use crate::tui::shortcut_buffer::ShortcutBuffer;
use crate::tui::store::{SidebarTarget, TuiStore};
use crate::tui::toast::{Toast, ToastSeverity};

const ADD_PROJECT_TITLE: &str = "Add project";
const DELETE_PROJECT_TITLE: &str = "Delete project";
const DELETE_TASK_TITLE: &str = "Delete task";
const ADD_LABEL_TITLE: &str = "Add label";
const ADD_TASK_NATURAL_TITLE: &str = "Add task: natural language";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskRefKind {
    Short,
    Durable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Tasks,
}

pub(crate) struct WidgetState {
    pub(crate) sidebar: ListState,
    pub(crate) table: TableState,
}

pub(crate) struct App {
    pub(crate) store: TuiStore,
    pub(crate) should_quit: bool,
    pub(crate) focus: Focus,
    pub(crate) widgets: WidgetState,
    pub(crate) overlay: Option<OverlayState>,
    pub(crate) message: Option<Toast>,
    pub(crate) message_at: Option<Instant>,
    pub(super) pending_shortcut: ShortcutBuffer,
    pub(super) detail_context: bool,
    pub(super) authoring: AuthoringState,
    pub(super) conflict_flow: ConflictFlowState,
    pending_delete_project: Option<String>,
    pub(super) needs_terminal_clear: bool,
    pub(super) add_task_only: bool,
    pub(super) add_task_only_message: Option<String>,
    pub(super) add_task_config: AppConfig,
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
        let mut app = Self {
            store,
            should_quit: false,
            focus: Focus::Tasks,
            widgets: WidgetState {
                sidebar: ListState::default(),
                table: TableState::default(),
            },
            overlay: None,
            message: None,
            message_at: None,
            pending_shortcut: ShortcutBuffer::default(),
            detail_context: false,
            authoring: AuthoringState::default(),
            conflict_flow: ConflictFlowState::default(),
            pending_delete_project: None,
            needs_terminal_clear: false,
            add_task_only: false,
            add_task_only_message: None,
            add_task_config: AppConfig::default(),
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
        if matches!(self.store.active_view, SidebarTarget::Conflicts)
            || self.store.filters.conflicts_only
        {
            self.move_to_conflict(-1);
        } else {
            self.set_info("previous item is available in conflict flows");
        }
    }

    pub(super) fn next_item(&mut self) {
        if matches!(self.store.active_view, SidebarTarget::Conflicts)
            || self.store.filters.conflicts_only
        {
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
            input: LineEdit::new(self.store.filters.search.clone().unwrap_or_default()),
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
            input: LineEdit::blank(),
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
        let active_project = match &self.store.active_view {
            SidebarTarget::Project(project) => Some(project.clone()),
            _ => None,
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

    pub(super) fn begin_add_task_title_priority(&mut self) {
        let Some(selected) = self.authoring.selected_add_task_priority() else {
            return;
        };
        let items = self.store.priority_picker_items(&selected);
        self.open_picker_overlay(
            OverlayRoute::AddTaskTitlePriority,
            ADD_TASK_TITLE_PRIORITY_TITLE,
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

    pub(super) async fn submit_add_task_natural(&mut self, value: String) -> Result<()> {
        let raw = value.trim();
        if raw.is_empty() {
            self.set_warning("task description is required");
            self.begin_add_task_natural_with_value(value);
            return Ok(());
        }
        match self
            .store
            .parse_task_intake(&self.add_task_config.agent.task_intake, raw)
            .await
        {
            Ok(draft) => {
                if self.authoring.apply_add_task_draft(draft) {
                    self.set_success("parsed task draft, review and save");
                    self.begin_add_task_step();
                }
            }
            Err(error) => {
                self.set_error(format!("task intake failed: {error:#}"));
                self.begin_add_task_natural_with_value(value);
            }
        }
        Ok(())
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
        self.pending_delete_project = None;
        let had_overlay = self.overlay.take().is_some();
        self.detail_context = false;
        if !had_overlay && self.focus == Focus::Sidebar {
            self.focus = Focus::Tasks;
            self.widgets.sidebar.select(self.store.sidebar_selection());
        }
    }

    pub(super) async fn set_sort(&mut self, sort: TaskSort) -> Result<()> {
        let selected = self.store.set_sort(sort).await?;
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
                SidebarTarget::Project(project) => Some(project.clone()),
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
        self.message = Some(Toast::new(message, severity));
        self.message_at = Some(Instant::now());
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

    pub(super) fn submit_delete_project_picker(&mut self, values: Vec<String>) {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_delete_project();
            return;
        };
        self.pending_delete_project = Some(project.clone());
        self.overlay = Some(OverlayState::confirm(
            OverlayRoute::DeleteProjectConfirm,
            DELETE_PROJECT_TITLE,
            format!("Delete project {project}?"),
        ));
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

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
