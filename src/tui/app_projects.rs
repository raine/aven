use std::env;
use std::path::Path;

use anyhow::Result;

use crate::tui::app::{App, Focus, TaskCopyKind, TaskRefKind};
use crate::tui::overlay::{ConfirmIntent, OverlayState, PickerIntent, PickerItem, TextIntent};
use crate::tui::platform::copy_to_clipboard;

pub(crate) const ADD_PROJECT_TITLE: &str = "Add project";
pub(crate) const ADD_PROJECT_PATH_TITLE: &str = "Add project path";
pub(crate) const REMOVE_PROJECT_PATH_TITLE: &str = "Remove project path";
pub(crate) const RENAME_PROJECT_TITLE: &str = "Rename project";
pub(crate) const DELETE_PROJECT_TITLE: &str = "Delete project";
pub(crate) const DELETE_TASK_TITLE: &str = "Delete task";
pub(crate) const ADD_LABEL_TITLE: &str = "Add label";
pub(crate) const BROWSE_LABELS_TITLE: &str = "Labels";
pub(crate) const RENAME_LABEL_TITLE: &str = "Rename label";
pub(crate) const DELETE_LABEL_TITLE: &str = "Delete label";

fn add_project_path_input(project: String, value: String) -> OverlayState {
    OverlayState::text_input(
        TextIntent::AddProjectPath {
            project: project.clone(),
        },
        ADD_PROJECT_PATH_TITLE,
        format!("directory path for {project}:"),
        value,
    )
}

fn usage_count(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}

fn label_delete_prompt(label: &str, task_count: usize, series_count: usize) -> String {
    format!(
        "Type {label} to delete this label.\nUsed by: {}, {}",
        usage_count(task_count, "task", "tasks"),
        usage_count(series_count, "recurring series", "recurring series")
    )
}

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

