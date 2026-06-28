mod hit_test;
mod view_model;

use self::hit_test::{task_list_hit, task_list_hit_in_view};
use self::view_model::{
    TaskGroupRow, TaskListRow, TaskListView, scrollbar_position, task_list_scroll,
    task_list_top_scroll, task_list_visible_rows,
};

pub(crate) use self::hit_test::TaskListHit;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    TableState, Wrap,
};

use super::input::clipped_input_line;
use super::task_display::{description_preview_text, labels_display};
use super::truncate::truncate_chars;
use crate::query::TaskListItem;
use crate::queue::{now_seconds, unix_seconds};
use crate::tui::app::{Focus, WidgetState};
use crate::tui::overlay::TextInputView;
use crate::tui::store::{TaskListRenderMode, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, FG_MUTED, INVERSE_FG, RED, SELECTED,
    SELECTED_INACTIVE,
};
use crate::tui::widgets::{
    age_style, priority_icon, priority_short, status_chip, status_span, title_cell,
};

impl TaskListRenderMode {
    fn uses_queue_age(self) -> bool {
        matches!(self, Self::Queue)
    }
}

#[derive(Debug)]
struct TaskListRenderModel {
    columns: [Constraint; 7],
    row_areas: Vec<Rect>,
    rows: Vec<TaskListRenderRow>,
    scroll: usize,
    row_count: usize,
    viewport_rows: usize,
    top_scroll: usize,
}

#[derive(Debug)]
enum TaskListRenderRow {
    Group(TaskGroupRow),
    Task(TaskListTaskRow),
}

#[derive(Debug)]
struct TaskListTaskRow {
    style: Style,
    cells: Vec<Line<'static>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskListAreas {
    table_area: Rect,
    preview_area: Rect,
}

fn task_list_areas(area: Rect) -> TaskListAreas {
    let [table_area, preview_area] = if area.height >= 24 {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(8)]).areas(area)
    } else {
        [area, Rect::default()]
    };
    TaskListAreas {
        table_area,
        preview_area,
    }
}

pub(crate) fn task_at_position(
    store: &TuiStore,
    table_state: &TableState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<TaskListHit> {
    let table_area = task_list_areas(area).table_area;
    let view = TaskListView::new(store);
    let candidate = task_list_hit_in_view(&view, table_state, table_area, column, row)?;
    task_list_hit(store, candidate)
}

pub(crate) fn task_status_at_position(
    store: &TuiStore,
    table_state: &TableState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<TaskListHit> {
    let table_area = task_list_areas(area).table_area;
    let view = TaskListView::new(store);
    let candidate = task_list_hit_in_view(&view, table_state, table_area, column, row)?;
    let status_area = task_list_status_area(store, table_area, candidate.viewport_row);
    if column < status_area.x || column >= status_area.x.saturating_add(status_area.width) {
        return None;
    }
    task_list_hit(store, candidate)
}

fn task_list_status_area(store: &TuiStore, table_area: Rect, visual_row: u16) -> Rect {
    let columns = task_list_columns(store, table_area.width < 90);
    let row_area = Rect::new(
        table_area.x,
        table_area.y.saturating_add(1).saturating_add(visual_row),
        table_area.width,
        1,
    );
    Layout::horizontal(columns).areas::<7>(row_area)[4]
}

pub(super) fn render_tasks(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    focus: Focus,
    area: Rect,
    inline_title_editor: Option<&TextInputView>,
) {
    let TaskListAreas {
        table_area,
        preview_area,
    } = task_list_areas(area);
    render_task_list(
        frame,
        store,
        &mut widgets.table,
        focus,
        table_area,
        inline_title_editor,
    );
    if preview_area.height > 0 {
        render_task_preview(frame, store, widgets.table.selected(), preview_area);
    }
}

fn render_task_list(
    frame: &mut Frame,
    store: &TuiStore,
    table_state: &mut TableState,
    focus: Focus,
    area: Rect,
    inline_title_editor: Option<&TextInputView>,
) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);
    let model = build_task_list_render_model(store, table_state, focus, area, inline_title_editor);
    if model.row_areas.is_empty() {
        return;
    }

    render_task_header(frame, model.row_areas[0], model.columns);

    for (index, row) in model.rows.iter().enumerate() {
        let Some(row_area) = model.row_areas.get(index + 1).copied() else {
            break;
        };
        match row {
            TaskListRenderRow::Group(group) => {
                render_group_row(frame, group.label, group.count, row_area);
            }
            TaskListRenderRow::Task(row) => {
                render_task_row_from_model(frame, row_area, &model.columns, row);
            }
        }
    }

    render_task_scrollbar(
        frame,
        model.scroll,
        model.row_count,
        model.viewport_rows,
        model.top_scroll,
        area,
    );
}

