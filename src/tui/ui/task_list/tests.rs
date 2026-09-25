pub(super) use super::cells::*;
pub(super) use super::layout::TableLayout;
pub(super) use super::table::*;
pub(super) use super::*;
pub(super) use crate::choices::TaskPriority;
pub(super) use crate::config::TableColumn;
pub(super) use crate::operations::TaskDraft;
pub(super) use crate::query::TaskListItem;
pub(super) use crate::tui::overlay::TextInputKind;
pub(super) use crate::tui::store::{
    ClosedTaskVisibility, TaskListRenderMode, TaskOrder, TaskProjectionOrigin, TaskQuery, TaskScope,
};
pub(super) use crate::tui::test_support::task_list_item;
pub(super) use ratatui::Terminal;
pub(super) use ratatui::backend::TestBackend;
pub(super) use ratatui::layout::{Constraint, Layout, Rect};
pub(super) use ratatui::style::Modifier;
pub(super) use ratatui::widgets::TableState;
pub(super) use std::collections::BTreeSet;

mod columns;
mod empty_state;
mod epics;
mod interaction;

pub(super) fn render_task_row_buffer(
    item: &TaskListItem,
    inline_title_editor: Option<&TextInputView>,
) -> ratatui::buffer::Buffer {
    render_task_row_buffer_with_mode(item, TaskListRenderMode::Flat, inline_title_editor)
}

pub(super) fn render_task_row_buffer_with_mode(
    item: &TaskListItem,
    render_mode: TaskListRenderMode,
    inline_title_editor: Option<&TextInputView>,
) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(80, 1);
    let mut terminal = Terminal::new(backend).unwrap();
    let columns = [
        Constraint::Length(12),
        Constraint::Fill(1),
        Constraint::Length(12),
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Length(6),
    ];
    terminal
        .draw(|frame| {
            let layout = TableLayout::resolve(&columns, &TableColumn::ALL, frame.area().width);
            let column_widths = layout.widths();
            let style = row_style(true, true, false, false, false);
            let cells = build_task_row_cells(
                item,
                TaskTimeContext {
                    now_seconds: 0,
                    render_mode,
                    due_order: false,
                    show_due: true,
                },
                inline_title_editor,
                &column_widths,
                TaskRowState {
                    selected: true,
                    focused: true,
                    marked: false,
                },
                EpicSelectionContext::default(),
            );
            render_task_row_cells(
                frame,
                frame.area(),
                style,
                &layout,
                &cells,
                TaskRowState {
                    selected: true,
                    focused: true,
                    marked: false,
                },
            );
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

pub(super) fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

pub(super) fn render_task_list_buffer(
    store: &TuiStore,
    width: u16,
    height: u16,
) -> ratatui::buffer::Buffer {
    render_task_list_buffer_with_selection(store, width, height, false)
}

pub(super) fn render_task_list_buffer_with_selection(
    store: &TuiStore,
    width: u16,
    height: u16,
    marked: bool,
) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut table_state = TableState::default();
    table_state.select(Some(0));
    let marked_task_ids = if marked {
        BTreeSet::from([store.tasks[0].task.id.clone()])
    } else {
        BTreeSet::new()
    };
    terminal
        .draw(|frame| {
            render_task_list(
                frame,
                store,
                &mut table_state,
                Focus::Tasks,
                frame.area(),
                None,
                &marked_task_ids,
            );
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

pub(super) async fn test_store_with_tasks(tasks: Vec<TaskListItem>) -> TuiStore {
    let dir = tempfile::tempdir().unwrap();
    let (database, _) = crate::test_support::open_database(&dir.path().join("test.db"))
        .await
        .unwrap();
    let mut store = TuiStore::new(database, crate::workspaces::Workspace::default())
        .await
        .unwrap();
    store._test_database_dir = Some(std::sync::Arc::new(dir));
    if !tasks.is_empty() {
        store.create_project("app".to_string()).await.unwrap();
    }
    let labels = tasks
        .iter()
        .flat_map(|item| item.labels.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    for label in labels {
        store.create_label(label).await.unwrap();
    }
    for item in tasks {
        let draft = TaskDraft {
            metadata: Vec::new(),
            title: item.task.title,
            description: item.task.description,
            project: Some("app".to_string()),
            status: item.task.status.as_str().to_string(),
            priority: item.task.priority.as_str().to_string(),
            source: crate::choices::TaskSource::Unknown,
            labels: item.labels,
            available_at: item.task.available_at,
            due_on: None,
            is_epic: false,
        };
        store.create_task(draft, None).await.unwrap();
    }
    store
}

pub(super) fn epic_parent_and_child() -> (TaskListItem, TaskListItem) {
    let parent_id = crate::test_support::task_id("epic-parent");
    let child_id = crate::test_support::task_id("epic-child");
    let mut parent = task_list_item("Ship account recovery");
    parent.task.id = parent_id.clone();
    parent.task.is_epic = true;
    parent.task.updated_at = "2026-06-20T00:00:00Z".to_string();
    parent.epic_rollup = Some(crate::query::EpicRollup {
        total: 5,
        open: 3,
        done: 1,
        canceled: 1,
        blocked: 1,
        overdue: 1,
        ready: 1,
        latest_activity_at: "2026-06-21T00:00:00Z".to_string(),
    });
    parent.epic_children = vec![crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: child_id.clone(),
        display_ref: "APP-CHLD".to_string(),
        title: "Verify recovery email".to_string(),
        status: "active".to_string(),
        priority: "none".to_string(),
        unresolved: true,
    }];

    let mut child = task_list_item("Verify recovery email");
    child.task.id = child_id;
    child.task.status = crate::choices::TaskStatus::Active;
    child.task.updated_at = "2026-06-21T00:00:00Z".to_string();
    child.display_ref = "APP-CHLD".to_string();
    child.epic_parent = Some(crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: parent_id,
        display_ref: "APP-EPIC".to_string(),
        title: "Ship account recovery".to_string(),
        status: "todo".to_string(),
        priority: "none".to_string(),
        unresolved: true,
    });
    (parent, child)
}

pub(super) async fn epic_test_store(expanded: bool) -> TuiStore {
    let mut store = test_store_with_tasks(Vec::new()).await;
    let (parent, child) = epic_parent_and_child();
    if expanded {
        store
            .view_state
            .expanded_epic_ids
            .insert(parent.task.id.clone());
    }
    store.tasks = vec![parent, child].into();
    store.view_state.query = TaskQuery::Epics;
    store
}

pub(super) fn text_in_cell(buffer: &ratatui::buffer::Buffer, area: Rect) -> String {
    (area.x..area.right())
        .map(|x| buffer[(x, area.y)].symbol())
        .collect::<String>()
}