fn task_refs_for_copy(tasks: &[crate::query::TaskListItem], kind: TaskRefKind) -> String {
    tasks
        .iter()
        .map(|task| match kind {
            TaskRefKind::Short => task.display_ref.as_str(),
            TaskRefKind::Durable => task.task.id.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn task_titles_for_copy(tasks: &[crate::query::TaskListItem]) -> String {
    tasks
        .iter()
        .map(|task| task.task.title.as_str())
        .collect::<Vec<_>>()
        .join("\n")
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

    pub(super) fn begin_add_project_path(&mut self) {
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
            PickerIntent::AddProjectPath,
            ADD_PROJECT_PATH_TITLE,
            items,
            false,
        );
    }

    pub(super) fn begin_remove_project_path(&mut self) {
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
            PickerIntent::RemoveProjectPath,
            REMOVE_PROJECT_PATH_TITLE,
            items,
            false,
        );
    }

    pub(super) fn begin_add_label(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::blank_text_input(
            TextIntent::AddLabel,
            ADD_LABEL_TITLE,
            "label name:",
        ));
    }

    pub(super) async fn begin_browse_labels(&mut self) -> Result<()> {
        self.open_label_picker(PickerIntent::BrowseLabels, BROWSE_LABELS_TITLE)
            .await
    }

    pub(super) async fn begin_rename_label(&mut self) -> Result<()> {
        self.open_label_picker(PickerIntent::RenameLabel, RENAME_LABEL_TITLE)
            .await
    }

    pub(super) async fn begin_delete_label(&mut self) -> Result<()> {
        self.open_label_picker(PickerIntent::DeleteLabel, DELETE_LABEL_TITLE)
            .await
    }

    async fn open_label_picker(&mut self, intent: PickerIntent, title: &str) -> Result<()> {
        self.pending_shortcut.clear();
        let items = self
            .store
            .label_usage()
            .await?
            .into_iter()
            .map(|usage| PickerItem {
                label: format!(
                    "{}  {}  {}",
                    usage.name,
                    usage_count(usage.task_count, "task", "tasks"),
                    usage_count(usage.series_count, "recurring series", "recurring series")
                ),
                value: usage.name,
                selected: false,
            })
            .collect();
        self.open_picker_overlay(intent, title, items, false);
        Ok(())
    }

    pub(super) fn submit_browse_label(&mut self, values: Vec<String>) {
        let Some(label) = self.require_picker_value(values, "no matching label") else {
            return;
        };
        self.open_picker_overlay(
            PickerIntent::LabelActions {
                label: label.clone(),
            },
            format!("Label {label}"),
            vec![
                PickerItem {
                    label: "Rename label".to_string(),
                    value: "rename".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "Delete label".to_string(),
                    value: "delete".to_string(),
                    selected: false,
                },
            ],
            false,
        );
    }

    pub(super) async fn submit_label_action(
        &mut self,
        label: String,
        values: Vec<String>,
    ) -> Result<()> {
        match values.first().map(String::as_str) {
            Some("rename") => self.open_rename_label_input(label),
            Some("delete") => self.submit_delete_label_picker(vec![label]).await?,
            _ => self.set_warning("no label action selected"),
        }
        Ok(())
    }

    pub(super) fn submit_rename_label_picker(&mut self, values: Vec<String>) {
        let Some(label) = self.require_picker_value(values, "no matching label") else {
            return;
        };
        self.open_rename_label_input(label);
    }

    fn open_rename_label_input(&mut self, label: String) {
        self.overlay = Some(OverlayState::text_input(
            TextIntent::RenameLabel {
                label: label.clone(),
            },
            RENAME_LABEL_TITLE,
            "new label name:",
            label,
        ));
    }

    pub(super) async fn submit_delete_label_picker(&mut self, values: Vec<String>) -> Result<()> {
        let Some(label) = self.require_picker_value(values, "no matching label") else {
            return Ok(());
        };
        let Some(usage) = self
            .store
            .label_usage()
            .await?
            .into_iter()
            .find(|usage| usage.name == label)
        else {
            self.set_warning("label no longer exists");
            return Ok(());
        };
        self.overlay = Some(OverlayState::text_input(
            TextIntent::ConfirmDeleteLabel {
                label: label.clone(),
                task_count: usage.task_count,
                series_count: usage.series_count,
            },
            DELETE_LABEL_TITLE,
            label_delete_prompt(&label, usage.task_count, usage.series_count),
            String::new(),
        ));
        Ok(())
    }

    pub(super) async fn submit_rename_label(&mut self, label: String, value: String) -> Result<()> {
        match self.store.rename_label(&label, value.clone()).await {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.overlay = Some(OverlayState::text_input(
                    TextIntent::RenameLabel { label },
                    RENAME_LABEL_TITLE,
                    "new label name:",
                    value,
                ));
            }
        }
        Ok(())
    }

    pub(super) fn submit_delete_label_name(
        &mut self,
        label: String,
        task_count: usize,
        series_count: usize,
        value: String,
    ) {
        if value.trim() != label {
            self.set_warning("label name does not match");
            self.overlay = Some(OverlayState::text_input(
                TextIntent::ConfirmDeleteLabel {
                    label: label.clone(),
                    task_count,
                    series_count,
                },
                DELETE_LABEL_TITLE,
                label_delete_prompt(&label, task_count, series_count),
                value,
            ));
            return;
        }
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::DeleteLabel {
                label: label.clone(),
            },
            DELETE_LABEL_TITLE,
            format!("Delete label {label} everywhere it is used?"),
        ));
    }

    pub(super) async fn submit_delete_label(&mut self, label: String) -> Result<()> {
        match self.store.delete_label(&label).await {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => self.set_error(format!("{error:#}")),
        }
        Ok(())
    }

    pub(super) fn begin_delete_task(&mut self) {
        self.pending_shortcut.clear();
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task to edit");
            return;
        };
        if !self.detail.is_active() && !self.marked_task_ids_in_view().is_empty() {
            let count = selection.len();
            self.overlay = Some(OverlayState::confirm(
                ConfirmIntent::DeleteTasks { selection },
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
            ConfirmIntent::DeleteTasks { selection },
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
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task to copy");
            return;
        };
        let value = task_refs_for_copy(selection.targets(), kind);
        let success = if selection.is_single() {
            format!("copied {}", selection.targets()[0].display_ref)
        } else {
            format!("copied {} task refs", selection.len())
        };
        match copy_to_clipboard(&value) {
            Ok(()) => self.set_success(success),
            Err(error) => self.set_error(format!("copy failed: {error}")),
        }
    }

    pub(super) fn copy_selected_task_text(&mut self, kind: TaskCopyKind) {
        if kind == TaskCopyKind::Title {
            let Some(selection) = self.resolve_task_selection() else {
                self.set_info("no selected task to copy");
                return;
            };
            let success = if selection.is_single() {
                "copied task title".to_string()
            } else {
                format!("copied {} task titles", selection.len())
            };
            match copy_to_clipboard(&task_titles_for_copy(selection.targets())) {
                Ok(()) => self.set_success(success),
                Err(error) => self.set_error(format!("copy failed: {error}")),
            }
            return;
        }

        let Some(task) = self.selected_command_task() else {
            self.set_info("no selected task to copy");
            return;
        };
        if kind == TaskCopyKind::Description && task.task.description.is_empty() {
            self.set_info("task description is empty");
            return;
        }
        let value = task_text_for_copy(&task.task.title, &task.task.description, kind);
        let copied = match kind {
            TaskCopyKind::Title => unreachable!("title copies use task selection"),
            TaskCopyKind::Description => "task description",
            TaskCopyKind::TitleAndDescription => "task title and description",
        };
        match copy_to_clipboard(&value) {
            Ok(()) => self.set_success(format!("copied {copied}")),
            Err(error) => self.set_error(format!("copy failed: {error}")),
        }
    }

    pub(super) fn copy_selected_task_notes(&mut self) {
        let Some(task) = self.selected_command_task() else {
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

    pub(super) fn submit_add_project_path_picker(&mut self, values: Vec<String>) {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_add_project_path();
            return;
        };
        let path = env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.overlay = Some(add_project_path_input(project, path));
    }

    pub(super) async fn submit_add_project_path(
        &mut self,
        project: String,
        value: String,
    ) -> Result<()> {
        let path = value.trim();
        if path.is_empty() {
            self.set_warning("project path is required");
            self.overlay = Some(add_project_path_input(project, value));
            return Ok(());
        }
        match self.store.add_project_path(&project, Path::new(path)).await {
            Ok(message) => self.set_success(message),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.overlay = Some(OverlayState::text_input(
                    TextIntent::AddProjectPath { project },
                    ADD_PROJECT_PATH_TITLE,
                    "directory path:",
                    value,
                ));
            }
        }
        Ok(())
    }

    pub(super) async fn submit_remove_project_path_picker(&mut self, values: Vec<String>) {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_remove_project_path();
            return;
        };
        match self.store.project_paths(&project).await {
            Ok(paths) if paths.is_empty() => {
                self.set_warning(format!("project {project} has no path mappings"));
                self.begin_remove_project_path();
            }
            Ok(paths) => {
                let items = paths
                    .into_iter()
                    .map(|item| PickerItem {
                        label: item.path.clone(),
                        value: item.path,
                        selected: false,
                    })
                    .collect();
                self.open_picker_overlay(
                    PickerIntent::RemoveProjectPathValue {
                        project: project.clone(),
                    },
                    format!("{REMOVE_PROJECT_PATH_TITLE}: select path for {project}"),
                    items,
                    false,
                );
            }
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.begin_remove_project_path();
            }
        }
    }

    pub(super) fn submit_remove_project_path_value(
        &mut self,
        project: String,
        values: Vec<String>,
    ) {
        let Some(path) = self.require_picker_value(values, "no matching project path") else {
            self.begin_remove_project_path();
            return;
        };
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::RemoveProjectPath {
                project: project.clone(),
                path: path.clone(),
            },
            REMOVE_PROJECT_PATH_TITLE,
            format!("Remove path {path} from project {project}?"),
        ));
    }

    pub(super) async fn submit_remove_project_path(
        &mut self,
        project: String,
        path: String,
    ) -> Result<()> {
        match self
            .store
            .remove_project_path(&project, Path::new(&path))
            .await
        {
            Ok(message) => self.set_success(message),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.overlay = Some(OverlayState::confirm(
                    ConfirmIntent::RemoveProjectPath {
                        project: project.clone(),
                        path: path.clone(),
                    },
                    REMOVE_PROJECT_PATH_TITLE,
                    format!("Remove path {path} from project {project}?"),
                ));
            }
        }
        Ok(())
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
                id: "note-1".to_string(),
                body: "First note\n- item".to_string(),
                created_at: "2026-07-13T10:00:00Z".to_string(),
            },
            crate::query::TaskNote {
                id: "note-2".to_string(),
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
