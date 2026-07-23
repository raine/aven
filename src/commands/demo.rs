use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use aven_core::ids::TaskId;
use aven_core::operations::TaskDraft;
use chrono::{DateTime, Local, NaiveDate, SecondsFormat, TimeDelta, Utc};

use crate::config::AppConfig;
use crate::tui;
use crate::workspaces::Workspace;

use super::demo_data::{DEPENDENCIES, EPIC_LINKS, LABELS, NOTES, PROJECTS, TASKS};

#[derive(Clone, Copy)]
struct DemoClock {
    today: NaiveDate,
    now: DateTime<Utc>,
}

impl DemoClock {
    fn now() -> Self {
        let local_now = Local::now();
        Self {
            today: local_now.date_naive(),
            now: local_now.with_timezone(&Utc),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct DemoSummary {
    projects: usize,
    labels: usize,
    tasks: usize,
    notes: usize,
    dependencies: usize,
    epic_links: usize,
}

pub(crate) async fn cmd_demo(db: Option<PathBuf>, workspace: Option<String>) -> Result<()> {
    if db.is_some() {
        bail!("error demo-isolated option=--db");
    }
    if workspace.is_some() {
        bail!("error demo-isolated option=--workspace");
    }

    run_demo_session(|database, workspace, db_path, config| async move {
        let launch = tui::resolve_launch(&database, &workspace, Default::default()).await?;
        tui::run_demo(database, workspace, launch, db_path, config).await
    })
    .await
}

async fn run_demo_session<R, F, Fut>(runner: F) -> Result<R>
where
    F: FnOnce(Database, Workspace, PathBuf, AppConfig) -> Fut,
    Fut: Future<Output = Result<R>>,
{
    let demo_dir = tempfile::tempdir().context("could not create demo directory")?;
    let db_path = demo_dir.path().join("demo.sqlite");
    let blob_dir = demo_dir.path().join("blobs");
    std::fs::create_dir_all(&blob_dir).context("could not create demo attachment directory")?;

    let database = Database::open(&db_path).await?;
    let workspace = Workspace::default();
    seed_demo(&database, &workspace, DemoClock::now()).await?;

    let mut config = AppConfig::default();
    config.local.db_path = Some(db_path.clone());
    config.local.blob_dir = Some(blob_dir);

    runner(database, workspace, db_path, config).await
}

async fn seed_demo(
    database: &Database,
    workspace: &Workspace,
    clock: DemoClock,
) -> Result<DemoSummary> {
    for project in PROJECTS {
        database.create_project(workspace, project).await?;
    }
    for label in LABELS {
        database.create_label(workspace, label).await?;
    }

    let mut task_ids = HashMap::with_capacity(TASKS.len());
    for task in TASKS {
        let outcome = database
            .create_task(
                workspace,
                TaskDraft {
                    title: task.title.to_string(),
                    description: task.description.to_string(),
                    project: Some(task.project.to_string()),
                    status: task.status.as_str().to_string(),
                    priority: task.priority.as_str().to_string(),
                    labels: task
                        .labels
                        .iter()
                        .map(|label| (*label).to_string())
                        .collect(),
                    available_at: relative_available_at(clock.now, task.available_in_hours)?,
                    due_on: relative_due_on(clock.today, task.due_in_days)?,
                    is_epic: task.is_epic,
                },
            )
            .await?;
        task_ids.insert(task.key, outcome.task.id);
    }

    seed_relationships(database, workspace, &task_ids).await?;

    Ok(DemoSummary {
        projects: PROJECTS.len(),
        labels: LABELS.len(),
        tasks: TASKS.len(),
        notes: NOTES.len(),
        dependencies: DEPENDENCIES.len(),
        epic_links: EPIC_LINKS.len(),
    })
}

async fn seed_relationships(
    database: &Database,
    workspace: &Workspace,
    task_ids: &HashMap<&str, TaskId>,
) -> Result<()> {
    for note in NOTES {
        database
            .add_note(
                workspace,
                task_id(task_ids, note.task)?,
                note.body.to_string(),
            )
            .await?;
    }
    for dependency in DEPENDENCIES {
        database
            .add_task_dependency(
                workspace,
                task_id(task_ids, dependency.task)?,
                task_id(task_ids, dependency.depends_on)?,
            )
            .await?;
    }
    for link in EPIC_LINKS {
        database
            .add_task_to_epic(
                workspace,
                task_id(task_ids, link.child)?,
                task_id(task_ids, link.epic)?,
            )
            .await?;
    }
    Ok(())
}

fn task_id<'a>(task_ids: &'a HashMap<&str, TaskId>, key: &str) -> Result<&'a TaskId> {
    task_ids
        .get(key)
        .with_context(|| format!("error invalid-demo-dataset unknown-task-key={key}"))
}

fn relative_due_on(today: NaiveDate, offset: Option<i64>) -> Result<Option<String>> {
    offset
        .map(|days| {
            today
                .checked_add_signed(TimeDelta::days(days))
                .context("error invalid-demo-dataset due-date-overflow")
                .and_then(|date| crate::time_input::parse_due_on_input(&date.to_string()))
        })
        .transpose()
}

fn relative_available_at(now: DateTime<Utc>, offset: Option<i64>) -> Result<Option<String>> {
    offset
        .map(|hours| {
            now.checked_add_signed(TimeDelta::hours(hours))
                .context("error invalid-demo-dataset available-at-overflow")
                .and_then(|time| {
                    crate::time_input::parse_available_at_input(
                        &time.to_rfc3339_opts(SecondsFormat::Secs, true),
                    )
                })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use aven_core::operations::TaskDraft;
    use aven_core::query::{
        SortDirection, TaskAvailabilityFilter, TaskFilters, TaskQueryMode, TaskSort,
    };
    use chrono::TimeZone;

    use super::*;

    fn fixed_clock() -> DemoClock {
        DemoClock {
            today: NaiveDate::from_ymd_opt(2026, 7, 23).unwrap(),
            now: Utc.with_ymd_and_hms(2026, 7, 23, 12, 0, 0).unwrap(),
        }
    }

    async fn all_tasks(
        database: &Database,
        workspace: &Workspace,
    ) -> Vec<aven_core::query::TaskListItem> {
        database
            .list_task_items(
                &workspace.id,
                TaskFilters::default(),
                TaskQueryMode::Flat,
                TaskSort::Created,
                SortDirection::Asc,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn seed_demo_creates_the_marketing_dataset() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("demo.sqlite"))
            .await
            .unwrap();
        let workspace = Workspace::default();

        let summary = seed_demo(&database, &workspace, fixed_clock())
            .await
            .unwrap();
        assert_eq!(
            summary,
            DemoSummary {
                projects: 6,
                labels: 16,
                tasks: 41,
                notes: 2,
                dependencies: 6,
                epic_links: 24,
            }
        );

        let tasks = all_tasks(&database, &workspace).await;
        assert_eq!(tasks.len(), 41);
        assert_eq!(
            database
                .list_projects(&workspace.id, None)
                .await
                .unwrap()
                .len(),
            6
        );
        assert_eq!(
            database
                .list_labels(&workspace.id, None)
                .await
                .unwrap()
                .len(),
            16
        );

        let scheduling = tasks
            .iter()
            .find(|item| item.task.title == "Add due dates and scheduling")
            .unwrap();
        assert_eq!(scheduling.task.due_on.as_deref(), Some("2026-07-21"));
        assert_eq!(scheduling.notes.len(), 1);
        assert!(scheduling.labels.iter().any(|label| label == "scheduling"));

        let upcoming = tasks
            .iter()
            .find(|item| item.task.title == "Parse natural-language dates when adding tasks")
            .unwrap();
        assert_eq!(
            upcoming.task.available_at.as_deref(),
            Some("2026-07-26T12:00:00Z")
        );
    }

    #[tokio::test]
    async fn seed_demo_preserves_relationships_and_populates_date_views() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("demo.sqlite"))
            .await
            .unwrap();
        let workspace = Workspace::default();
        seed_demo(&database, &workspace, fixed_clock())
            .await
            .unwrap();

        let tasks = all_tasks(&database, &workspace).await;
        assert_eq!(
            tasks
                .iter()
                .map(|item| item.depends_on.len())
                .sum::<usize>(),
            6
        );
        assert_eq!(
            tasks
                .iter()
                .map(|item| item.epic_children.len())
                .sum::<usize>(),
            24
        );
        assert_eq!(tasks.iter().filter(|item| item.task.is_epic).count(), 5);

        let overdue = database
            .list_task_items(
                &workspace.id,
                TaskFilters {
                    overdue_only: true,
                    ..Default::default()
                },
                TaskQueryMode::Flat,
                TaskSort::DueOn,
                SortDirection::Asc,
            )
            .await
            .unwrap();
        assert!(!overdue.is_empty());

        let upcoming = database
            .list_task_items(
                &workspace.id,
                TaskFilters {
                    availability: TaskAvailabilityFilter::Upcoming,
                    ..Default::default()
                },
                TaskQueryMode::Flat,
                TaskSort::AvailableAt,
                SortDirection::Asc,
            )
            .await
            .unwrap();
        assert!(!upcoming.is_empty());

        assert!(
            tasks
                .iter()
                .filter(|item| matches!(item.task.status.as_str(), "inbox" | "backlog"))
                .filter(|item| item.task.priority == aven_core::choices::TaskPriority::None)
                .count()
                >= 4
        );
    }

    #[tokio::test]
    async fn invalid_relationship_key_fails_seed() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("demo.sqlite"))
            .await
            .unwrap();
        let workspace = Workspace::default();
        let task_ids = HashMap::new();

