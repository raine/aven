mod types;

#[cfg(test)]
mod tests;

use std::time::Instant;

use anyhow::{Context, Result};
use sqlx::SqlitePool;

pub(crate) use types::{ConflictTarget, MutationMessage, SidebarEntry, SidebarTarget};

use crate::choices::PRIORITIES;
use crate::labels::list_labels_in_workspace;
use crate::mutation::{cycle_priority, set_deleted, set_status};
use crate::operations::{
    TaskDraft, TaskUpdate, add_note as add_note_operation, create_label_operation,
    create_project_operation, create_task as create_task_operation, delete_project_operation,
    init_config as init_config_operation, resolve_conflict, show_config as show_config_operation,
    show_config_paths as show_config_paths_operation,
    show_config_status as show_config_status_operation, task_conflicts,
    update_task as update_task_operation,
};
use crate::projects::inferred_project_key_for_add_in_workspace;
use crate::query::{
    ProjectListItem, SidebarCounts, SortDirection, TaskFilters, TaskListItem, TaskSort,
    list_project_items_in_workspace, list_task_items_in_workspace, sidebar_counts_in_workspace,
};
use crate::refs::display_ref;
use crate::tui::overlay::PickerItem;
use crate::undo::{UndoCommand, UndoPayload, task_field_value, task_snapshot};
use crate::workspaces::{
    Workspace, active_workspace, find_workspace, list_workspaces, set_active_workspace,
};

pub(crate) struct TuiStore {
    pool: SqlitePool,
    pub(crate) tasks: Vec<TaskListItem>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) labels: Vec<String>,
    pub(crate) workspaces: Vec<Workspace>,
    pub(crate) active_workspace: Workspace,
    pub(crate) counts: SidebarCounts,
    pub(crate) sidebar_entries: Vec<SidebarEntry>,
    pub(crate) active_view: SidebarTarget,
    pub(crate) filters: TaskFilters,
    pub(crate) sort: TaskSort,
    pub(crate) sort_direction: SortDirection,
    pub(crate) last_refresh: Instant,
}

impl TuiStore {
    pub(crate) async fn new(pool: SqlitePool) -> Result<Self> {
        let mut store = Self {
            pool,
            tasks: Vec::new(),
            projects: Vec::new(),
            labels: Vec::new(),
            workspaces: Vec::new(),
            active_workspace: active_workspace(),
            counts: SidebarCounts::default(),
            sidebar_entries: Vec::new(),
            active_view: SidebarTarget::All,
            filters: TaskFilters::default(),
            sort: TaskSort::Queue,
            sort_direction: SortDirection::Asc,
            last_refresh: Instant::now(),
        };
        store.apply_active_view_filters();
        store.refresh(None).await?;
        Ok(store)
    }

    pub(crate) fn selected_task(&self, selected: Option<usize>) -> Option<&TaskListItem> {
        selected.and_then(|index| self.tasks.get(index))
    }

