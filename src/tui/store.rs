use std::time::Instant;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::choices::PRIORITIES;
use crate::labels::list_labels;
use crate::mutation::{cycle_priority, set_deleted, set_status};
use crate::operations::{
    TaskDraft, TaskUpdate, add_note as add_note_operation, create_label_operation,
    create_project_operation, create_task as create_task_operation,
    init_config as init_config_operation, resolve_conflict, show_config as show_config_operation,
    show_config_paths as show_config_paths_operation,
    show_config_status as show_config_status_operation, task_conflicts,
    update_task as update_task_operation,
};
use crate::query::{
    ProjectListItem, SidebarCounts, SortDirection, TaskFilters, TaskListItem, TaskSort,
    list_project_items, list_task_items, sidebar_counts,
};
use crate::tui::overlay::PickerItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MutationMessage {
    pub(crate) message: String,
    pub(crate) selected: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConflictTarget {
    pub(crate) task_id: String,
    pub(crate) display_ref: String,
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarTarget {
    All,
    Inbox,
    Active,
    Backlog,
    Todo,
    Conflicts,
    Project(String),
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarEntry {
    pub(crate) label: String,
    pub(crate) count: i64,
    pub(crate) target: Option<SidebarTarget>,
    pub(crate) section: bool,
}

pub(crate) struct TuiStore {
    pool: SqlitePool,
    pub(crate) tasks: Vec<TaskListItem>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) labels: Vec<String>,
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
            counts: SidebarCounts::default(),
            sidebar_entries: Vec::new(),
            active_view: SidebarTarget::All,
            filters: TaskFilters::default(),
            sort: TaskSort::Queue,
            sort_direction: SortDirection::Asc,
            last_refresh: Instant::now(),
        };
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

    pub(crate) async fn refresh(&mut self, selected_id: Option<&str>) -> Result<Option<usize>> {
        let mut conn = self.pool.acquire().await?;
        self.projects = list_project_items(&mut conn).await?;
        self.labels = list_labels(&mut conn, None).await?;
        self.counts = sidebar_counts(&mut conn).await?;
        self.tasks = list_task_items(
            &mut conn,
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
        self.filters = TaskFilters::default();
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
            SidebarTarget::All => {}
            SidebarTarget::Inbox => self.filters.status = Some("inbox".to_string()),
            SidebarTarget::Active => self.filters.status = Some("active".to_string()),
            SidebarTarget::Backlog => self.filters.status = Some("backlog".to_string()),
            SidebarTarget::Todo => self.filters.status = Some("todo".to_string()),
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
        status: &'static str,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            set_status(&mut conn, &item.task, status).await?;
            drop(conn);
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
            let mut conn = self.pool.acquire().await?;
            let task = cycle_priority(&mut conn, &item.task, reverse).await?;
            drop(conn);
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
        self.update_selected_task(
            index,
            TaskUpdate {
                priority: Some(priority.to_string()),
                ..TaskUpdate::default()
            },
            |item| format!("set {} priority={priority}", item.display_ref),
        )
        .await
    }

    pub(crate) async fn update_title(
        &mut self,
        index: Option<usize>,
        title: String,
    ) -> Result<Option<MutationMessage>> {
        let title = title.trim().to_string();
        self.update_selected_task(
            index,
            TaskUpdate {
                title: Some(title),
                ..TaskUpdate::default()
            },
            |item| format!("set {} title", item.display_ref),
        )
        .await
    }

    pub(crate) async fn update_description(
        &mut self,
        index: Option<usize>,
        description: String,
    ) -> Result<Option<MutationMessage>> {
        self.update_selected_task(
            index,
            TaskUpdate {
                description: Some(description),
                ..TaskUpdate::default()
            },
            |item| format!("set {} description", item.display_ref),
        )
        .await
    }

    pub(crate) async fn update_project(
        &mut self,
        index: Option<usize>,
        project: String,
    ) -> Result<Option<MutationMessage>> {
        self.update_selected_task(
            index,
            TaskUpdate {
                project: Some(project),
                ..TaskUpdate::default()
            },
            |item| format!("set {} project", item.display_ref),
        )
        .await
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
        self.update_selected_task(
            index,
            TaskUpdate {
                add_labels,
                remove_labels,
                ..TaskUpdate::default()
            },
            |item| format!("set {} labels", item.display_ref),
        )
        .await
    }

    pub(crate) async fn update_deleted(
        &mut self,
        index: Option<usize>,
        deleted: bool,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            set_deleted(&mut conn, &item.task, deleted).await?;
            drop(conn);
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
        let mut conn = self.pool.acquire().await?;
        let outcome = create_project_operation(&mut conn, &name, None).await?;
        drop(conn);
        self.refresh(None).await?;
        Ok(format!("created project {}", outcome.project.key))
    }

    pub(crate) async fn create_label(&mut self, name: String) -> Result<String> {
        let name = name.trim().to_string();
        let mut conn = self.pool.acquire().await?;
        let outcome = create_label_operation(&mut conn, &name).await?;
        self.labels = list_labels(&mut conn, None).await?;
        drop(conn);
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
            .map(|project| PickerItem {
                label: format!("{} {}", project.prefix, project.name),
                value: project.key.clone(),
                selected: project.key == selected,
            })
            .collect()
    }

    pub(crate) fn project_picker_items(&self, selected: Option<&str>) -> Vec<PickerItem> {
        let selected = selected.unwrap_or_default();
        let mut items = vec![PickerItem {
            label: "Infer project".to_string(),
            value: String::new(),
            selected: selected.is_empty(),
        }];
        items.extend(self.projects.iter().map(|project| PickerItem {
            label: format!("{} {}", project.prefix, project.name),
            value: project.key.clone(),
            selected: project.key == selected,
        }));
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

    pub(crate) async fn create_task(
        &mut self,
        draft: TaskDraft,
        current_selected_index: Option<usize>,
    ) -> Result<(String, Option<usize>)> {
        let previous_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        let mut conn = self.pool.acquire().await?;
        let outcome = create_task_operation(&mut conn, draft).await?;
        let task_id = outcome.task.id.clone();
        drop(conn);

        self.refresh(None).await?;
        let created_index = self.tasks.iter().position(|item| item.task.id == task_id);
        if created_index.is_some() {
            return Ok((format!("created task {task_id}"), created_index));
        }

        let restored = self.restored_task_selection(previous_id.as_deref());
        Ok((
            format!("created task {task_id} hidden by current filters"),
            restored,
        ))
    }

    pub(crate) async fn add_note_to_task(&mut self, task_id: &str, body: String) -> Result<String> {
        let mut conn = self.pool.acquire().await?;
        let outcome = add_note_operation(&mut conn, task_id, body).await?;
        Ok(outcome.note_id)
    }

    pub(crate) async fn conflict_targets(
        &self,
        index: Option<usize>,
    ) -> Result<Option<Vec<ConflictTarget>>> {
        let Some(item) = self.selected_task(index) else {
            return Ok(None);
        };
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
        let mut conn = self.pool.acquire().await?;
        let outcome = resolve_conflict(&mut conn, &target.task_id, &target.field, &value).await?;
        drop(conn);
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
                label: "All".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> TuiStore {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        TuiStore::new(pool).await.unwrap()
    }

    #[tokio::test]
    async fn create_project_refreshes_sidebar() {
        let mut store = test_store().await;
        store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        assert!(
            store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
        assert!(
            store
                .sidebar_entries
                .iter()
                .any(|entry| entry.label.contains("Mobile App"))
        );
    }

    #[tokio::test]
    async fn create_label_refreshes_label_cache() {
        let mut store = test_store().await;
        store
            .create_label("Needs Review".to_string())
            .await
            .unwrap();

        assert!(store.labels.iter().any(|label| label == "needs-review"));
        assert!(
            store
                .label_picker_items()
                .iter()
                .any(|item| item.value == "needs-review")
        );
    }

    #[tokio::test]
    async fn project_picker_includes_infer_project_and_existing_projects() {
        let mut store = test_store().await;
        store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        let items = store.project_picker_items(None);
        assert_eq!(items[0].label, "Infer project");
        assert!(items[0].selected);
        assert!(items.iter().any(|item| item.value == "mobile-app"));
    }

    #[tokio::test]
    async fn priority_picker_includes_all_priorities() {
        let store = test_store().await;
        let items = store.priority_picker_items("none");
        assert_eq!(items.len(), PRIORITIES.len());
        assert!(items[0].selected);
    }

    #[tokio::test]
    async fn create_task_refreshes_and_selects_visible_task() {
        let mut store = test_store().await;
        store
            .create_label("needs-review".to_string())
            .await
            .unwrap();
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Write docs".to_string(),
                    description: "details".to_string(),
                    project: None,
                    priority: "high".to_string(),
                    labels: vec!["needs-review".to_string()],
                },
                None,
            )
            .await
            .unwrap();

        let selected = selected.unwrap();
        let task = &store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.priority, "high");
        assert!(task.labels.iter().any(|label| label == "needs-review"));
    }

    #[tokio::test]
    async fn create_task_reports_hidden_by_filters() {
        let mut store = test_store().await;
        store.filters.status = Some("todo".to_string());
        let (message, selected) = store
            .create_task(
                TaskDraft {
                    title: "Inbox task".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();

        assert!(selected.is_none());
        assert!(message.contains("hidden by current filters"));
    }

    #[tokio::test]
    async fn create_task_preserves_previous_selection_when_hidden() {
        let mut store = test_store().await;
        let (_, first_selected) = store
            .create_task(
                TaskDraft {
                    title: "Todo task".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        let first_selected = first_selected.unwrap();
        let task_id = store.tasks[first_selected].task.id.clone();
        store
            .update_status(Some(first_selected), "todo")
            .await
            .unwrap();
        store.filters.status = Some("todo".to_string());
        let current_index = store.refresh(Some(&task_id)).await.unwrap();

        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Hidden inbox task".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                current_index,
            )
            .await
            .unwrap();

        assert_eq!(selected, current_index);
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].task.title, "Todo task");
    }

    #[tokio::test]
    async fn add_note_to_task_writes_note() {
        let mut store = test_store().await;
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Note target".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        let task_id = store.tasks[selected.unwrap()].task.id.clone();
        let note_id = store
            .add_note_to_task(&task_id, "hello note".to_string())
            .await
            .unwrap();
        assert!(!note_id.is_empty());
    }

    #[tokio::test]
    async fn update_task_fields_refresh_selected_task() {
        let mut store = test_store().await;
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Old".to_string(),
                    description: "old body".to_string(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();

        store
            .update_title(Some(selected), "New".to_string())
            .await
            .unwrap();
        store
            .update_description(Some(selected), "new body".to_string())
            .await
            .unwrap();
        store
            .set_exact_priority(Some(selected), "urgent")
            .await
            .unwrap();

        let task = &store.tasks[selected].task;
        assert_eq!(task.title, "New");
        assert_eq!(task.description, "new body");
        assert_eq!(task.priority, "urgent");
    }

    #[tokio::test]
    async fn update_labels_adds_and_removes_labels() {
        let mut store = test_store().await;
        store.create_label("bug".to_string()).await.unwrap();
        store.create_label("docs".to_string()).await.unwrap();
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Labels".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: vec!["bug".to_string()],
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();

        store
            .update_labels(Some(selected), vec!["docs".to_string()])
            .await
            .unwrap();

        assert_eq!(store.tasks[selected].labels, vec!["docs".to_string()]);
    }

    #[test]
    fn next_conflict_flag_index_wraps_forward() {
        let flags = vec![false, true, false, true];
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(1), 1),
            Some(3)
        );
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(3), 1),
            Some(1)
        );
    }

    #[test]
    fn next_conflict_flag_index_wraps_backward() {
        let flags = vec![false, true, false, true];
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(3), -1),
            Some(1)
        );
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(1), -1),
            Some(3)
        );
    }

    #[test]
    fn next_conflict_flag_index_returns_none_without_conflicts() {
        let flags = vec![false, false];
        assert!(TuiStore::next_conflict_flag_index(&flags, Some(0), 1).is_none());
    }

    #[test]
    fn next_conflict_flag_index_keeps_single_conflict() {
        let flags = vec![false, true, false];
        assert_eq!(
            TuiStore::next_conflict_flag_index(&flags, Some(1), 1),
            Some(1)
        );
    }

    #[tokio::test]
    async fn resolve_conflict_value_updates_task_and_clears_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(pool.clone()).await.unwrap();
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Before".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();
        let task_id = store.tasks[selected].task.id.clone();
        let display_ref = store.tasks[selected].display_ref.clone();

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES (?, 'title', NULL, 'local title', 'remote title', NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(&task_id)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        store.refresh(Some(&task_id)).await.unwrap();

        store
            .resolve_conflict_value(
                ConflictTarget {
                    task_id,
                    display_ref,
                    field: "title".to_string(),
                    variant_a: "a".to_string(),
                    local_value: "local title".to_string(),
                    variant_b: "b".to_string(),
                    remote_value: "remote title".to_string(),
                },
                "local title".to_string(),
            )
            .await
            .unwrap();

        assert_eq!(store.tasks[selected].task.title, "local title");
        assert!(!store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn resolve_missing_conflict_leaves_task_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(pool.clone()).await.unwrap();
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Stable title".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();
        let task_id = store.tasks[selected].task.id.clone();

        let error = store
            .resolve_conflict_value(
                ConflictTarget {
                    task_id,
                    display_ref: "APP-1".to_string(),
                    field: "title".to_string(),
                    variant_a: "a".to_string(),
                    local_value: "local".to_string(),
                    variant_b: "b".to_string(),
                    remote_value: "remote".to_string(),
                },
                "local".to_string(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("conflict-not-found"));
        assert_eq!(store.tasks[selected].task.title, "Stable title");
    }

    #[tokio::test]
    async fn update_title_returns_conflicted_field_error() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut store = TuiStore::new(pool.clone()).await.unwrap();
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    title: "Conflict".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();
        let task_id = store.tasks[selected].task.id.clone();

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES (?, 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(&task_id)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        let error = store
            .update_title(Some(selected), "blocked".to_string())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("conflicted-field"));
    }

    #[tokio::test]
    async fn existing_project_picker_items_excludes_infer_project() {
        let mut store = test_store().await;
        store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        let items = store.existing_project_picker_items("mobile-app");
        assert!(!items.iter().any(|item| item.label == "Infer project"));
        assert!(items.iter().any(|item| item.value == "mobile-app"));
        assert!(items.iter().any(|item| item.selected));
    }

    #[tokio::test]
    async fn clear_filters_returns_to_all_view() {
        let mut store = test_store().await;
        store.filters.status = Some("todo".to_string());
        store.filters.search = Some("needle".to_string());
        store.active_view = SidebarTarget::Todo;

        store.clear_filters().await.unwrap();

        assert_eq!(store.active_view, SidebarTarget::All);
        assert!(store.filters.status.is_none());
        assert!(store.filters.search.is_none());
    }

    #[tokio::test]
    async fn show_conflicts_view_sets_conflicts_filter() {
        let mut store = test_store().await;

        store.show_view(SidebarTarget::Conflicts).await.unwrap();

        assert_eq!(store.active_view, SidebarTarget::Conflicts);
        assert!(store.filters.conflicts_only);
    }

    #[tokio::test]
    async fn show_todo_view_clears_stale_view_flags_and_preserves_search() {
        let mut store = test_store().await;
        store.filters.include_deleted = true;
        store.filters.conflicts_only = true;
        store.filters.search = Some("needle".to_string());

        store.show_view(SidebarTarget::Todo).await.unwrap();

        assert_eq!(store.filters.status.as_deref(), Some("todo"));
        assert_eq!(store.filters.search.as_deref(), Some("needle"));
        assert!(!store.filters.include_deleted);
        assert!(!store.filters.conflicts_only);
    }

    #[tokio::test]
    async fn filter_actions_reset_active_view() {
        let mut store = test_store().await;
        store.active_view = SidebarTarget::Conflicts;
        store.filters.conflicts_only = true;

        store.filter_status("todo".to_string()).await.unwrap();

        assert_eq!(store.active_view, SidebarTarget::All);
        assert_eq!(store.filters.status.as_deref(), Some("todo"));
        assert!(!store.filters.conflicts_only);
    }

    #[tokio::test]
    async fn toggle_deleted_filter_switches_include_deleted() {
        let mut store = test_store().await;

        store.toggle_deleted_filter().await.unwrap();
        assert_eq!(store.active_view, SidebarTarget::All);
        assert!(store.filters.include_deleted);

        store.toggle_deleted_filter().await.unwrap();
        assert!(!store.filters.include_deleted);
    }

    #[tokio::test]
    async fn set_sort_and_reverse_sort_update_order_state() {
        let mut store = test_store().await;

        store.set_sort(TaskSort::Priority).await.unwrap();
        assert_eq!(store.sort, TaskSort::Priority);
        assert_eq!(store.sort_direction, SortDirection::Asc);

        store.reverse_sort().await.unwrap();
        assert_eq!(store.sort_direction, SortDirection::Desc);
    }
}
