mod config;
mod pickers;
mod sidebar;
mod sort;
mod task_commands;
mod task_creation;
mod types;
mod undo;
mod view;

#[cfg(test)]
mod tests;

use std::time::Instant;

use anyhow::{Context, Result};
use sqlx::SqlitePool;

pub(crate) use types::{ConflictTarget, MutationMessage, SidebarEntry, SidebarTarget};

use crate::labels::list_labels_in_workspace;
use crate::operations::{
    create_label_operation, create_project_operation, delete_project_operation, resolve_conflict,
    task_conflicts,
};
use crate::projects::inferred_project_key_for_add_in_workspace;
use crate::query::{
    ProjectListItem, SidebarCounts, SortDirection, TaskFilters, TaskListItem, TaskSort,
    list_project_items_in_workspace, list_task_items_in_workspace, sidebar_counts_in_workspace,
};
use crate::undo::{UndoCommand, UndoPayload, task_field_value};
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

    pub(crate) async fn inferred_add_project(&self) -> Result<Option<String>> {
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        inferred_project_key_for_add_in_workspace(&mut conn, &self.active_workspace.id).await
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
}
