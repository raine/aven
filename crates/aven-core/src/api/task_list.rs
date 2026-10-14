use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use super::*;

/// The key a task list is ordered by. Ties always break on the display ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskListSort {
    Created,
    Updated,
    Priority,
    DueOn,
    Title,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskListDirection {
    Asc,
    Desc,
}

/// One task list read. Values within a facet combine with OR and facets combine
/// with AND; an empty facet does not filter. `text` uses task-search matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskListQuery {
    pub project_id: Option<ProjectId>,
    pub sort: TaskListSort,
    pub direction: TaskListDirection,
    pub statuses: Vec<TaskStatus>,
    pub priorities: Vec<TaskPriority>,
    pub labels: Vec<String>,
    pub text: String,
    pub limit: Option<u32>,
}

impl TaskListQuery {
    pub fn new(sort: TaskListSort, direction: TaskListDirection) -> Self {
        Self {
            project_id: None,
            sort,
            direction,
            statuses: Vec::new(),
            priorities: Vec::new(),
            labels: Vec::new(),
            text: String::new(),
            limit: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskListFacet<T> {
    pub value: T,
    /// Tasks in scope that have this value and match the other facets.
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskList {
    pub rows: Vec<TaskListRow>,
    /// Matching tasks before `limit` is applied.
    pub total: u32,
    pub statuses: Vec<TaskListFacet<TaskStatus>>,
    pub priorities: Vec<TaskListFacet<TaskPriority>>,
    /// Labels on tasks in scope, plus any selected label, by name.
    pub labels: Vec<TaskListFacet<String>>,
}

const STATUS_ORDER: [TaskStatus; 6] = [
    TaskStatus::Inbox,
    TaskStatus::Backlog,
    TaskStatus::Todo,
    TaskStatus::Active,
    TaskStatus::Done,
    TaskStatus::Canceled,
];

const PRIORITY_ORDER: [TaskPriority; 5] = [
    TaskPriority::Urgent,
    TaskPriority::High,
    TaskPriority::Medium,
    TaskPriority::Low,
    TaskPriority::None,
];

const MAX_TEXT_CHARS: usize = 256;

impl Store {
    /// Lists the workspace's tasks, or one project's, with facet counts for the
    /// status, priority and label filters.
    pub async fn task_list(
        &self,
        workspace_id: &WorkspaceId,
        query: TaskListQuery,
    ) -> Result<TaskList, Error> {
        self.workspace(workspace_id).await?;
        let text = query.text.trim();
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(Error::new(
                ErrorCode::Validation,
                "search query is too long".to_string(),
            ));
        }
        let project_key = match &query.project_id {
            Some(project_id) => Some(
                self.database
                    .find_project_by_id(workspace_id, project_id)
                    .await
                    .map_err(Error::from_internal)?
                    .ok_or_else(|| {
                        Error::new(ErrorCode::NotFound, "project not found".to_string())
                    })?
                    .key,
            ),
            None => None,
        };
        let mut items = self
            .database
            .list_task_summary_items(
                workspace_id,
                TaskFilters {
                    project: project_key.clone(),
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Updated,
                SortDirection::Desc,
                None,
            )
            .await
            .map_err(Error::from_internal)?;
        if !text.is_empty() {
            let matches = self
                .database
                .search_task_items(
                    workspace_id,
                    TaskSearchQuery {
                        text: text.to_string(),
                        project: project_key,
                        metadata: Vec::new(),
                        has_metadata: Vec::new(),
                        missing_metadata: Vec::new(),
                        include_deleted: false,
                        limit: usize::MAX,
                    },
                )
                .await
                .map_err(Error::from_internal)?
                .into_iter()
                .map(|result| result.item.task.id)
                .collect::<HashSet<_>>();
            items.retain(|item| matches.contains(&item.task.id));
        }

        let status_matches = |item: &TaskListItem| {
            query.statuses.is_empty() || query.statuses.contains(&item.task.status)
        };
        let priority_matches = |item: &TaskListItem| {
            query.priorities.is_empty() || query.priorities.contains(&item.task.priority)
        };
        let label_matches = |item: &TaskListItem| {
            query.labels.is_empty() || item.labels.iter().any(|label| query.labels.contains(label))
        };

        let statuses = STATUS_ORDER
            .iter()
            .map(|status| TaskListFacet {
                value: *status,
                count: count(items.iter().filter(|item| {
                    item.task.status == *status && priority_matches(item) && label_matches(item)
                })),
            })
            .collect();
        let priorities = PRIORITY_ORDER
            .iter()
            .map(|priority| TaskListFacet {
                value: *priority,
                count: count(items.iter().filter(|item| {
                    item.task.priority == *priority && status_matches(item) && label_matches(item)
                })),
            })
            .collect();
        let mut label_counts = BTreeMap::<String, u32>::new();
        for label in &query.labels {
            label_counts.entry(label.clone()).or_default();
        }
        for item in &items {
            let counted = status_matches(item) && priority_matches(item);
            for label in &item.labels {
                let entry = label_counts.entry(label.clone()).or_default();
                if counted {
                    *entry = entry.saturating_add(1);
                }
            }
        }
        let labels = label_counts
            .into_iter()
            .map(|(value, count)| TaskListFacet { value, count })
            .collect();

        items.retain(|item| status_matches(item) && priority_matches(item) && label_matches(item));
        let mut rows = items.into_iter().map(TaskListRow::from).collect::<Vec<_>>();
        rows.sort_by(|a, b| compare_rows(a, b, query.sort, query.direction));
        let total = count(rows.iter());
        if let Some(limit) = query.limit {
            rows.truncate(limit as usize);
        }
        Ok(TaskList {
            rows,
            total,
            statuses,
            priorities,
            labels,
        })
    }
}

fn count<T>(values: impl Iterator<Item = T>) -> u32 {
    u32::try_from(values.count()).unwrap_or(u32::MAX)
}

fn priority_rank(priority: TaskPriority) -> u8 {
    match priority {
        TaskPriority::None => 0,
        TaskPriority::Low => 1,
        TaskPriority::Medium => 2,
        TaskPriority::High => 3,
        TaskPriority::Urgent => 4,
    }
}

fn compare_rows(
    a: &TaskListRow,
    b: &TaskListRow,
    sort: TaskListSort,
    direction: TaskListDirection,
) -> Ordering {
    let directed = |ordering: Ordering| match direction {
        TaskListDirection::Asc => ordering,
        TaskListDirection::Desc => ordering.reverse(),
    };
    let primary = match sort {
        TaskListSort::Created => directed(a.created_at.cmp(&b.created_at)),
        TaskListSort::Updated => directed(a.updated_at.cmp(&b.updated_at)),
        TaskListSort::Priority => {
            directed(priority_rank(a.priority).cmp(&priority_rank(b.priority)))
        }
        TaskListSort::Title => directed(a.title.to_lowercase().cmp(&b.title.to_lowercase())),
        // Tasks without a due date stay last in both directions.
        TaskListSort::DueOn => match (&a.due_on, &b.due_on) {
            (Some(a), Some(b)) => directed(a.cmp(b)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        },
    };
    primary.then_with(|| a.display_ref.cmp(&b.display_ref))
}
