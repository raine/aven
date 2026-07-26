use anyhow::Result;

use crate::tui::app::{App, Focus, TaskCopyKind, TaskRefKind};
use crate::tui::overlay::{ConfirmIntent, OverlayState, PickerIntent, TextIntent};
use crate::tui::platform::copy_to_clipboard;

pub(crate) const ADD_PROJECT_TITLE: &str = "Add project";
pub(crate) const RENAME_PROJECT_TITLE: &str = "Rename project";
pub(crate) const DELETE_PROJECT_TITLE: &str = "Delete project";
pub(crate) const DELETE_TASK_TITLE: &str = "Delete task";
pub(crate) const ADD_LABEL_TITLE: &str = "Add label";

fn task_text_for_copy(title: &str, description: &str, kind: TaskCopyKind) -> String {
    match kind {
        TaskCopyKind::Title => title.to_string(),
        TaskCopyKind::Description => description.to_string(),
        TaskCopyKind::TitleAndDescription if description.is_empty() => title.to_string(),
        TaskCopyKind::TitleAndDescription => format!("{title}\n\n{description}"),
    }
}

fn task_notes_for_copy(notes: &[crate::query::TaskNote]) -> String {
    notes
        .iter()
        .map(|note| note.body.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl App {
    pub(super) fn begin_add_project(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::blank_text_input(
            TextIntent::AddProject,
            ADD_PROJECT_TITLE,
            "project name:",
        ));
    }

    pub(super) fn begin_add_label(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::blank_text_input(
            TextIntent::AddLabel,
            ADD_LABEL_TITLE,
            "label name:",
        ));
    }

    pub(super) fn begin_delete_task(&mut self) {
        self.pending_shortcut.clear();
        let Some(selection) = crate::tui::task_selection::TaskSelection::resolve(
            &self.store.tasks,
            self.list.marked_task_ids(),
            self.list.selected_task(),
        ) else {
            self.set_info("no selected task to edit");
            return;
        };
        let return_to_detail = self.detail.is_some();
        if !self.marked_task_ids_in_view().is_empty() {
            let count = selection.len();
            self.overlay = Some(OverlayState::confirm(
                ConfirmIntent::DeleteTasks {
                    selection,
                    return_to_detail,
                },
                DELETE_TASK_TITLE,
                format!("Delete {count} marked tasks?"),
            ));
            return;
        }
        let Some(task) = self.store.selected_task(self.list.selected_task()) else {
            self.set_info("no selected task to edit");
            return;
        };
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::DeleteTasks {
                selection,
                return_to_detail,
            },
            DELETE_TASK_TITLE,
            format!("Delete {} {}?", task.display_ref, task.task.title),
        ));
    }

    pub(super) fn begin_rename_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = if self.list.focus() == Focus::Sidebar {
            self.selected_sidebar_project()
        } else {
            None
        };
        let items = self
            .store
            .existing_project_picker_items(selected.as_deref().unwrap_or_default());
        self.open_picker_overlay(
            PickerIntent::RenameProject,
            RENAME_PROJECT_TITLE,
            items,
            false,
        );
    }

    pub(super) fn begin_delete_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = if self.list.focus() == Focus::Sidebar {
            self.selected_sidebar_project()
        } else {
            None
        };
        let items = self
            .store
            .existing_project_picker_items(selected.as_deref().unwrap_or_default());
        self.open_picker_overlay(
            PickerIntent::DeleteProject,
            DELETE_PROJECT_TITLE,
            items,
            false,
        );
    }

    fn selected_sidebar_project(&self) -> Option<String> {
        self.list
            .selected_sidebar()
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
        let Some(task) = self.store.selected_task(self.list.selected_task()) else {
            self.set_info("no selected task to copy");
            return;
        };
        let (value, message_ref) = match kind {
            TaskRefKind::Short => (task.display_ref.clone(), task.display_ref.clone()),
            TaskRefKind::Durable => (task.task.id.to_string(), task.display_ref.clone()),
        };
        match copy_to_clipboard(&value) {
            Ok(()) => self.set_success(format!("copied {message_ref}")),
            Err(error) => self.set_error(format!("copy failed: {error}")),
        }
    }

    pub(super) fn copy_selected_task_text(&mut self, kind: TaskCopyKind) {
        let Some(task) = self.store.selected_task(self.list.selected_task()) else {
            self.set_info("no selected task to copy");
            return;
        };
        if kind == TaskCopyKind::Description && task.task.description.is_empty() {
            self.set_info("task description is empty");
            return;
        }
        let value = task_text_for_copy(&task.task.title, &task.task.description, kind);
        let copied = match kind {
            TaskCopyKind::Title => "task title",
            TaskCopyKind::Description => "task description",
            TaskCopyKind::TitleAndDescription => "task title and description",
        };
        match copy_to_clipboard(&value) {
            Ok(()) => self.set_success(format!("copied {copied}")),
            Err(error) => self.set_error(format!("copy failed: {error}")),
        }
    }

    pub(super) fn copy_selected_task_notes(&mut self) {
        let Some(task) = self.store.selected_task(self.list.selected_task()) else {
            self.set_info("no selected task to copy");
            return;
        };
        if task.notes.is_empty() {
            self.set_info("task has no notes");
            return;
        }
        match copy_to_clipboard(&task_notes_for_copy(&task.notes)) {
            Ok(()) => self.set_success("copied task notes"),
            Err(error) => self.set_error(format!("copy failed: {error}")),
        }
    }

    pub(super) fn submit_rename_project_picker(&mut self, values: Vec<String>) {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_rename_project();
            return;
        };
        self.overlay = Some(OverlayState::text_input(
            TextIntent::RenameProject {
                project: project.clone(),
            },
            RENAME_PROJECT_TITLE,
            "new project name:",
            project,
        ));
    }

    pub(super) async fn submit_rename_project(
        &mut self,
        project: String,
        value: String,
    ) -> Result<()> {
        match self.store.rename_project(&project, value.clone()).await {
            Ok(result) => {
                self.apply_mutation_result(result);
            }
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.overlay = Some(OverlayState::text_input(
                    TextIntent::RenameProject { project },
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
        self.overlay = Some(OverlayState::text_input(
            TextIntent::ConfirmDeleteProject {
                project: project.clone(),
            },
            DELETE_PROJECT_TITLE,
            format!("Type {project} to delete project:"),
            String::new(),
        ));
    }

    pub(super) async fn submit_delete_project_name(
        &mut self,
        project: String,
        value: String,
    ) -> Result<()> {
        if value.trim() != project {
            self.set_warning("project name does not match");
            self.overlay = Some(OverlayState::text_input(
                TextIntent::ConfirmDeleteProject {
                    project: project.clone(),
                },
                DELETE_PROJECT_TITLE,
                format!("Type {project} to delete project:"),
                value,
            ));
            return Ok(());
        }
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::DeleteProject {
                project: project.clone(),
            },
            DELETE_PROJECT_TITLE,
            format!("Delete project {project}?"),
        ));
        Ok(())
    }

    pub(super) async fn submit_delete_project(&mut self, project: String) -> Result<()> {
        match self.store.delete_project(&project).await {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => self.set_error(format!("{error:#}")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_copy_text_preserves_description_formatting() {
        let description = "First paragraph.\n\n- one\n  - nested\n\n```rust\nfn main() {}\n```\n";

        assert_eq!(
            task_text_for_copy("Copy me", description, TaskCopyKind::Description),
            description
        );
        assert_eq!(
            task_text_for_copy("Copy me", description, TaskCopyKind::TitleAndDescription),
            format!("Copy me\n\n{description}")
        );
    }

    #[test]
    fn combined_task_copy_omits_separator_for_empty_description() {
        assert_eq!(
            task_text_for_copy("Title only", "", TaskCopyKind::TitleAndDescription),
            "Title only"
        );
    }

    #[test]
    fn task_notes_copy_preserves_bodies_and_separates_notes() {
        let notes = vec![
            crate::query::TaskNote {
                body: "First note\n- item".to_string(),
                created_at: "2026-07-13T10:00:00Z".to_string(),
            },
            crate::query::TaskNote {
                body: "Second note\n\nParagraph".to_string(),
                created_at: "2026-07-13T11:00:00Z".to_string(),
            },
        ];

        assert_eq!(
            task_notes_for_copy(&notes),
            "First note\n- item\n\nSecond note\n\nParagraph"
        );
    }
}