fn build_task_list_render_model(
    store: &TuiStore,
    table_state: &mut TableState,
    focus: Focus,
    area: Rect,
    inline_title_editor: Option<&TextInputView>,
) -> TaskListRenderModel {
    let row_areas = Layout::vertical(vec![Constraint::Length(1); area.height as usize]).split(area);
    let columns = task_list_columns(store, area.width < 90);
    if row_areas.is_empty() {
        return TaskListRenderModel {
            columns,
            row_areas: row_areas.to_vec(),
            rows: Vec::new(),
            scroll: 0,
            row_count: 0,
            viewport_rows: 0,
            top_scroll: 0,
        };
    }

    let view = TaskListView::new(store);
    let viewport_rows = row_areas.len().saturating_sub(1);
    let selected_task = table_state.selected();
    let selected_row = selected_task
        .map(|selected| view.visual_row(selected))
        .unwrap_or(0);
    let scroll = task_list_scroll(
        table_state.offset(),
        selected_row,
        view.rows.len(),
        viewport_rows,
    );
    *table_state.offset_mut() = scroll;

    let now = now_seconds();
    let column_widths = task_list_column_widths(
        &columns,
        row_areas.get(1).map_or(area.width, |area| area.width),
    );
    let mut rows = Vec::new();
    for (_, row) in task_list_visible_rows(&view, scroll, viewport_rows) {
        match row {
            TaskListRow::Group(group) => rows.push(TaskListRenderRow::Group(*group)),
            TaskListRow::Task { task_index } => {
                let Some(item) = store.tasks.get(*task_index) else {
                    rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                        style: row_style(false, focus == Focus::Tasks),
                        cells: blank_task_row_cells(),
                    }));
                    continue;
                };
                let selected = selected_task == Some(*task_index);
                rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                    style: row_style(selected, focus == Focus::Tasks),
                    cells: build_task_row_cells(
                        item,
                        now,
                        view.render_mode,
                        inline_title_editor.filter(|_| selected),
                        &column_widths,
                    ),
                }));
            }
        }
    }

    TaskListRenderModel {
        columns,
        row_areas: row_areas.to_vec(),
        rows,
        scroll,
        row_count: view.rows.len(),
        viewport_rows,
        top_scroll: task_list_top_scroll(&view),
    }
}

fn task_list_columns(store: &TuiStore, narrow: bool) -> [Constraint; 7] {
    let project_width = project_column_width(store, narrow);
    [
        Constraint::Length(12),
        Constraint::Fill(1),
        Constraint::Length(6),
        Constraint::Length(project_width),
        Constraint::Length(10),
        Constraint::Length(3),
        Constraint::Length(5),
    ]
}

fn task_list_column_widths(columns: &[Constraint; 7], width: u16) -> [usize; 7] {
    if width == 0 {
        return [0; 7];
    }
    let cells = Layout::horizontal(*columns).areas::<7>(Rect::new(0, 0, width, 1));
    [
        cells[0].width as usize,
        cells[1].width as usize,
        cells[2].width as usize,
        cells[3].width as usize,
        cells[4].width as usize,
        cells[5].width as usize,
        cells[6].width as usize,
    ]
}