        let error = seed_relationships(&database, &workspace, &task_ids)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "error invalid-demo-dataset unknown-task-key=add_due_dates_and_scheduling"
        );
    }

    #[tokio::test]
    async fn demo_directory_is_removed_after_session() {
        let path = Arc::new(Mutex::new(None));
        let observed = Arc::clone(&path);

        run_demo_session(move |database, _workspace, db_path, config| async move {
            assert!(db_path.exists());
            assert_eq!(config.local.db_path.as_ref(), Some(&db_path));
            assert!(config.local.blob_dir.as_ref().unwrap().exists());
            *observed.lock().unwrap() = Some(db_path.parent().unwrap().to_path_buf());
            drop(database);
            Ok(())
        })
        .await
        .unwrap();

        let path = path.lock().unwrap().clone().unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn demo_starts_fresh_each_time() {
        run_demo_session(|database, workspace, _db_path, _config| async move {
            database
                .create_task(
                    &workspace,
                    TaskDraft {
                        title: "Temporary demo edit".to_string(),
                        description: String::new(),
                        project: Some("cli".to_string()),
                        status: "inbox".to_string(),
                        priority: "none".to_string(),
                        labels: Vec::new(),
                        available_at: None,
                        due_on: None,
                        is_epic: false,
                    },
                )
                .await?;
            assert_eq!(all_tasks(&database, &workspace).await.len(), 42);
            Ok(())
        })
        .await
        .unwrap();

        run_demo_session(|database, workspace, _db_path, _config| async move {
            assert_eq!(all_tasks(&database, &workspace).await.len(), 41);
            Ok(())
        })
        .await
        .unwrap();
    }
}
