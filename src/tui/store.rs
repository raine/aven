use std::time::Instant;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::choices::PRIORITIES;
use crate::labels::list_labels;
use crate::mutation::{cycle_priority, set_deleted, set_status};
use crate::operations::{
    TaskDraft, TaskUpdate, add_note as add_note_operation, create_label_operation,
    create_project_operation, create_task as create_task_operation,
    update_task as update_task_operation,
};
use crate::query::{
    ProjectListItem, SidebarCounts, TaskFilters, TaskListItem, TaskSort, list_project_items,
    list_task_items, sidebar_counts,
};
use crate::tui::overlay::PickerItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MutationMessage {
    pub(crate) message: String,
    pub(crate) selected: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarTarget {
    All,
    Active,
    Todo,
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
            TaskSort::Project => "project",
            TaskSort::Title => "title",
        }
    }

    pub(crate) async fn refresh(&mut self, selected_id: Option<&str>) -> Result<Option<usize>> {
        let mut conn = self.pool.acquire().await?;
        self.projects = list_project_items(&mut conn).await?;
        self.labels = list_labels(&mut conn, None).await?;
        self.counts = sidebar_counts(&mut conn).await?;
        self.tasks = list_task_items(&mut conn, self.filters.clone(), self.sort).await?;
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
        self.active_view = target;
        self.filters.project = None;
        self.filters.status = None;
        match &self.active_view {
            SidebarTarget::All => {}
            SidebarTarget::Active => self.filters.status = Some("active".to_string()),
            SidebarTarget::Todo => self.filters.status = Some("todo".to_string()),
            SidebarTarget::Project(project) => self.filters.project = Some(project.clone()),
        }
        self.refresh(None).await?;
        Ok(())
    }

    pub(crate) async fn accept_search(&mut self, input: &str) -> Result<Option<usize>> {
        self.filters.search = if input.trim().is_empty() {
            None
        } else {
            Some(input.trim().to_string())
        };
        self.refresh(None).await
    }

    pub(crate) fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            TaskSort::Queue => TaskSort::Created,
            TaskSort::Created => TaskSort::Updated,
            TaskSort::Updated => TaskSort::Project,
            TaskSort::Project => TaskSort::Title,
            TaskSort::Title => TaskSort::Queue,
        };
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
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            update_task_operation(
                &mut conn,
                &item.task.id,
                TaskUpdate {
                    priority: Some(priority.to_string()),
                    ..TaskUpdate::default()
                },
            )
            .await?;
            drop(conn);
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: format!("set {} priority={priority}", item.display_ref),
                selected,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn update_title(
        &mut self,
        index: Option<usize>,
        title: String,
    ) -> Result<Option<MutationMessage>> {
        let title = title.trim().to_string();
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            update_task_operation(
                &mut conn,
                &item.task.id,
                TaskUpdate {
                    title: Some(title),
                    ..TaskUpdate::default()
                },
            )
            .await?;
            drop(conn);
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: format!("set {} title", item.display_ref),
                selected,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn update_description(
        &mut self,
        index: Option<usize>,
        description: String,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            update_task_operation(
                &mut conn,
                &item.task.id,
                TaskUpdate {
                    description: Some(description),
                    ..TaskUpdate::default()
                },
            )
            .await?;
            drop(conn);
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: format!("set {} description", item.display_ref),
                selected,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn update_project(
        &mut self,
        index: Option<usize>,
        project: String,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            update_task_operation(
                &mut conn,
                &item.task.id,
                TaskUpdate {
                    project: Some(project),
                    ..TaskUpdate::default()
                },
            )
            .await?;
            drop(conn);
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: format!("set {} project", item.display_ref),
                selected,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn update_labels(
        &mut self,
        index: Option<usize>,
        selected_labels: Vec<String>,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
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
            let mut conn = self.pool.acquire().await?;
            update_task_operation(
                &mut conn,
                &item.task.id,
                TaskUpdate {
                    add_labels,
                    remove_labels,
                    ..TaskUpdate::default()
                },
            )
            .await?;
            drop(conn);
            let selected = self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(MutationMessage {
                message: format!("set {} labels", item.display_ref),
                selected,
            }));
        }
        Ok(None)
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
                label: "Active".to_string(),
                count: self.counts.active,
                target: Some(SidebarTarget::Active),
                section: false,
            },
            SidebarEntry {
                label: "Todo".to_string(),
                count: self.counts.todo,
                target: Some(SidebarTarget::Todo),
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
}