fn render_task_scrollbar(
    frame: &mut Frame,
    scroll: usize,
    row_count: usize,
    viewport_rows: usize,
    top_scroll: usize,
    area: Rect,
) {
    if viewport_rows == 0 || row_count <= viewport_rows {
        return;
    }
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_symbol("┃")
        .track_symbol(Some("│"))
        .thumb_style(Style::new().fg(ACCENT).bg(BG))
        .track_style(Style::new().fg(BORDER).bg(BG));
    let mut scrollbar_state = ScrollbarState::new(row_count)
        .position(scrollbar_position(
            scroll,
            row_count,
            viewport_rows,
            top_scroll,
        ))
        .viewport_content_length(viewport_rows);
    frame.render_stateful_widget(scrollbar, list_scrollbar_area(area), &mut scrollbar_state);
}

fn list_scrollbar_area(area: Rect) -> Rect {
    Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    }
}

fn project_column_width(store: &TuiStore, narrow: bool) -> u16 {
    let max_width = if narrow { 14 } else { 18 };
    store
        .tasks
        .iter()
        .map(|item| item.task.project_key.chars().count() as u16 + 2)
        .max()
        .unwrap_or(9)
        .max(9)
        .min(max_width)
}

fn render_task_header(frame: &mut Frame, area: Rect, columns: [Constraint; 7]) {
    let cells = Layout::horizontal(columns).areas::<7>(area);
    let style = Style::new()
        .fg(INVERSE_FG)
        .bg(BORDER)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Block::new().style(style), area);
    for (area, label) in cells
        .into_iter()
        .zip([" REF", "TITLE", "", "PROJECT", "STATUS", "P", "IDLE"])
    {
        frame.render_widget(Paragraph::new(label).style(style), area);
    }
}

fn render_group_row(frame: &mut Frame, label: &str, count: usize, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ▸ ", Style::new().fg(ACCENT).bg(BG_ALT)),
            Span::styled(
                format!("{} ({count})", label.to_uppercase()),
                Style::new()
                    .fg(ACCENT)
                    .bg(BG_ALT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::new().bg(BG_ALT)),
        area,
    );
}

fn row_style(selected: bool, focused: bool) -> Style {
    if selected {
        if focused { SELECTED } else { SELECTED_INACTIVE }
    } else {
        Style::new().bg(BG)
    }
}

fn render_task_row_from_model(
    frame: &mut Frame,
    area: Rect,
    columns: &[Constraint; 7],
    row: &TaskListTaskRow,
) {
    render_task_row_cells(frame, area, row.style, columns, &row.cells);
}

fn render_task_row_cells(
    frame: &mut Frame,
    area: Rect,
    style: Style,
    columns: &[Constraint; 7],
    values: &[Line<'static>],
) {
    frame.render_widget(Block::new().style(style), area);
    let areas = Layout::horizontal(columns).areas::<7>(area);
    for (area, value) in areas.into_iter().zip(values) {
        frame.render_widget(Paragraph::new(value.clone()).style(style), area);
    }
}

fn build_task_row_cells(
    item: &TaskListItem,
    now_seconds: i64,
    render_mode: TaskListRenderMode,
    inline_title_editor: Option<&TextInputView>,
    column_widths: &[usize; 7],
) -> Vec<Line<'static>> {
    let age_seconds = if render_mode.uses_queue_age() {
        item.queue.idle_seconds()
    } else {
        task_seconds_since(&item.task.created_at, now_seconds)
    };
    let age_style_input = if render_mode.uses_queue_age() {
        &item.task.queue_activity_at
    } else {
        &item.task.created_at
    };
    let title = inline_title_editor
        .map(|editor| inline_title_edit_cell(editor, column_widths[1]))
        .unwrap_or_else(|| title_cell(item, column_widths[1]));
    vec![
        task_ref_cell(item),
        title,
        metadata_cell(item),
        project_cell(item, column_widths[3]),
        status_chip(&item.task.status),
        Line::from(Span::styled(
            priority_icon(&item.task.priority),
            theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            age_seconds.map(compact_age).unwrap_or_default(),
            age_style(age_style_input, now_seconds),
        )),
    ]
}

