use anyhow::{Result, bail};
use aven_core::db::Database;

use crate::cli::{TuiArgs, TuiLayoutArg, TuiViewArg};
use crate::ids::TaskId;
use crate::tui::store::{TaskFilterModifiers, TaskLayout, TaskQuery, TaskScope, TaskViewState};
use crate::workspaces::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TuiLaunch {
    pub(crate) view_state: TaskViewState,
    pub(crate) startup: TuiStartup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TuiStartup {
    Browse,
    AddTask { natural: bool },
    AddTaskOnly { natural: bool },
    Detail { task_id: TaskId },
}

impl TuiLaunch {
    pub(crate) async fn resolve(
        database: &Database,
        workspace: &Workspace,
        args: TuiArgs,
    ) -> Result<Self> {
        if let Some(task_ref) = args.task_ref {
            let task = database.resolve_task_ref(workspace, &task_ref).await?;
            return Ok(Self {
                view_state: TaskViewState::for_exact_task(task.id.clone()),
                startup: TuiStartup::Detail { task_id: task.id },
            });
        }

        let scope = match args.project.as_deref() {
            Some("") => {
                crate::projects::inferred_existing_project_key_with_database(database, workspace)
                    .await?
                    .map_or(TaskScope::Workspace, TaskScope::Project)
            }
            Some(project) => TaskScope::Project(
                database
                    .resolve_existing_project(&workspace.id, project)
                    .await?
                    .key,
            ),
            None => TaskScope::Workspace,
        };
        let view = args.view.map_or(TaskQuery::Queue, TaskQuery::from);
        let layout = args.layout.map_or(TaskLayout::List, TaskLayout::from);
        if !view.supports_layout(layout) {
            bail!("{} query does not support columns layout", query_name(view));
        }
        if view == TaskQuery::RecentActions && (args.label.is_some() || args.priority.is_some()) {
            bail!("recent-actions query does not support task filters");
        }
        let label = match args.label {
            Some(label) => Some(
                database
                    .resolve_labels(&workspace.id, std::slice::from_ref(&label))
                    .await?
                    .into_iter()
                    .next()
                    .expect("one label resolves to one label"),
            ),
            None => None,
        };
        let priority = args.priority.map(|priority| priority.as_str().to_string());
        let view_state = TaskViewState {
            scope,
            query: view,
            layout,
            filter_modifiers: TaskFilterModifiers {
                label,
                priority,
                ..TaskFilterModifiers::default()
            },
            ..TaskViewState::default()
        };

        let startup = if args.add_task_only {
            TuiStartup::AddTaskOnly {
                natural: args.natural,
            }
        } else if args.add_task {
            TuiStartup::AddTask {
                natural: args.natural,
            }
        } else {
            TuiStartup::Browse
        };
        Ok(Self {
            view_state,
            startup,
        })
    }
}

impl From<TuiViewArg> for TaskQuery {
    fn from(view: TuiViewArg) -> Self {
        match view {
            TuiViewArg::Queue => Self::Queue,
            TuiViewArg::All => Self::All,
            TuiViewArg::Open => Self::Open,
            TuiViewArg::Inbox => Self::Inbox,
            TuiViewArg::Active => Self::Active,
            TuiViewArg::Backlog => Self::Backlog,
            TuiViewArg::Todo => Self::Todo,
            TuiViewArg::Done => Self::Done,
            TuiViewArg::Upcoming => Self::Upcoming,
            TuiViewArg::Conflicts => Self::Conflicts,
            TuiViewArg::Epics => Self::Epics,
            TuiViewArg::Recurring => Self::Recurring,
            TuiViewArg::RecentActions => Self::RecentActions,
        }
    }
}

impl From<TuiLayoutArg> for TaskLayout {
    fn from(layout: TuiLayoutArg) -> Self {
        match layout {
            TuiLayoutArg::List => Self::List,
            TuiLayoutArg::Columns => Self::Columns,
        }
    }
}

fn query_name(query: TaskQuery) -> &'static str {
    match query {
        TaskQuery::Queue => "queue",
        TaskQuery::All => "all",
        TaskQuery::Open => "open",
        TaskQuery::Inbox => "inbox",
        TaskQuery::Active => "active",
        TaskQuery::Backlog => "backlog",
        TaskQuery::Todo => "todo",
        TaskQuery::Done => "done",
        TaskQuery::Upcoming => "upcoming",
        TaskQuery::Conflicts => "conflicts",
        TaskQuery::Search => "search",
        TaskQuery::Epics => "epics",
        TaskQuery::Recurring => "recurring",
        TaskQuery::RecentActions => "recent-actions",
    }
}

#[cfg(test)]
mod tests {
    use sqlx::{Sqlite, SqliteConnection, pool::PoolConnection};

    use super::*;

