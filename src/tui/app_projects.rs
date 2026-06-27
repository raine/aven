use anyhow::Result;

use crate::tui::app::{App, Focus, TaskRefKind};
use crate::tui::overlay::{OverlayRoute, OverlayState};
use crate::tui::platform::copy_to_clipboard;

pub(crate) const ADD_PROJECT_TITLE: &str = "Add project";
pub(crate) const RENAME_PROJECT_TITLE: &str = "Rename project";
pub(crate) const DELETE_PROJECT_TITLE: &str = "Delete project";
pub(crate) const DELETE_TASK_TITLE: &str = "Delete task";
pub(crate) const ADD_LABEL_TITLE: &str = "Add label";

impl App {
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