fn blank_task_row_cells() -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(""),
    ]
}

fn metadata_cell(item: &TaskListItem) -> Line<'static> {
    let mut spans = Vec::new();
    if item.task.deleted {
        spans.push(Span::styled(
            "×",
            Style::new().fg(RED).add_modifier(Modifier::BOLD),
        ));
    }
    if item.unresolved_blocker_count > 0 {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("←{}", item.unresolved_blocker_count),
            Style::new().fg(FG_MUTED).remove_modifier(Modifier::BOLD),
        ));
    }
    if item.dependent_count > 0 {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("→{}", item.dependent_count),
            Style::new().fg(FG_MUTED).remove_modifier(Modifier::BOLD),
        ));
    }
    if !item.notes.is_empty() {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            "✎",
            Style::new().fg(FG_MUTED).remove_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn inline_title_edit_cell(editor: &TextInputView, max_width: usize) -> Line<'static> {
    clipped_input_line(&editor.input, editor.cursor, max_width.saturating_sub(1))
}

fn task_ref_cell(item: &TaskListItem) -> Line<'static> {
    if let Some((project, suffix)) = item.display_ref.split_once('-') {
        Line::from(vec![
            Span::styled(
                format!(" {project}"),
                Style::new().fg(theme::project_color(&item.task.project_key)),
            ),
            Span::styled("-", Style::new().fg(FG_DIM)),
            Span::styled(suffix.to_string(), Style::new().fg(FG_MUTED)),
        ])
    } else {
        Line::from(Span::styled(
            format!(" {}", item.display_ref),
            Style::new().fg(FG_MUTED),
        ))
    }
}

fn task_seconds_since(value: &str, now_seconds: i64) -> Option<i64> {
    unix_seconds(value).map(|seconds| now_seconds.saturating_sub(seconds).max(0))
}

fn compact_age(age_seconds: i64) -> String {
    let minutes = age_seconds / 60;
    if minutes < 60 {
        return format!("{}m", minutes.max(0));
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    if days < 14 {
        return format!("{days}d");
    }
    let weeks = days / 7;
    if weeks < 13 {
        return format!("{weeks}w");
    }
    format!("{}mo", days / 30)
}

fn project_cell(item: &TaskListItem, max_width: usize) -> Line<'static> {
    let project = truncate_chars(&item.task.project_key, max_width.saturating_sub(1));
    Line::from(vec![
        Span::styled(
            project,
            Style::new().fg(theme::project_color(&item.task.project_key)),
        ),
        Span::raw(" "),
    ])
}

fn task_heading_line(item: &TaskListItem) -> Line<'_> {
    let title_style = if item.task.deleted {
        Style::new()
            .fg(FG_MUTED)
            .add_modifier(Modifier::BOLD | Modifier::CROSSED_OUT)
    } else {
        Style::new().fg(FG).add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(
            &item.display_ref,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(&item.task.title, title_style),
    ])
}

fn task_preview_fields_line(item: &TaskListItem) -> Line<'static> {
    let mut fields = vec![
        Span::styled("project ", Style::new().fg(FG_DIM)),
        Span::styled(
            item.task.project_key.clone(),
            Style::new()
                .fg(theme::project_color(&item.task.project_key))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  status ", Style::new().fg(FG_DIM)),
        status_span(&item.task.status),
        Span::styled("  priority ", Style::new().fg(FG_DIM)),
        Span::styled(
            priority_short(&item.task.priority),
            theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
        ),
    ];
    if item.task.deleted {
        fields.extend([
            Span::styled("  deleted ", Style::new().fg(FG_DIM)),
            Span::styled("yes", Style::new().fg(RED).add_modifier(Modifier::BOLD)),
        ]);
    }
    Line::from(fields)
}

