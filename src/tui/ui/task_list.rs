use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    TableState, Wrap,
};

use super::input::clipped_input_line;
use super::task_display::{description_or_placeholder, labels_display};
use super::truncate::truncate_chars;
use crate::query::TaskListItem;
use crate::queue::{now_seconds, unix_seconds};
use crate::tui::app::{Focus, WidgetState};
use crate::tui::overlay::TextInputView;
use crate::tui::store::{TaskListRenderMode, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, FG_MUTED, INVERSE_FG, SELECTED, SELECTED_INACTIVE,
};
use crate::tui::widgets::{
    age_style, priority_icon, priority_short, status_chip, status_span, title_cell,
};

impl TaskListRenderMode {
    fn uses_queue_age(self) -> bool {
        matches!(self, Self::Queue)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskGroupRow {
    label: &'static str,
    count: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum TaskListRow {
    Group(TaskGroupRow),
    Task { task_index: usize },
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

struct TaskListView {
    rows: Vec<TaskListRow>,
    render_mode: TaskListRenderMode,
}

impl TaskListView {
    fn new(store: &TuiStore) -> Self {
        Self::from_tasks(store.view_state.render_mode(), &store.tasks)
    }

    fn from_tasks(render_mode: TaskListRenderMode, tasks: &[TaskListItem]) -> Self {
        let rows = match render_mode {
            TaskListRenderMode::Queue => queue_rows(tasks),
            TaskListRenderMode::Flat => task_rows(tasks),
        };
        Self { rows, render_mode }
    }

    fn visual_row(&self, selected_task: usize) -> usize {
        self.rows
            .iter()
            .position(|row| {
                matches!(row, TaskListRow::Task { task_index } if *task_index == selected_task)
            })
            .unwrap_or(0)
    }
}

fn task_rows(tasks: &[TaskListItem]) -> Vec<TaskListRow> {
    tasks
        .iter()
        .enumerate()
        .map(|(task_index, _)| TaskListRow::Task { task_index })
        .collect()
}

fn queue_rows(tasks: &[TaskListItem]) -> Vec<TaskListRow> {
    let mut rows = Vec::new();
    let mut index = 0;
    while index < tasks.len() {
        let label = queue_group_label(&tasks[index]);
        let start = index;
        while index < tasks.len() && queue_group_label(&tasks[index]) == label {
            index += 1;
        }
        rows.push(TaskListRow::Group(TaskGroupRow {
            label,
            count: index - start,
        }));
        rows.extend((start..index).map(|task_index| TaskListRow::Task { task_index }));
    }
    rows
}

fn queue_group_label(item: &TaskListItem) -> &'static str {
    match item.task.status.as_str() {
        "done" => "done",
        "canceled" => "canceled",
        _ => item.queue.band.label(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskListAreas {
    table_area: Rect,
    preview_area: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskListHit {
    pub(crate) task_index: usize,
    pub(crate) task_id: String,
    pub(crate) viewport_row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskListHitCandidate {
    task_index: usize,
    viewport_row: u16,
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

fn task_list_hit_in_view(
    view: &TaskListView,
    table_state: &TableState,
    table_area: Rect,
    column: u16,
    row: u16,
) -> Option<TaskListHitCandidate> {
    if column < table_area.x || column >= table_area.x.saturating_add(table_area.width) {
        return None;
    }
    if row <= table_area.y || row >= table_area.y.saturating_add(table_area.height) {
        return None;
    }
    let viewport_rows = table_area.height.saturating_sub(1) as usize;
    if view.rows.len() > viewport_rows
        && column
            == table_area
                .x
                .saturating_add(table_area.width)
                .saturating_sub(1)
    {
        return None;
    }
    if viewport_rows == 0 {
        return None;
    }
    let scroll = task_list_scroll(
        table_state.offset(),
        table_state
            .selected()
            .map(|selected| view.visual_row(selected))
            .unwrap_or(0),
        view.rows.len(),
        viewport_rows,
    );

    let visual_row = row - table_area.y - 1;
    let visual_row = usize::from(visual_row);
    if visual_row >= viewport_rows {
        return None;
    }

    let viewport_rows = task_list_visible_rows(view, scroll, viewport_rows);
    let (_, row) = *viewport_rows.get(visual_row)?;
    let viewport_row = u16::try_from(visual_row).ok()?;
    match row {
        TaskListRow::Task { task_index } => Some(TaskListHitCandidate {
            task_index: *task_index,
            viewport_row,
        }),
        TaskListRow::Group(_) => None,
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

fn task_list_hit(store: &TuiStore, candidate: TaskListHitCandidate) -> Option<TaskListHit> {
    let task_id = store
        .tasks
        .get(candidate.task_index)
        .map(|item| item.task.id.clone())?;
    Some(TaskListHit {
        task_index: candidate.task_index,
        task_id,
        viewport_row: candidate.viewport_row,
    })
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

fn task_list_visible_rows(
    view: &TaskListView,
    scroll: usize,
    viewport_rows: usize,
) -> Vec<(usize, &TaskListRow)> {
    let mut rows = Vec::new();
    if let Some(TaskListRow::Task { .. }) = view.rows.get(scroll)
        && let Some(group @ TaskListRow::Group(_)) = view.rows.get(scroll.saturating_sub(1))
    {
        rows.push((scroll.saturating_sub(1), group));
    }
    rows.extend(
        view.rows
            .iter()
            .enumerate()
            .skip(scroll)
            .take(viewport_rows.saturating_sub(rows.len())),
    );
    rows
}

fn task_list_scroll(
    current_scroll: usize,
    selected_row: usize,
    row_count: usize,
    viewport_rows: usize,
) -> usize {
    if viewport_rows == 0 || row_count <= viewport_rows {
        return 0;
    }
    let max_scroll = row_count - viewport_rows;
    let scroll = current_scroll.min(max_scroll);
    if selected_row <= scroll {
        selected_row
    } else if selected_row >= scroll + viewport_rows {
        selected_row.saturating_sub(viewport_rows.saturating_sub(1))
    } else {
        scroll
    }
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

fn task_list_top_scroll(view: &TaskListView) -> usize {
    match view.rows.first() {
        Some(TaskListRow::Group(_)) => 1,
        _ => 0,
    }
}

fn scrollbar_position(
    scroll: usize,
    row_count: usize,
    viewport_rows: usize,
    top_scroll: usize,
) -> usize {
    if viewport_rows == 0 || row_count <= viewport_rows || scroll <= top_scroll {
        0
    } else {
        scroll.saturating_mul(row_count.saturating_sub(1)) / (row_count - viewport_rows)
    }
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
    if item.notes.is_empty() {
        return Line::from("");
    }
    Line::from(Span::styled(
        "✎",
        Style::new().fg(FG_MUTED).remove_modifier(Modifier::BOLD),
    ))
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
    Line::from(vec![
        Span::styled(
            &item.display_ref,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            &item.task.title,
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn render_task_preview(frame: &mut Frame, store: &TuiStore, selected: Option<usize>, area: Rect) {
    let Some(item) = store.selected_task(selected) else {
        return;
    };
    let labels = labels_display(&item.labels, ", ");
    let text = Text::from(vec![
        task_heading_line(item),
        Line::from(vec![
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
        ]),
        Line::from(vec![
            Span::styled("labels ", Style::new().fg(FG_DIM)),
            Span::styled(labels, Style::new().fg(FG_MUTED)),
        ]),
        Line::from(""),
        Line::from(description_or_placeholder(&item.task.description)),
    ]);
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
    use crate::queue::QueueBand;
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

    fn task_item_with(title: &str, status: &str, band: QueueBand) -> TaskListItem {
        let mut item = task_item(title);
        item.task.title = title.to_string();
        item.task.status = status.to_string();
        item.queue.band = band;
        item
    }

    fn task_id(task: &mut TaskListItem, id: &str) {
        task.task.id = id.to_string();
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

    #[test]
    fn project_filtered_queue_view_groups_by_queue_band() {
        let tasks = vec![
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo medium", "todo", QueueBand::Triage),
            task_item_with("backlog", "backlog", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks);

        assert_eq!(view.render_mode, TaskListRenderMode::Queue);
        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Group(TaskGroupRow {
                    label: "focus",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Group(TaskGroupRow {
                    label: "triage",
                    count: 2,
                }),
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Task { task_index: 2 },
                TaskListRow::Group(TaskGroupRow {
                    label: "later",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 3 },
            ]
        );
    }

    #[test]
    fn queue_view_groups_terminal_statuses_by_status() {
        let tasks = vec![
            task_item_with("backlog", "backlog", QueueBand::Later),
            task_item_with("finished", "done", QueueBand::Later),
            task_item_with("canceled", "canceled", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks);

        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Group(TaskGroupRow {
                    label: "later",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Group(TaskGroupRow {
                    label: "done",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Group(TaskGroupRow {
                    label: "canceled",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 2 },
            ]
        );
    }

    #[test]
    fn non_queue_sort_does_not_emit_duplicate_status_groups() {
        let tasks = vec![
            task_item_with("todo 1", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo 2", "todo", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks);

        assert_eq!(view.render_mode, TaskListRenderMode::Flat);
        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Task { task_index: 2 },
            ]
        );
    }

    #[test]
    fn visual_row_uses_planned_rows() {
        let tasks = vec![
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo medium", "todo", QueueBand::Triage),
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks);

        assert_eq!(view.visual_row(0), 1);
        assert_eq!(view.visual_row(1), 3);
        assert_eq!(view.visual_row(2), 4);
    }

    #[test]
    fn queue_view_keeps_group_header_with_first_visible_task() {
        let tasks = vec![
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo medium", "todo", QueueBand::Triage),
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks);

        assert_eq!(
            task_list_visible_rows(&view, 1, 3),
            vec![
                (
                    0,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "focus",
                        count: 1
                    })
                ),
                (1, &TaskListRow::Task { task_index: 0 }),
                (
                    2,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "triage",
                        count: 2
                    })
                ),
            ]
        );
        assert_eq!(
            task_list_visible_rows(&view, 3, 3),
            vec![
                (
                    2,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "triage",
                        count: 2
                    })
                ),
                (3, &TaskListRow::Task { task_index: 1 }),
                (4, &TaskListRow::Task { task_index: 2 }),
            ]
        );
    }

    #[test]
    fn task_at_position_skips_queue_group_rows() {
        let mut tasks = vec![
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("todo medium", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
        ];
        task_id(&mut tasks[0], "task-1");
        task_id(&mut tasks[1], "task-2");
        task_id(&mut tasks[2], "task-3");
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks);
        let table_area = Rect::new(0, 0, 80, 10);
        let table_state = TableState::default();

        let header_hit = task_list_hit_in_view(
            &view,
            &table_state,
            table_area,
            table_area.x + 1,
            table_area.y + 1,
        );
        assert!(header_hit.is_none());

        let first_task = task_list_hit_in_view(
            &view,
            &table_state,
            table_area,
            table_area.x + 1,
            table_area.y + 2,
        )
        .unwrap();
        assert_eq!(first_task.task_index, 0);
        assert_eq!(first_task.viewport_row, 1);
    }

    #[test]
    fn task_at_position_respects_scroll_position() {
        let mut tasks = Vec::new();
        for index in 0..20 {
            let mut item = task_item(&format!("task {index}"));
            task_id(&mut item, &format!("task-{index:02}"));
            tasks.push(item);
        }
        let view = TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks);
        let mut table_state = TableState::default();
        table_state.select(Some(10));

        let hit = task_list_hit_in_view(&view, &table_state, Rect::new(0, 0, 80, 5), 1, 4).unwrap();
        assert_eq!(hit.task_index, 10);
        assert_eq!(hit.viewport_row, 3);
    }

    #[test]
    fn task_at_position_ignores_scrollbar_column() {
        let tasks = (0..20)
            .map(|index| task_item(&format!("task {index}")))
            .collect::<Vec<_>>();
        let view = TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks);
        let mut table_state = TableState::default();
        table_state.select(Some(10));

        let hit = task_list_hit_in_view(&view, &table_state, Rect::new(0, 0, 80, 5), 79, 4);

        assert!(hit.is_none());
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
    fn upward_selection_from_bottom_keeps_scroll_at_bottom_until_top_edge() {
        assert_eq!(task_list_scroll(6, 9, 10, 4), 6);
        assert_eq!(task_list_scroll(6, 8, 10, 4), 6);
        assert_eq!(task_list_scroll(6, 7, 10, 4), 6);
        assert_eq!(task_list_scroll(6, 6, 10, 4), 6);
        assert_eq!(task_list_scroll(6, 5, 10, 4), 5);
    }

    #[test]
    fn returning_to_first_row_resets_scroll_to_top() {
        assert_eq!(task_list_scroll(1, 0, 10, 4), 0);
        assert_eq!(task_list_scroll(6, 6, 10, 4), 6);
    }

    #[test]
    fn downward_selection_scrolls_after_bottom_edge() {
        assert_eq!(task_list_scroll(0, 0, 10, 4), 0);
        assert_eq!(task_list_scroll(0, 1, 10, 4), 0);
        assert_eq!(task_list_scroll(0, 2, 10, 4), 0);
        assert_eq!(task_list_scroll(0, 3, 10, 4), 0);
        assert_eq!(task_list_scroll(0, 4, 10, 4), 1);
    }

    #[test]
    fn task_list_scroll_clamps_to_valid_rows() {
        assert_eq!(task_list_scroll(6, 2, 3, 4), 0);
        assert_eq!(task_list_scroll(8, 9, 10, 4), 6);
    }

    #[test]
    fn scrollbar_position_maps_max_scroll_to_end() {
        assert_eq!(scrollbar_position(0, 10, 4, 0), 0);
        assert_eq!(scrollbar_position(6, 10, 4, 0), 9);
        assert_eq!(scrollbar_position(0, 3, 4, 0), 0);
    }

    #[test]
    fn grouped_queue_top_scroll_keeps_scrollbar_at_top() {
        assert_eq!(scrollbar_position(1, 10, 4, 1), 0);
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
    fn queue_render_mode_displays_queue_idle_age() {
        let mut item = task_item("queued");
        item.task.created_at = "2026-06-20T00:00:00Z".to_string();
        item.task.queue_activity_at = "1970-01-01T00:00:00Z".to_string();
        item.queue.idle_days = Some(9);
        item.queue.idle_seconds = Some(9 * 86_400);

        let buffer = render_task_row_buffer_with_mode(&item, TaskListRenderMode::Queue, None);
        let rendered = buffer_text(&buffer);

        assert!(rendered.contains("9d"));
        assert!(!rendered.contains("0h"));
    }

    #[test]
    fn queue_render_mode_displays_sub_hour_idle_age_as_minutes() {
        let mut item = task_item("queued");
        item.queue.idle_days = Some(0);
        item.queue.idle_seconds = Some(59 * 60);

        let buffer = render_task_row_buffer_with_mode(&item, TaskListRenderMode::Queue, None);
        let rendered = buffer_text(&buffer);

        assert!(rendered.contains("59m"));
        assert!(!rendered.contains("0h"));
        assert!(!rendered.contains("0m"));
    }

    #[test]
    fn empty_task_view_has_no_rows() {
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &[]);

        assert!(view.rows.is_empty());
        assert_eq!(view.visual_row(0), 0);
    }

    #[test]
    fn compact_age_formats_minutes_hours_days_weeks_and_months() {
        assert_eq!(compact_age(-1), "0m");
        assert_eq!(compact_age(0), "0m");
        assert_eq!(compact_age(59), "0m");
        assert_eq!(compact_age(60), "1m");
        assert_eq!(compact_age(3_599), "59m");
        assert_eq!(compact_age(6 * 3_600), "6h");
        assert_eq!(compact_age(3_600), "1h");
        assert_eq!(compact_age(86_399), "23h");
        assert_eq!(compact_age(13 * 86_400), "13d");
        assert_eq!(compact_age(9 * 7 * 86_400), "9w");
        assert_eq!(compact_age(122 * 86_400), "4mo");
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

        let cells = build_task_row_cells(
            &item,
            0,
            TaskListRenderMode::Flat,
            None,
            &[12, 40, 6, 9, 10, 3, 5],
        );

        assert_eq!(cells.len(), 7);
        assert_eq!(cells[2].to_string(), "✎");
        assert_eq!(cells[3].to_string(), "app ");
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