    async fn setup() -> (tempfile::TempDir, Database, PoolConnection<Sqlite>) {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("test.sqlite"))
            .await
            .unwrap();
        let conn = aven_core::test_support::acquire(&database).await.unwrap();
        (temp, database, conn)
    }

    fn args() -> TuiArgs {
        TuiArgs {
            task_ref: None,
            view: None,
            layout: None,
            project: None,
            label: None,
            priority: None,
            add_task: false,
            add_task_only: false,
            natural: false,
        }
    }

    async fn seed_project(conn: &mut SqliteConnection) {
        sqlx::query(
            "INSERT INTO projects(
                workspace_id, id, key, name, prefix, created_at, updated_at
             ) VALUES (?, 'ABCDEFGHJKMNPQRS', 'app', 'App', 'APP', 't', 't')",
        )
        .bind(&Workspace::default().id)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn resolves_composed_browse_state() {
        let (_temp, database, mut conn) = setup().await;
        seed_project(&mut conn).await;
        sqlx::query(
            "INSERT INTO labels(workspace_id, name, created_at)
             VALUES (?, 'bug-fix', 't')",
        )
        .bind(&Workspace::default().id)
        .execute(&mut *conn)
        .await
        .unwrap();
        let mut input = args();
        input.project = Some("App".to_string());
        input.view = Some(TuiViewArg::Todo);
        input.label = Some("Bug Fix".to_string());
        input.priority = Some(crate::cli::TuiPriorityArg::High);
        input.add_task = true;
        input.natural = true;
        drop(conn);

        let launch = TuiLaunch::resolve(&database, &Workspace::default(), input)
            .await
            .unwrap();

        assert_eq!(
            launch.view_state.scope,
            TaskScope::Project("app".to_string())
        );
        assert_eq!(launch.view_state.query, TaskQuery::Todo);
        assert_eq!(
            launch.view_state.filter_modifiers.label.as_deref(),
            Some("bug-fix")
        );
        assert_eq!(
            launch.view_state.filter_modifiers.priority.as_deref(),
            Some("high")
        );
        assert_eq!(launch.startup, TuiStartup::AddTask { natural: true });
    }

    #[tokio::test]
    async fn resolves_all_query_with_columns_layout() {
        let (_temp, database, _conn) = setup().await;
        let mut input = args();
        input.view = Some(TuiViewArg::All);
        input.layout = Some(TuiLayoutArg::Columns);

        let launch = TuiLaunch::resolve(&database, &Workspace::default(), input)
            .await
            .unwrap();

        assert_eq!(launch.view_state.query, TaskQuery::All);
        assert_eq!(launch.view_state.layout, TaskLayout::Columns);
    }

    #[tokio::test]
    async fn rejects_incompatible_query_and_layout() {
        let (_temp, database, _conn) = setup().await;
        let mut input = args();
        input.view = Some(TuiViewArg::Queue);
        input.layout = Some(TuiLayoutArg::Columns);

        let error = TuiLaunch::resolve(&database, &Workspace::default(), input)
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "queue query does not support columns layout"
        );
    }

    #[tokio::test]
    async fn resolves_upcoming_browse_view() {
        let (_temp, database, _conn) = setup().await;
        let mut input = args();
        input.view = Some(TuiViewArg::Upcoming);

        let launch = TuiLaunch::resolve(&database, &Workspace::default(), input)
            .await
            .unwrap();

        assert_eq!(launch.view_state.query, TaskQuery::Upcoming);
        assert_eq!(launch.startup, TuiStartup::Browse);
    }

    #[tokio::test]
    async fn resolves_recurring_browse_view() {
        let (_temp, database, _conn) = setup().await;
        let mut input = args();
        input.view = Some(TuiViewArg::Recurring);

        let launch = TuiLaunch::resolve(&database, &Workspace::default(), input)
            .await
            .unwrap();

        assert_eq!(launch.view_state.query, TaskQuery::Recurring);
        assert_eq!(launch.startup, TuiStartup::Browse);
    }

    #[tokio::test]
    async fn resolves_deleted_task_as_singleton_search_detail() {
        let (_temp, database, mut conn) = setup().await;
        seed_project(&mut conn).await;
        let task_id: TaskId = "ABCD000000000000".parse().unwrap();
        sqlx::query(
            "INSERT INTO tasks(
                workspace_id, id, title, description, project_id, status, priority,
                created_at, updated_at, queue_activity_at, deleted
             ) VALUES (?, ?, 'deleted task', '', 'ABCDEFGHJKMNPQRS', 'done', 'none',
                       't', 't', 't', 1)",
        )
        .bind(&Workspace::default().id)
        .bind(&task_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        let mut input = args();
        input.task_ref = Some("APP-ABCD".to_string());
        drop(conn);

        let launch = TuiLaunch::resolve(&database, &Workspace::default(), input)
            .await
            .unwrap();

        assert_eq!(launch.view_state.scope, TaskScope::Workspace);
        assert_eq!(launch.view_state.query, TaskQuery::Search);
        assert_eq!(
            launch.view_state.projection_origin,
            crate::tui::store::TaskProjectionOrigin::ExactTasks(vec![task_id.clone()])
        );
        assert_eq!(launch.startup, TuiStartup::Detail { task_id });
    }

    #[tokio::test]
    async fn rejects_task_filters_for_recent_actions() {
        let (_temp, database, _conn) = setup().await;
        let mut input = args();
        input.view = Some(TuiViewArg::RecentActions);
        input.priority = Some(crate::cli::TuiPriorityArg::Urgent);

        let error = TuiLaunch::resolve(&database, &Workspace::default(), input)
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "recent-actions query does not support task filters"
        );
    }

    #[tokio::test]
    async fn rejects_unknown_label_before_store_construction() {
        let (_temp, database, conn) = setup().await;
        let mut input = args();
        input.label = Some("missing".to_string());
        drop(conn);

        let error = TuiLaunch::resolve(&database, &Workspace::default(), input)
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "unknown label");
    }
}