fn dependency_preview_lines(item: &TaskListItem) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !item.depends_on.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("blocked by ", Style::new().fg(FG_DIM)),
            dependency_links_summary(&item.depends_on),
        ]));
    }
    if !item.blocks.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("blocks ", Style::new().fg(FG_DIM)),
            dependency_links_summary(&item.blocks),
        ]));
    }
    lines
}

fn dependency_links_summary(links: &[crate::query::TaskDependencyLink]) -> Span<'static> {
    let summary = links
        .iter()
        .take(3)
        .map(|link| format!("{} {}", link.display_ref, link.title))
        .collect::<Vec<_>>()
        .join(", ");
    let more = links.len().saturating_sub(3);
    let summary = if more > 0 {
        format!("{summary}, +{more}")
    } else {
        summary
    };
    Span::styled(summary, Style::new().fg(FG_MUTED))
}

fn render_task_preview(frame: &mut Frame, store: &TuiStore, selected: Option<usize>, area: Rect) {
    let Some(item) = store.selected_task(selected) else {
        return;
    };
    let labels = labels_display(&item.labels, ", ");
    let mut lines = vec![
        task_heading_line(item),
        task_preview_fields_line(item),
        Line::from(vec![
            Span::styled("labels ", Style::new().fg(FG_DIM)),
            Span::styled(labels, Style::new().fg(FG_MUTED)),
        ]),
    ];
    lines.extend(dependency_preview_lines(item));
    lines.extend([
        Line::from(""),
        Line::from(description_preview_text(&item.task.description)),
    ]);
    let text = Text::from(lines);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::new()
                    .title(" SELECTED ")
                    .borders(Borders::TOP)
                    .border_style(Style::new().fg(BORDER))
                    .padding(Padding::horizontal(1)),
            )
            .wrap(Wrap { trim: false })
            .style(Style::new().fg(FG).bg(BG)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::TaskDraft;
    use crate::tui::overlay::OverlayRoute;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn task_item(title: &str) -> TaskListItem {
        TaskListItem {
            task: crate::types::Task {
                id: "task-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                title: title.to_string(),
                description: String::new(),
                project_id: "project-id".to_string(),
                project_key: "app".to_string(),
                project_prefix: "APP".to_string(),
                status: "todo".to_string(),
                priority: "none".to_string(),
                created_at: "2026-06-20T00:00:00Z".to_string(),
                updated_at: "2026-06-20T00:00:00Z".to_string(),
                queue_activity_at: "2026-06-20T00:00:00Z".to_string(),
                deleted: false,
            },
            display_ref: "APP-1".to_string(),
            labels: Vec::new(),
            notes: Vec::new(),
            has_conflict: false,
            unresolved_blocker_count: 0,
            dependent_count: 0,
            depends_on: Vec::new(),
            blocks: Vec::new(),
            queue: Default::default(),
        }
    }

    fn render_task_row_buffer(
        item: &TaskListItem,
        inline_title_editor: Option<&TextInputView>,
    ) -> ratatui::buffer::Buffer {
        render_task_row_buffer_with_mode(item, TaskListRenderMode::Flat, inline_title_editor)
    }

    fn render_task_row_buffer_with_mode(
        item: &TaskListItem,
        render_mode: TaskListRenderMode,
        inline_title_editor: Option<&TextInputView>,
    ) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let columns = [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(3),
            Constraint::Length(5),
        ];
        terminal
            .draw(|frame| {
                let column_widths = task_list_column_widths(&columns, frame.area().width);
                let cells =
                    build_task_row_cells(item, 0, render_mode, inline_title_editor, &column_widths);
                render_task_row_cells(frame, frame.area(), row_style(true, true), &columns, &cells);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer.content.iter().map(|cell| cell.symbol()).collect()
    }

    async fn test_store_with_tasks(tasks: Vec<TaskListItem>) -> TuiStore {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
            .await
            .unwrap();
        crate::workspaces::set_active_workspace(workspace);
        drop(conn);
        let mut store = TuiStore::new(pool).await.unwrap();
        for item in tasks {
            let draft = TaskDraft {
                title: item.task.title,
                description: item.task.description,
                project: None,
                status: item.task.status,
                priority: item.task.priority,
                labels: Vec::new(),
            };
            store.create_task(draft, None).await.unwrap();
        }
        store
    }

    #[tokio::test]
    async fn task_status_at_position_only_hits_status_column() {
        let store = test_store_with_tasks(vec![task_item("task")]).await;
        let table_state = TableState::default();
        let area = Rect::new(0, 0, 140, 10);
        let task_id = store.tasks[0].task.id.clone();

        let status_area = task_list_status_area(&store, area, 1);
        let hit = task_status_at_position(&store, &table_state, area, status_area.x, 2).unwrap();
        assert_eq!(hit.task_index, 0);
        assert_eq!(hit.task_id, task_id);

        assert!(
            task_status_at_position(&store, &table_state, area, status_area.x - 1, 2).is_none()
        );
        assert!(
            task_status_at_position(
                &store,
                &table_state,
                area,
                status_area.x.saturating_add(status_area.width),
                2
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn task_status_at_position_respects_wide_sidebar_offset() {
        let store = test_store_with_tasks(vec![task_item("task")]).await;
        let table_state = TableState::default();
        let area = Rect::new(26, 2, 114, 18);
        let task_id = store.tasks[0].task.id.clone();

        let status_area = task_list_status_area(&store, area, 1);
        let hit = task_status_at_position(&store, &table_state, area, status_area.x, 4).unwrap();

        assert_eq!(hit.task_index, 0);
        assert_eq!(hit.task_id, task_id);
    }

    #[test]
    fn list_scrollbar_area_skips_header_row() {
        assert_eq!(
            list_scrollbar_area(Rect::new(2, 3, 10, 6)),
            Rect::new(2, 4, 10, 5)
        );
    }

    #[test]
    fn task_scrollbar_draws_on_right_side() {
        let backend = TestBackend::new(5, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_task_scrollbar(frame, 6, 10, 4, 0, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(buffer[(4, 1)].symbol(), "│");
        assert_eq!(buffer[(4, 2)].symbol(), "│");
        assert_eq!(buffer[(4, 3)].symbol(), "│");
        assert_eq!(buffer[(4, 4)].symbol(), "┃");
        assert_eq!(buffer[(4, 5)].symbol(), "┃");
    }

    #[test]
    fn project_cell_truncates_with_status_spacing() {
        let mut item = task_item("Title");
        item.task.project_key = "very-long-project-name".to_string();

        let rendered = project_cell(&item, 10).to_string();

        assert_eq!(rendered, "very-lon… ");
    }

    #[test]
    fn selected_row_renders_inline_title_editor() {
        let item = task_item("original title");
        let editor = TextInputView {
            route: OverlayRoute::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: "edited title".to_string(),
            cursor: 12,
        };

        let buffer = render_task_row_buffer(&item, Some(&editor));
        let rendered = buffer_text(&buffer);

        assert!(rendered.contains("edited title"));
        assert!(!rendered.contains("original title"));
    }

    #[test]
    fn inline_title_editor_draws_end_cursor_in_title_column() {
        let item = task_item("original title");
        let editor = TextInputView {
            route: OverlayRoute::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: "edited".to_string(),
            cursor: 6,
        };

        let buffer = render_task_row_buffer(&item, Some(&editor));

        assert_eq!(buffer[(18, 0)].symbol(), " ");
        assert_eq!(buffer[(18, 0)].style().bg, Some(FG));
    }

    #[test]
    fn normal_row_keeps_title_rendering_without_inline_editor() {
        let item = task_item("original title");

        let buffer = render_task_row_buffer(&item, None);
        let rendered = buffer_text(&buffer);

        assert!(rendered.contains("original title"));
    }

    #[test]
    fn deleted_row_marks_metadata_column_and_keeps_status() {
        let mut item = task_item("original title");
        item.task.deleted = true;

        let buffer = render_task_row_buffer(&item, None);
        let rendered = buffer_text(&buffer);
        let cells = build_task_row_cells(
            &item,
            0,
            TaskListRenderMode::Flat,
            None,
            &[12, 40, 6, 9, 10, 3, 5],
        );

        assert!(rendered.contains("original title"));
        assert!(!rendered.contains("deleted original title"));
        assert_eq!(cells[2].to_string(), "×");
        assert_eq!(cells[4].to_string(), "□ todo");
        assert!(
            task_preview_fields_line(&item)
                .to_string()
                .contains("deleted yes")
        );
    }

    #[test]
    fn inline_title_editor_clips_to_cursor_cell() {
        let editor = TextInputView {
            route: OverlayRoute::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: "abcdef".to_string(),
            cursor: 5,
        };

        let rendered = inline_title_edit_cell(&editor, 5).to_string();

        assert_eq!(rendered, "cdef");
    }

    #[test]
    fn metadata_cell_shows_note_marker() {
        let mut item = task_item("documented");
        item.task.description = "details".to_string();
        item.notes = vec![
            crate::query::TaskNote {
                body: "one".to_string(),
                created_at: "001".to_string(),
            },
            crate::query::TaskNote {
                body: "two".to_string(),
                created_at: "002".to_string(),
            },
        ];

        assert_eq!(metadata_cell(&item).to_string(), "✎");
    }

    #[test]
    fn metadata_cell_shows_dependency_counts() {
        let mut item = task_item("blocked");
        item.unresolved_blocker_count = 2;
        item.dependent_count = 1;

        assert_eq!(metadata_cell(&item).to_string(), "←2 →1");
    }

    #[test]
    fn metadata_cell_ignores_description_without_notes() {
        let mut item = task_item("plain");
        item.task.description = "details".to_string();

        assert_eq!(metadata_cell(&item).to_string(), "");
    }

    #[test]
    fn task_row_cells_insert_metadata_between_title_and_project() {
        let mut item = task_item("documented");
        item.task.description = "details".to_string();
        item.notes = vec![crate::query::TaskNote {
            body: "one".to_string(),
            created_at: "001".to_string(),
        }];
        item.unresolved_blocker_count = 1;
        item.dependent_count = 1;

        let cells = build_task_row_cells(
            &item,
            0,
            TaskListRenderMode::Flat,
            None,
            &[12, 40, 6, 9, 10, 3, 5],
        );

        assert_eq!(cells.len(), 7);
        assert_eq!(cells[2].to_string(), "←1 →1 ✎");
        assert_eq!(cells[3].to_string(), "app ");

        item.task.deleted = true;
        let cells = build_task_row_cells(
            &item,
            0,
            TaskListRenderMode::Flat,
            None,
            &[12, 40, 6, 9, 10, 3, 5],
        );
        assert_eq!(cells[2].to_string(), "× ←1 →1 ✎");
    }

    #[test]
    fn task_row_cells_use_inline_title_when_selected() {
        let item = task_item("original title");
        let editor = TextInputView {
            route: OverlayRoute::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: "edited title".to_string(),
            cursor: 12,
        };

        let cells = build_task_row_cells(
            &item,
            0,
            TaskListRenderMode::Flat,
            Some(&editor),
            &[12, 40, 6, 9, 10, 3, 5],
        );

        assert!(cells[1].to_string().contains("edited title"));
    }
}