    pub(crate) fn sort_label(&self) -> &'static str {
        match self.sort {
            TaskSort::Queue => "queue",
            TaskSort::Created => "created",
            TaskSort::Updated => "updated",
            TaskSort::Priority => "priority",
            TaskSort::Project => "project",
            TaskSort::Title => "title",
        }
    }

    fn activate_workspace(&self) {
        set_active_workspace(self.active_workspace.clone());
    }

    pub(crate) async fn refresh(&mut self, selected_id: Option<&str>) -> Result<Option<usize>> {
        let mut conn = self.pool.acquire().await?;
        let workspace_id = self.active_workspace.id.as_str();
        self.workspaces = list_workspaces(&mut conn).await?;
        self.projects = list_project_items_in_workspace(&mut conn, workspace_id).await?;
        self.labels = list_labels_in_workspace(&mut conn, workspace_id, None).await?;
        self.counts = sidebar_counts_in_workspace(&mut conn, workspace_id).await?;
        self.tasks = list_task_items_in_workspace(
            &mut conn,
            workspace_id,
            self.filters.clone(),
            self.sort,
            self.sort_direction,
        )
        .await?;
        self.rebuild_sidebar();
        self.last_refresh = Instant::now();
        Ok(self.restored_task_selection(selected_id))
    }

    pub(crate) fn sidebar_selection(&self) -> Option<usize> {
        self.sidebar_entries
            .iter()
            .position(|entry| entry.target.as_ref() == Some(&self.active_view))
            .or(Some(1))
    }

    pub(crate) async fn apply_sidebar_selection(&mut self, selected: Option<usize>) -> Result<()> {
        let Some(target) = selected
            .and_then(|index| self.sidebar_entries.get(index))
            .and_then(|entry| entry.target.clone())
        else {
            return Ok(());
        };
        self.show_view(target).await?;
        Ok(())
    }

    pub(crate) async fn show_view(&mut self, target: SidebarTarget) -> Result<Option<usize>> {
        self.active_view = target;
        self.apply_active_view_filters();
        self.refresh(None).await
    }

    pub(crate) async fn clear_filters(&mut self) -> Result<Option<usize>> {
        self.active_view = SidebarTarget::All;
        self.filters = TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        };
        self.refresh(None).await
    }

    async fn apply_attribute_filter(
        &mut self,
        setter: impl FnOnce(&mut TaskFilters),
    ) -> Result<Option<usize>> {
        self.active_view = SidebarTarget::All;
        self.filters.include_deleted = false;
        self.filters.conflicts_only = false;
        setter(&mut self.filters);
        self.refresh(None).await
    }

    async fn record_undo(&self, summary: &str, payload: UndoPayload) -> Result<()> {
        self.activate_workspace();
        let workspace_id = crate::workspaces::active_workspace_id();
        let mut conn = self.pool.acquire().await?;
        crate::undo::record_tui_undo(&mut conn, &workspace_id, summary, payload).await?;
        Ok(())
    }

    pub(crate) async fn undo_last(&mut self) -> Result<Option<MutationMessage>> {
        self.activate_workspace();
        let workspace_id = crate::workspaces::active_workspace_id();
        let mut conn = self.pool.acquire().await?;
        let Some(outcome) = crate::undo::apply_latest_tui_undo(&mut conn, &workspace_id).await?
        else {
            return Ok(None);
        };
        drop(conn);

        if let Some(include_deleted) = outcome.include_deleted {
            self.filters.include_deleted = include_deleted;
        }

        let selected = self.refresh(outcome.task_id.as_deref()).await?;
        Ok(Some(MutationMessage {
            message: format!("undid {}", outcome.summary),
            selected,
        }))
    }

    async fn update_selected_task<F>(
        &mut self,
        index: Option<usize>,
        update: TaskUpdate,
        message: F,
    ) -> Result<Option<MutationMessage>>
    where
        F: FnOnce(&TaskListItem) -> String,
    {
        if let Some(item) = self.selected_task(index).cloned() {
            self.activate_workspace();
            let mut conn = self.pool.acquire().await?;
            update_task_operation(&mut conn, &item.task.id, update).await?;
            drop(conn);
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: message(&item),
                selected,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn filter_project(&mut self, project: String) -> Result<Option<usize>> {
        self.apply_attribute_filter(|filters| filters.project = Some(project))
            .await
    }

    pub(crate) async fn filter_label(&mut self, label: String) -> Result<Option<usize>> {
        self.apply_attribute_filter(|filters| filters.label = Some(label))
            .await
    }

    pub(crate) async fn filter_status(&mut self, status: String) -> Result<Option<usize>> {
        self.apply_attribute_filter(|filters| filters.status = Some(status))
            .await
    }

    pub(crate) async fn filter_priority(&mut self, priority: String) -> Result<Option<usize>> {
        self.apply_attribute_filter(|filters| filters.priority = Some(priority))
            .await
    }

    pub(crate) async fn toggle_deleted_filter(&mut self) -> Result<Option<usize>> {
        self.active_view = SidebarTarget::All;
        self.filters.include_deleted = !self.filters.include_deleted;
        self.filters.conflicts_only = false;
        self.refresh(None).await
    }

    fn apply_active_view_filters(&mut self) {
        let search = self.filters.search.clone();
        self.filters = TaskFilters {
            search,
            ..TaskFilters::default()
        };
        match &self.active_view {
            SidebarTarget::All => self.filters.hide_done = true,
            SidebarTarget::Inbox => self.filters.status = Some("inbox".to_string()),
            SidebarTarget::Active => self.filters.status = Some("active".to_string()),
            SidebarTarget::Backlog => self.filters.status = Some("backlog".to_string()),
            SidebarTarget::Todo => self.filters.status = Some("todo".to_string()),
            SidebarTarget::Done => self.filters.status = Some("done".to_string()),
            SidebarTarget::Conflicts => self.filters.conflicts_only = true,
            SidebarTarget::Project(project) => self.filters.project = Some(project.clone()),
        }
    }

    pub(crate) fn status_picker_items(&self, selected: Option<&str>) -> Vec<PickerItem> {
        let selected = selected.unwrap_or_default();
        crate::choices::STATUSES
            .iter()
            .map(|status| PickerItem {
                label: (*status).to_string(),
                value: (*status).to_string(),
                selected: *status == selected,
            })
            .collect()
    }

    pub(crate) async fn accept_search(&mut self, input: &str) -> Result<Option<usize>> {
        self.filters.search = if input.trim().is_empty() {
            None
        } else {
            Some(input.trim().to_string())
        };
        self.refresh(None).await
    }

    pub(crate) fn sort_direction_label(&self) -> &'static str {
        match self.sort_direction {
            SortDirection::Asc => "asc",
            SortDirection::Desc => "desc",
        }
    }

    pub(crate) fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            TaskSort::Queue => TaskSort::Created,
            TaskSort::Created => TaskSort::Updated,
            TaskSort::Updated => TaskSort::Priority,
            TaskSort::Priority => TaskSort::Project,
            TaskSort::Project => TaskSort::Title,
            TaskSort::Title => TaskSort::Queue,
        };
    }

    pub(crate) async fn set_sort(&mut self, sort: TaskSort) -> Result<Option<usize>> {
        self.sort = sort;
        self.refresh(None).await
    }

    pub(crate) async fn reverse_sort(&mut self) -> Result<Option<usize>> {
        self.sort_direction = self.sort_direction.toggled();
        self.refresh(None).await
    }

    pub(crate) async fn update_status(
        &mut self,
        index: Option<usize>,
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let before = item.task.status.clone();
            self.activate_workspace();
            let mut conn = self.pool.acquire().await?;
            set_status(&mut conn, &item.task, status).await?;
            drop(conn);
            self.record_undo(
                &format!("status {}", item.display_ref),
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskField {
                        task_id: item.task.id.clone(),
                        field: "status".to_string(),
                        before,
                        after: status.to_string(),
                    }],
                },
            )
            .await?;
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: format!("set {} status={status}", item.display_ref),
                selected,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn update_priority(
        &mut self,
        index: Option<usize>,
        reverse: bool,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let before = item.task.priority.clone();
            self.activate_workspace();
            let mut conn = self.pool.acquire().await?;
            let task = cycle_priority(&mut conn, &item.task, reverse).await?;
            drop(conn);
            self.record_undo(
                &format!("priority {}", item.display_ref),
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskField {
                        task_id: item.task.id.clone(),
                        field: "priority".to_string(),
                        before,
                        after: task.priority.clone(),
                    }],
                },
            )
            .await?;
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: format!("set {} priority={}", item.display_ref, task.priority),
                selected,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn set_exact_priority(
        &mut self,
        index: Option<usize>,
        priority: &str,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.priority.clone();
        let outcome = self
            .update_selected_task(
                index,
                TaskUpdate {
                    priority: Some(priority.to_string()),
                    ..TaskUpdate::default()
                },
                |item| format!("set {} priority={priority}", item.display_ref),
            )
            .await?;
        if outcome.is_some() {
            self.record_undo(
                &format!("priority {}", item.display_ref),
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskField {
                        task_id: item.task.id.clone(),
                        field: "priority".to_string(),
                        before,
                        after: priority.to_string(),
                    }],
                },
            )
            .await?;
        }
        Ok(outcome)
    }

    pub(crate) async fn update_title(
        &mut self,
        index: Option<usize>,
        title: String,
    ) -> Result<Option<MutationMessage>> {
        let title = title.trim().to_string();
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.title.clone();
        let outcome = self
            .update_selected_task(
                index,
                TaskUpdate {
                    title: Some(title.clone()),
                    ..TaskUpdate::default()
                },
                |item| format!("set {} title", item.display_ref),
            )
            .await?;
        if outcome.is_some() {
            self.record_undo(
                &format!("title {}", item.display_ref),
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskField {
                        task_id: item.task.id.clone(),
                        field: "title".to_string(),
                        before,
                        after: title,
                    }],
                },
            )
            .await?;
        }
        Ok(outcome)
    }

    pub(crate) async fn update_description(
        &mut self,
        index: Option<usize>,
        description: String,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.description.clone();
        let outcome = self
            .update_selected_task(
                index,
                TaskUpdate {
                    description: Some(description.clone()),
                    ..TaskUpdate::default()
                },
                |item| format!("set {} description", item.display_ref),
            )
            .await?;
        if outcome.is_some() {
            self.record_undo(
                &format!("description {}", item.display_ref),
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskField {
                        task_id: item.task.id.clone(),
                        field: "description".to_string(),
                        before,
                        after: description,
                    }],
                },
            )
            .await?;
        }
        Ok(outcome)
    }

    pub(crate) async fn update_project(
        &mut self,
        index: Option<usize>,
        project: String,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.project_key.clone();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = update_task_operation(
            &mut conn,
            &item.task.id,
            TaskUpdate {
                project: Some(project.clone()),
                ..TaskUpdate::default()
            },
        )
        .await?;
        drop(conn);
        self.record_undo(
            &format!("project {}", item.display_ref),
            UndoPayload {
                commands: vec![UndoCommand::SetTaskField {
                    task_id: item.task.id.clone(),
                    field: "project".to_string(),
                    before,
                    after: outcome.task.project_key.clone(),
                }],
            },
        )
        .await?;
        let selected = self.refresh(Some(&item.task.id)).await?;
        Ok(Some(MutationMessage {
            message: format!("set {} project", item.display_ref),
            selected,
        }))
    }

    pub(crate) async fn update_labels(
        &mut self,
        index: Option<usize>,
        selected_labels: Vec<String>,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let add_labels = selected_labels
            .iter()
            .filter(|label| !item.labels.contains(label))
            .cloned()
            .collect::<Vec<_>>();
        let remove_labels = item
            .labels
            .iter()
            .filter(|label| !selected_labels.contains(label))
            .cloned()
            .collect::<Vec<_>>();
        let outcome = self
            .update_selected_task(
                index,
                TaskUpdate {
                    add_labels,
                    remove_labels,
                    ..TaskUpdate::default()
                },
                |item| format!("set {} labels", item.display_ref),
            )
            .await?;
        if outcome.is_some() {
            self.record_undo(
                &format!("labels {}", item.display_ref),
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskLabels {
                        task_id: item.task.id.clone(),
                        before: item.labels.clone(),
                        after: selected_labels,
                    }],
                },
            )
            .await?;
        }
        Ok(outcome)
    }

    pub(crate) async fn update_deleted(
        &mut self,
        index: Option<usize>,
        deleted: bool,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            if item.task.deleted == deleted {
                return Ok(Some(MutationMessage {
                    message: if deleted {
                        format!("already deleted {}", item.display_ref)
                    } else {
                        format!("already restored {}", item.display_ref)
                    },
                    selected: index,
                }));
            }

            let before = if item.task.deleted { "1" } else { "0" };
            self.activate_workspace();
            let mut conn = self.pool.acquire().await?;
            set_deleted(&mut conn, &item.task, deleted).await?;
            drop(conn);
            let summary = if deleted {
                format!("delete {}", item.display_ref)
            } else {
                format!("restore {}", item.display_ref)
            };
            self.record_undo(
                &summary,
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskField {
                        task_id: item.task.id.clone(),
                        field: "deleted".to_string(),
                        before: before.to_string(),
                        after: if deleted { "1" } else { "0" }.to_string(),
                    }],
                },
            )
            .await?;
            self.filters.include_deleted = deleted;
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: if deleted {
                    format!("deleted {} (showing deleted)", item.display_ref)
                } else {
                    format!("restored {}", item.display_ref)
                },
                selected,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn create_project(&mut self, name: String) -> Result<String> {
        let name = name.trim().to_string();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = create_project_operation(&mut conn, &name, None).await?;
        drop(conn);
        let commands = if outcome.created {
            vec![UndoCommand::DeleteCreatedProject {
                project_key: outcome.project.key.clone(),
                create_change_id: outcome.change_id.unwrap_or_default(),
                expected_name: outcome.project.name.clone(),
                expected_prefix: outcome.project.prefix.clone(),
            }]
        } else {
            Vec::new()
        };
        self.record_undo(
            &format!("project {}", outcome.project.key),
            UndoPayload { commands },
        )
        .await?;
        self.refresh(None).await?;
        Ok(format!("created project {}", outcome.project.key))
    }

    pub(crate) async fn delete_project(&mut self, project: &str) -> Result<MutationMessage> {
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = delete_project_operation(&mut conn, &self.active_workspace, project).await?;
        drop(conn);

        if self.active_view == SidebarTarget::Project(outcome.project.key.clone()) {
            self.active_view = SidebarTarget::All;
            self.apply_active_view_filters();
        }
        if self.filters.project.as_deref() == Some(outcome.project.key.as_str()) {
            self.filters.project = None;
        }
        let selected = self.refresh(None).await?;
        let mut message = format!("deleted project {}", outcome.project.key);
        if outcome.config_mapping {
            message.push_str("; config path mappings were left unchanged");
        }
        Ok(MutationMessage { message, selected })
    }

    pub(crate) async fn create_label(&mut self, name: String) -> Result<String> {
        let name = name.trim().to_string();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = create_label_operation(&mut conn, &name).await?;
        drop(conn);
        let commands = if outcome.created {
            vec![UndoCommand::DeleteCreatedLabel {
                label: outcome.name.clone(),
                create_change_id: outcome.change_id.unwrap_or_default(),
            }]
        } else {
            Vec::new()
        };
        self.record_undo(&format!("label {}", outcome.name), UndoPayload { commands })
            .await?;
        let mut conn = self.pool.acquire().await?;
        self.labels = list_labels_in_workspace(&mut conn, &self.active_workspace.id, None).await?;
        Ok(format!("created label {}", outcome.name))
    }

    pub(crate) fn label_picker_items(&self) -> Vec<PickerItem> {
        self.labels
            .iter()
            .map(|label| PickerItem {
                label: label.clone(),
                value: label.clone(),
                selected: false,
            })
            .collect()
    }

    pub(crate) fn existing_project_picker_items(&self, selected: &str) -> Vec<PickerItem> {
        self.projects
            .iter()
            .map(|project| project_picker_item(project, selected))
            .collect()
    }

    pub(crate) async fn inferred_add_project(&self) -> Result<Option<String>> {
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        inferred_project_key_for_add_in_workspace(&mut conn, &self.active_workspace.id).await
    }

    pub(crate) fn project_picker_items(&self, selected: Option<&str>) -> Vec<PickerItem> {
        let selected = selected.unwrap_or_default();
        let inferred_label = self
            .projects
            .iter()
            .find(|project| project.key == selected)
            .map(|project| format!("Infer project ({})", project.key))
            .unwrap_or_else(|| "Infer project".to_string());
        let mut items = vec![PickerItem {
            label: inferred_label,
            value: String::new(),
            selected: selected.is_empty(),
        }];
        items.extend(
            self.projects
                .iter()
                .map(|project| project_picker_item(project, selected)),
        );
        items
    }

    pub(crate) fn priority_picker_items(&self, selected: &str) -> Vec<PickerItem> {
        PRIORITIES
            .iter()
            .map(|priority| PickerItem {
                label: (*priority).to_string(),
                value: (*priority).to_string(),
                selected: *priority == selected,
            })
            .collect()
    }

    pub(crate) fn workspace_picker_items(&self) -> Vec<PickerItem> {
        let selected_key = self
            .workspaces
            .iter()
            .find(|workspace| workspace.key != self.active_workspace.key)
            .map(|workspace| workspace.key.as_str());
        self.workspaces
            .iter()
            .filter(|workspace| workspace.key == self.active_workspace.key)
            .chain(
                self.workspaces
                    .iter()
                    .filter(|workspace| workspace.key != self.active_workspace.key),
            )
            .map(|workspace| workspace_picker_item(workspace, selected_key))
            .collect()
    }

    pub(crate) async fn switch_workspace(
        &mut self,
        key: String,
    ) -> Result<(String, Option<usize>)> {
        let mut conn = self.pool.acquire().await?;
        let workspace = find_workspace(&mut conn, &key)
            .await?
            .with_context(|| format!("workspace not found: {key}"))?;
        drop(conn);
        let name = workspace.name.clone();
        let key = workspace.key.clone();
        self.active_workspace = workspace;
        self.active_view = SidebarTarget::All;
        self.filters = TaskFilters::default();
        self.activate_workspace();
        let selected = self.refresh(None).await?;
        Ok((format!("switched workspace to {key} ({name})"), selected))
    }

    pub(crate) async fn create_task(
        &mut self,
        draft: TaskDraft,
        current_selected_index: Option<usize>,
    ) -> Result<(String, Option<usize>)> {
        let previous_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = create_task_operation(&mut conn, draft).await?;
        let task_id = outcome.task.id.clone();
        let message_ref = display_ref(&mut conn, &outcome.task).await?;
        let workspace_id = crate::workspaces::active_workspace_id();
        let snapshot = task_snapshot(&mut conn, &workspace_id, &task_id).await?;
        drop(conn);
        self.record_undo(
            &format!("task {task_id}"),
            UndoPayload {
                commands: vec![UndoCommand::DeleteCreatedTask {
                    task_id: task_id.clone(),
                    create_change_id: outcome.create_change_id,
                    expected: snapshot,
                }],
            },
        )
        .await?;

        self.refresh(None).await?;
        let created_index = self.tasks.iter().position(|item| item.task.id == task_id);
        if created_index.is_some() {
            return Ok((format!("created task {message_ref}"), created_index));
        }

        let restored = self.restored_task_selection(previous_id.as_deref());
        Ok((
            format!("created task {message_ref} hidden by current filters"),
            restored,
        ))
    }

    pub(crate) async fn add_note_to_task(&mut self, task_id: &str, body: String) -> Result<String> {
        self.activate_workspace();
        let workspace_id = crate::workspaces::active_workspace_id();
        let mut conn = self.pool.acquire().await?;
        let outcome = add_note_operation(&mut conn, task_id, body).await?;
        let note_change_id: String = sqlx::query_scalar(
            "SELECT change_id FROM notes WHERE workspace_id = ? AND id = ? AND task_id = ?",
        )
        .bind(&workspace_id)
        .bind(&outcome.note_id)
        .bind(task_id)
        .fetch_one(&mut *conn)
        .await?;
        drop(conn);
        self.record_undo(
            &format!("note {}", outcome.note_id),
            UndoPayload {
                commands: vec![UndoCommand::DeleteCreatedNote {
                    task_id: task_id.to_string(),
                    note_id: outcome.note_id.clone(),
                    note_add_change_id: note_change_id,
                }],
            },
        )
        .await?;
        Ok(outcome.note_id)
    }

    pub(crate) async fn conflict_targets(
        &self,
        index: Option<usize>,
    ) -> Result<Option<Vec<ConflictTarget>>> {
        let Some(item) = self.selected_task(index) else {
            return Ok(None);
        };
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let details = task_conflicts(&mut conn, &item.task.id, None).await?;
        Ok(Some(
            details
                .into_iter()
                .map(|detail| ConflictTarget {
                    task_id: item.task.id.clone(),
                    display_ref: item.display_ref.clone(),
                    field: detail.field,
                    variant_a: detail.variant_a,
                    local_value: detail.local_value,
                    variant_b: detail.variant_b,
                    remote_value: detail.remote_value,
                })
                .collect(),
        ))
    }

    pub(crate) async fn resolve_conflict_value(
        &mut self,
        target: ConflictTarget,
        value: String,
    ) -> Result<MutationMessage> {
        self.activate_workspace();
        let workspace_id = crate::workspaces::active_workspace_id();
        let mut conn = self.pool.acquire().await?;
        let before =
            task_field_value(&mut conn, &workspace_id, &target.task_id, &target.field).await?;
        let conflict_id =
            crate::undo::conflict_row_id(&mut conn, &workspace_id, &target.task_id, &target.field)
                .await?;
        let outcome = resolve_conflict(&mut conn, &target.task_id, &target.field, &value).await?;
        drop(conn);
        self.record_undo(
            &format!("conflict {} {}", target.display_ref, target.field),
            UndoPayload {
                commands: vec![UndoCommand::RestoreConflictResolution {
                    task_id: target.task_id.clone(),
                    field: target.field.clone(),
                    before,
                    after: value,
                    conflict_id,
                }],
            },
        )
        .await?;
        let selected = self.refresh(Some(&outcome.task.id)).await?;
        Ok(MutationMessage {
            message: format!(
                "resolved {} conflict field={}",
                target.display_ref, outcome.field
            ),
            selected,
        })
    }

    pub(crate) fn next_conflict_flag_index(
        flags: &[bool],
        selected: Option<usize>,
        delta: isize,
    ) -> Option<usize> {
        if flags.is_empty() || !flags.iter().any(|flag| *flag) {
            return None;
        }
        let len = flags.len() as isize;
        let mut current = selected.unwrap_or(0).min(flags.len() - 1) as isize;
        for _ in 0..len {
            current = (current + delta).rem_euclid(len);
            if flags[current as usize] {
                return Some(current as usize);
            }
        }
        None
    }

    pub(crate) fn next_conflict_index(
        &self,
        selected: Option<usize>,
        delta: isize,
    ) -> Option<usize> {
        let flags = self
            .tasks
            .iter()
            .map(|task| task.has_conflict)
            .collect::<Vec<_>>();
        Self::next_conflict_flag_index(&flags, selected, delta)
    }

    fn rebuild_sidebar(&mut self) {
        let mut entries = vec![
            SidebarEntry {
                label: "Smart Views".to_string(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: "Queue".to_string(),
                count: self.counts.all,
                target: Some(SidebarTarget::All),
                section: false,
            },
            SidebarEntry {
                label: "Inbox".to_string(),
                count: self.counts.inbox,
                target: Some(SidebarTarget::Inbox),
                section: false,
            },
            SidebarEntry {
                label: "Active".to_string(),
                count: self.counts.active,
                target: Some(SidebarTarget::Active),
                section: false,
            },
            SidebarEntry {
                label: "Backlog".to_string(),
                count: self.counts.backlog,
                target: Some(SidebarTarget::Backlog),
                section: false,
            },
            SidebarEntry {
                label: "Todo".to_string(),
                count: self.counts.todo,
                target: Some(SidebarTarget::Todo),
                section: false,
            },
            SidebarEntry {
                label: "Done".to_string(),
                count: self.counts.done,
                target: Some(SidebarTarget::Done),
                section: false,
            },
            SidebarEntry {
                label: "Conflicts".to_string(),
                count: self.counts.conflicts,
                target: Some(SidebarTarget::Conflicts),
                section: false,
            },
            SidebarEntry {
                label: String::new(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: "Projects".to_string(),
                count: 0,
                target: None,
                section: true,
            },
        ];
        entries.extend(self.projects.iter().map(|project| SidebarEntry {
            label: if project.inbox_count > 0 {
                format!("{} {}*", project.prefix, project.name)
            } else {
                format!("{} {}", project.prefix, project.name)
            },
            count: project.open_count,
            target: Some(SidebarTarget::Project(project.key.clone())),
            section: false,
        }));
        self.sidebar_entries = entries;
    }

    fn restored_task_selection(&self, selected_id: Option<&str>) -> Option<usize> {
        if self.tasks.is_empty() {
            return None;
        }
        selected_id
            .and_then(|id| self.tasks.iter().position(|item| item.task.id == id))
            .or(Some(0))
    }

    pub(crate) fn config_status_lines(&self) -> Result<Vec<String>> {
        Ok(show_config_status_operation()?.lines)
    }

    pub(crate) fn config_info_lines(&self) -> Result<Vec<String>> {
        let outcome = show_config_operation()?;
        let mut lines = vec![
            format!("config path: {}", outcome.path.display()),
            String::new(),
        ];
        lines.extend(outcome.text.lines().map(str::to_string));
        Ok(lines)
    }

    pub(crate) fn config_path_lines(&self) -> Result<Vec<String>> {
        Ok(show_config_paths_operation()?.lines)
    }

    pub(crate) fn init_config(&self) -> Result<String> {
        let outcome = init_config_operation()?;
        Ok(format!("created config {}", outcome.path.display()))
    }
}

fn project_picker_item(project: &ProjectListItem, selected: &str) -> PickerItem {
    PickerItem {
        label: format!("{} {}", project.prefix, project.name),
        value: project.key.clone(),
        selected: project.key == selected,
    }
}

fn workspace_picker_item(workspace: &Workspace, selected_key: Option<&str>) -> PickerItem {
    PickerItem {
        label: format!("{} ({})", workspace.name, workspace.key),
        value: workspace.key.clone(),
        selected: selected_key.is_some_and(|key| workspace.key == key),
    }
}
