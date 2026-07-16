mod hit_test;
mod view_model;

use std::collections::BTreeSet;

use self::hit_test::{task_list_hit, task_list_hit_in_view};
use self::view_model::{
    TaskGroupRow, TaskListRow, TaskListView, scrollbar_position, task_list_scroll,
    task_list_top_scroll, task_list_visible_rows,
};

pub(crate) use self::hit_test::TaskListHit;

use super::input::clipped_input_line;
use super::task_display::{description_or_placeholder, labels_display};
use super::timestamps::local_timestamp_display;
use super::truncate::truncate_chars;
use crate::query::TaskListItem;
use crate::queue::{now_seconds, unix_seconds};
use crate::tui::app::{Focus, WidgetState};
use crate::tui::markdown::render_markdown_preview;
use crate::tui::overlay::TextInputView;
use crate::tui::store::{TaskListRenderMode, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, FG_MUTED, INVERSE_FG, RED, SELECTED,
    SELECTED_INACTIVE, YELLOW,
};
use crate::tui::widgets::{
    age_style, label_cell, priority_icon, priority_short, status_chip, status_span, title_cell,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, TableState,
};

pub(super) const EPIC_MARKER: &str = "\u{f04ce}";
const EPIC_CHILD_MARKER: &str = "↳";
const DEFERRED_MARKER: &str = "\u{f017}";

#[derive(Debug)]
struct TaskListRenderModel {
    columns: [Constraint; 8],
    row_areas: Vec<Rect>,
    rows: Vec<TaskListRenderRow>,
    scroll: usize,
    row_count: usize,
    viewport_rows: usize,
    top_scroll: usize,
    render_mode: TaskListRenderMode,
    has_deferred_rows: bool,
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
    let preview_height = if area.height >= 32 {
        12
    } else if area.height >= 24 {
        8
    } else {
        0
    };
    let [table_area, preview_area] = if preview_height > 0 {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(preview_height)]).areas(area)
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
    if store.view_state.render_mode() == TaskListRenderMode::Columns {
        return super::columns::column_task_at_position(store, table_state, area, column, row);
    }
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
    if store.view_state.render_mode() == TaskListRenderMode::Columns {
        return None;
    }
    let table_area = task_list_areas(area).table_area;
    let view = TaskListView::new(store);
    let candidate = task_list_hit_in_view(&view, table_state, table_area, column, row)?;
    let status_area = task_list_status_area(store, table_state, table_area, candidate.viewport_row);
    if column < status_area.x || column >= status_area.x.saturating_add(status_area.width) {
        return None;
    }
    task_list_hit(store, candidate)
}

fn task_list_status_area(
    store: &TuiStore,
    table_state: &TableState,
    table_area: Rect,
    visual_row: u16,
) -> Rect {
    let view = TaskListView::new(store);
    let viewport_rows = table_area.height.saturating_sub(1) as usize;
    let selected_row = table_state
        .selected()
        .map(|selected| view.visual_row(selected))
        .unwrap_or(0);
    let scroll = task_list_scroll(table_state.offset(), selected_row, &view, viewport_rows);
    let visible_rows = task_list_visible_rows(&view, scroll, viewport_rows);
    let visible_tasks = visible_task_items(store, &visible_rows);
    let selected_epic_id = table_state
        .selected()
        .and_then(|index| store.tasks.get(index))
        .filter(|item| item.task.is_epic)
        .map(|item| item.task.id.as_str());
    let columns = task_list_columns_for_tasks(
        store,
        table_area.width < 90,
        &visible_tasks,
        selected_epic_id,
    );
    let row_area = Rect::new(
        table_area.x,
        table_area.y.saturating_add(1).saturating_add(visual_row),
        table_area.width,
        1,
    );
    Layout::horizontal(columns).areas::<8>(row_area)[5]
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
        &widgets.marked_task_ids,
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
    marked_task_ids: &BTreeSet<String>,
) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);
    let model = build_task_list_render_model(
        store,
        table_state,
        focus,
        area,
        inline_title_editor,
        marked_task_ids,
    );
    if model.row_areas.is_empty() {
        return;
    }

    render_task_header(
        frame,
        model.row_areas[0],
        model.columns,
        model.render_mode,
        model.has_deferred_rows,
    );

    for (index, row) in model.rows.iter().enumerate() {
        let Some(row_area) = model.row_areas.get(index + 1).copied() else {
            break;
        };
        match row {
            TaskListRenderRow::Group(group) => {
                render_group_row(frame, &group.label, group.count, row_area);
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
    marked_task_ids: &BTreeSet<String>,
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
            render_mode: store.view_state.render_mode(),
            has_deferred_rows: false,
        };
    }

    let view = TaskListView::new(store);
    let viewport_rows = row_areas.len().saturating_sub(1);
    let selected_task = table_state.selected();
    let selected_epic_id = selected_task
        .and_then(|index| store.tasks.get(index))
        .filter(|item| item.task.is_epic)
        .map(|item| item.task.id.as_str());
    let selected_row = selected_task
        .map(|selected| view.visual_row(selected))
        .unwrap_or(0);
    let scroll = task_list_scroll(table_state.offset(), selected_row, &view, viewport_rows);
    *table_state.offset_mut() = scroll;
    let visible_rows = task_list_visible_rows(&view, scroll, viewport_rows);
    let visible_tasks = visible_task_items(store, &visible_rows);
    let columns =
        task_list_columns_for_tasks(store, area.width < 90, &visible_tasks, selected_epic_id);

    let now = now_seconds();
    let has_deferred_rows = view.render_mode == TaskListRenderMode::Flat
        && visible_tasks.iter().any(|item| is_deferred(item, now));
    let column_widths = task_list_column_widths(
        &columns,
        row_areas.get(1).map_or(area.width, |area| area.width),
    );
    let mut rows = Vec::new();
    for (_, row) in visible_rows {
        match row {
            TaskListRow::Group(group) => rows.push(TaskListRenderRow::Group(group.clone())),
            TaskListRow::Task { task_index } => {
                let Some(item) = store.tasks.get(*task_index) else {
                    rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                        style: row_style(false, focus == Focus::Tasks, false),
                        cells: blank_task_row_cells(),
                    }));
                    continue;
                };
                let selected = selected_task == Some(*task_index);
                let marked = marked_task_ids.contains(&item.task.id);
                let style = row_style(selected, focus == Focus::Tasks, marked);
                let cells = if view.render_mode == TaskListRenderMode::Epics && item.task.is_epic {
                    build_epic_parent_row_cells(
                        item,
                        now,
                        store,
                        inline_title_editor.filter(|_| selected),
                        &column_widths,
                        marked,
                        selected_epic_id,
                    )
                } else {
                    build_task_row_cells(
                        item,
                        now,
                        view.render_mode,
                        inline_title_editor.filter(|_| selected),
                        &column_widths,
                        marked,
                        selected_epic_id,
                    )
                };
                rows.push(TaskListRenderRow::Task(TaskListTaskRow { style, cells }));
            }
            TaskListRow::EpicChild {
                parent_index: _,
                task_index,
                last,
            } => {
                let Some(item) = store.tasks.get(*task_index) else {
                    rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                        style: row_style(false, focus == Focus::Tasks, false),
                        cells: blank_task_row_cells(),
                    }));
                    continue;
                };
                let selected = selected_task == Some(*task_index);
                let marked = marked_task_ids.contains(&item.task.id);
                rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                    style: row_style(selected, focus == Focus::Tasks, marked),
                    cells: build_epic_child_row_cells(
                        item,
                        *last,
                        now,
                        &column_widths,
                        marked,
                        selected_epic_id,
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
        render_mode: view.render_mode,
        has_deferred_rows,
    }
}

fn task_list_columns(store: &TuiStore, narrow: bool) -> [Constraint; 8] {
    task_list_columns_for_tasks(store, narrow, &store.tasks.iter().collect::<Vec<_>>(), None)
}

fn task_list_columns_for_tasks(
    store: &TuiStore,
    narrow: bool,
    label_tasks: &[&TaskListItem],
    selected_epic_id: Option<&str>,
) -> [Constraint; 8] {
    let project_width = project_column_width(store, narrow);
    let label_width = label_column_width_from_task_refs(label_tasks, narrow);
    let metadata_width = metadata_column_width_from_task_refs(
        label_tasks,
        selected_epic_id,
        store.view_state.render_mode() == TaskListRenderMode::Flat,
    );
    let priority_width = priority_column_width(store);
    let ref_width = if store.view_state.render_mode() == TaskListRenderMode::Epics {
        14
    } else {
        12
    };
    [
        Constraint::Length(ref_width),
        Constraint::Fill(1),
        Constraint::Length(label_width),
        Constraint::Length(metadata_width),
        Constraint::Length(project_width),
        Constraint::Length(10),
        Constraint::Length(priority_width),
        Constraint::Length(5),
    ]
}

fn task_list_column_widths(columns: &[Constraint; 8], width: u16) -> [usize; 8] {
    if width == 0 {
        return [0; 8];
    }
    let cells = Layout::horizontal(*columns).areas::<8>(Rect::new(0, 0, width, 1));
    [
        cells[0].width as usize,
        cells[1].width as usize,
        cells[2].width as usize,
        cells[3].width as usize,
        cells[4].width as usize,
        cells[5].width as usize,
        cells[6].width as usize,
        cells[7].width as usize,
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

fn visible_task_items<'a>(
    store: &'a TuiStore,
    visible_rows: &[(usize, &TaskListRow)],
) -> Vec<&'a TaskListItem> {
    visible_rows
        .iter()
        .filter_map(|(_, row)| match row {
            TaskListRow::Group(_) => None,
            TaskListRow::Task { task_index } | TaskListRow::EpicChild { task_index, .. } => {
                store.tasks.get(*task_index)
            }
        })
        .collect()
}

#[cfg(test)]
fn label_column_width_from_tasks(tasks: &[TaskListItem], narrow: bool) -> u16 {
    let tasks = tasks.iter().collect::<Vec<_>>();
    label_column_width_from_task_refs(&tasks, narrow)
}

fn label_column_width_from_task_refs(tasks: &[&TaskListItem], narrow: bool) -> u16 {
    if narrow {
        return 0;
    }
    tasks
        .iter()
        .filter(|item| !item.labels.is_empty())
        .map(|item| {
            let first = item.labels.first().map_or(0, |label| label.chars().count());
            let more = item.labels.len().saturating_sub(1);
            let summary_width = if more == 0 {
                first
            } else {
                first + more.to_string().chars().count() + 2
            };
            summary_width as u16 + 2
        })
        .max()
        .unwrap_or(0)
        .min(18)
}

#[cfg(test)]
fn metadata_column_width_from_tasks(tasks: &[TaskListItem]) -> u16 {
    let tasks = tasks.iter().collect::<Vec<_>>();
    metadata_column_width_from_task_refs(&tasks, None, false)
}

fn metadata_column_width_from_task_refs(
    tasks: &[&TaskListItem],
    selected_epic_id: Option<&str>,
    mark_deferred: bool,
) -> u16 {
    let now = now_seconds();
    let width = tasks
        .iter()
        .map(|item| {
            metadata_cell(
                item,
                selected_epic_id,
                mark_deferred && is_deferred(item, now),
            )
            .to_string()
            .chars()
            .count() as u16
        })
        .max()
        .unwrap_or(0);
    if width == 0 { 0 } else { width + 2 }
}

fn priority_column_width(store: &TuiStore) -> u16 {
    priority_column_width_from_tasks(&store.tasks)
}

fn priority_column_width_from_tasks(tasks: &[TaskListItem]) -> u16 {
    if tasks
        .iter()
        .any(|item| item.task.priority.as_str() != "none")
    {
        3
    } else {
        0
    }
}

fn render_task_header(
    frame: &mut Frame,
    area: Rect,
    columns: [Constraint; 8],
    render_mode: TaskListRenderMode,
    has_deferred_rows: bool,
) {
    let cells = Layout::horizontal(columns).areas::<8>(area);
    let style = Style::new()
        .fg(INVERSE_FG)
        .bg(BORDER)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Block::new().style(style), area);
    let time_header = match render_mode {
        TaskListRenderMode::Queue => "IDLE",
        TaskListRenderMode::Upcoming => "WHEN",
        TaskListRenderMode::Flat if has_deferred_rows => "TIME",
        _ => "AGE",
    };
    for (index, (area, label)) in cells
        .into_iter()
        .zip([
            " REF",
            "TITLE",
            "LABELS",
            "",
            "PROJECT",
            "STATUS",
            "P",
            time_header,
        ])
        .enumerate()
    {
        let label = if index == 2 {
            label_header_cell(label, area.width as usize)
        } else {
            Line::from(label)
        };
        frame.render_widget(Paragraph::new(label).style(style), area);
    }
}

fn label_header_cell(label: &str, max_width: usize) -> Line<'static> {
    let label_width = label.chars().count();
    if label_width >= max_width {
        return Line::from(label.to_string());
    }
    let padding = max_width.saturating_sub(label_width + 1);
    Line::from(format!("{}{label} ", " ".repeat(padding)))
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

fn row_style(selected: bool, focused: bool, marked: bool) -> Style {
    if selected {
        if focused { SELECTED } else { SELECTED_INACTIVE }
    } else if marked {
        Style::new().bg(BG_ALT)
    } else {
        Style::new().bg(BG)
    }
}

fn render_task_row_from_model(
    frame: &mut Frame,
    area: Rect,
    columns: &[Constraint; 8],
    row: &TaskListTaskRow,
) {
    render_task_row_cells(frame, area, row.style, columns, &row.cells);
}

fn render_task_row_cells(
    frame: &mut Frame,
    area: Rect,
    style: Style,
    columns: &[Constraint; 8],
    values: &[Line<'static>],
) {
    frame.render_widget(Block::new().style(style), area);
    let areas = Layout::horizontal(columns).areas::<8>(area);
    for (area, value) in areas.into_iter().zip(values) {
        frame.render_widget(Paragraph::new(value.clone()).style(style), area);
    }
}

fn build_task_row_cells(
    item: &TaskListItem,
    now_seconds: i64,
    render_mode: TaskListRenderMode,
    inline_title_editor: Option<&TextInputView>,
    column_widths: &[usize; 8],
    marked: bool,
    selected_epic_id: Option<&str>,
) -> Vec<Line<'static>> {
    let time = task_time_cell(item, now_seconds, render_mode);
    let title = inline_title_editor
        .map(|editor| inline_title_edit_cell(editor, column_widths[1]))
        .unwrap_or_else(|| title_cell(item, column_widths[1]));
    let labels = label_cell(&item.labels, column_widths[2]);
    vec![
        task_ref_cell(item, marked),
        title,
        labels,
        metadata_cell(
            item,
            selected_epic_id,
            render_mode == TaskListRenderMode::Flat && is_deferred(item, now_seconds),
        ),
        project_cell(item, column_widths[4]),
        status_chip(item.task.status.as_str()),
        Line::from(Span::styled(
            priority_icon(item.task.priority.as_str()),
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        )),
        time,
    ]
}

fn is_deferred(item: &TaskListItem, now_seconds: i64) -> bool {
    unix_seconds(&item.task.available_at).is_some_and(|available_at| available_at > now_seconds)
}

fn task_time_cell(
    item: &TaskListItem,
    now_seconds: i64,
    render_mode: TaskListRenderMode,
) -> Line<'static> {
    match render_mode {
        TaskListRenderMode::Upcoming => Line::from(Span::styled(
            crate::tui::time::available_in_label(&item.task.available_at, now_seconds)
                .unwrap_or_default(),
            Style::new().fg(ACCENT),
        )),
        TaskListRenderMode::Queue => {
            let style_input = if item.queue.band == crate::queue::QueueBand::Available {
                &item.task.available_at
            } else {
                &item.task.queue_activity_at
            };
            Line::from(Span::styled(
                item.queue
                    .idle_seconds
                    .map(crate::tui::time::compact_duration)
                    .unwrap_or_default(),
                age_style(style_input, now_seconds),
            ))
        }
        TaskListRenderMode::Flat if is_deferred(item, now_seconds) => Line::from(Span::styled(
            crate::tui::time::available_in_label(&item.task.available_at, now_seconds)
                .unwrap_or_default(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        _ => Line::from(Span::styled(
            task_seconds_since(&item.task.created_at, now_seconds)
                .map(compact_age)
                .unwrap_or_default(),
            age_style(&item.task.created_at, now_seconds),
        )),
    }
}

fn build_epic_parent_row_cells(
    item: &TaskListItem,
    now_seconds: i64,
    store: &TuiStore,
    inline_title_editor: Option<&TextInputView>,
    column_widths: &[usize; 8],
    marked: bool,
    selected_epic_id: Option<&str>,
) -> Vec<Line<'static>> {
    let age_seconds = task_seconds_since(&item.task.created_at, now_seconds);
    let title = inline_title_editor
        .map(|editor| inline_title_edit_cell(editor, column_widths[1]))
        .unwrap_or_else(|| title_cell(item, column_widths[1]));
    let expanded = store.view_state.expanded_epic_ids.contains(&item.task.id);
    let mut ref_spans = vec![
        Span::styled(if marked { "●" } else { " " }, Style::new().fg(YELLOW)),
        Span::styled(if expanded { "▾" } else { "▸" }, Style::new().fg(ACCENT)),
        Span::raw(" "),
    ];
    if let Some((project, suffix)) = item.display_ref.split_once('-') {
        ref_spans.push(Span::styled(
            project.to_string(),
            Style::new().fg(theme::project_color(&item.task.project_key)),
        ));
        ref_spans.push(Span::styled("-", Style::new().fg(FG_DIM)));
        ref_spans.push(Span::styled(suffix.to_string(), Style::new().fg(FG_MUTED)));
    } else {
        ref_spans.push(Span::styled(
            item.display_ref.clone(),
            Style::new().fg(FG_MUTED),
        ));
    }
    vec![
        Line::from(ref_spans),
        title,
        label_cell(&item.labels, column_widths[2]),
        metadata_cell(item, selected_epic_id, false),
        project_cell(item, column_widths[4]),
        status_chip(item.task.status.as_str()),
        Line::from(Span::styled(
            priority_icon(item.task.priority.as_str()),
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            age_seconds.map(compact_age).unwrap_or_default(),
            age_style(&item.task.created_at, now_seconds),
        )),
    ]
}

fn build_epic_child_row_cells(
    item: &TaskListItem,
    last: bool,
    now_seconds: i64,
    column_widths: &[usize; 8],
    marked: bool,
    selected_epic_id: Option<&str>,
) -> Vec<Line<'static>> {
    let age_seconds = task_seconds_since(&item.task.created_at, now_seconds);
    let branch = if last { "└─" } else { "├─" };
    let ref_prefix = format!("{}{branch} ", if marked { "●" } else { " " });
    let display_ref = truncate_chars(
        &item.display_ref,
        column_widths[0].saturating_sub(ref_prefix.chars().count() + 1),
    );
    let ref_line = Line::from(vec![
        Span::styled(ref_prefix, Style::new().fg(FG_DIM)),
        Span::styled(display_ref, Style::new().fg(FG_MUTED)),
        Span::raw(" "),
    ]);
    vec![
        ref_line,
        title_cell(item, column_widths[1]),
        label_cell(&item.labels, column_widths[2]),
        metadata_cell(item, selected_epic_id, false),
        project_cell(item, column_widths[4]),
        status_chip(item.task.status.as_str()),
        Line::from(Span::styled(
            priority_icon(item.task.priority.as_str()),
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            age_seconds.map(compact_age).unwrap_or_default(),
            age_style(&item.task.created_at, now_seconds),
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
        Line::from(""),
    ]
}

fn metadata_cell(
    item: &TaskListItem,
    selected_epic_id: Option<&str>,
    show_deferred: bool,
) -> Line<'static> {
    let mut spans = Vec::new();
    if show_deferred {
        spans.push(Span::styled(
            DEFERRED_MARKER,
            Style::new().fg(ACCENT).remove_modifier(Modifier::BOLD),
        ));
    }
    let is_selected_epic_child = item
        .epic_parent
        .as_ref()
        .is_some_and(|parent| Some(parent.task_id.as_str()) == selected_epic_id);
    if item.task.is_epic {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            EPIC_MARKER,
            Style::new().fg(YELLOW).remove_modifier(Modifier::BOLD),
        ));
    } else if is_selected_epic_child {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            EPIC_CHILD_MARKER,
            Style::new().fg(ACCENT).remove_modifier(Modifier::BOLD),
        ));
    }
    if item.task.deleted {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
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

fn task_ref_cell(item: &TaskListItem, marked: bool) -> Line<'static> {
    let marker = if marked { "●" } else { " " };
    if let Some((project, suffix)) = item.display_ref.split_once('-') {
        Line::from(vec![
            Span::styled(marker.to_string(), Style::new().fg(YELLOW)),
            Span::styled(
                project.to_string(),
                Style::new().fg(theme::project_color(&item.task.project_key)),
            ),
            Span::styled("-", Style::new().fg(FG_DIM)),
            Span::styled(suffix.to_string(), Style::new().fg(FG_MUTED)),
        ])
    } else {
        Line::from(vec![
            Span::styled(marker.to_string(), Style::new().fg(YELLOW)),
            Span::styled(item.display_ref.clone(), Style::new().fg(FG_MUTED)),
        ])
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

fn task_heading_line(item: &TaskListItem) -> Line<'static> {
    let title_style = if item.task.deleted {
        Style::new()
            .fg(FG_MUTED)
            .add_modifier(Modifier::BOLD | Modifier::CROSSED_OUT)
    } else {
        Style::new().fg(FG).add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(
            item.display_ref.clone(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(item.task.title.clone(), title_style),
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
        status_span(item.task.status.as_str()),
        Span::styled("  priority ", Style::new().fg(FG_DIM)),
        Span::styled(
            priority_short(item.task.priority.as_str()),
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  created ", Style::new().fg(FG_DIM)),
        Span::styled(
            local_timestamp_display(&item.task.created_at),
            Style::new().fg(FG_MUTED),
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

fn availability_preview_line(
    item: &TaskListItem,
    now_seconds: i64,
    width: usize,
) -> Option<Line<'static>> {
    if !is_deferred(item, now_seconds) {
        return None;
    }
    let [relative, local] =
        crate::tui::time::availability_summary_lines(&item.task.available_at, false, now_seconds)?;
    let countdown = relative.strip_prefix("available ").unwrap_or(&relative);
    let fixed_width = "available ".len() + countdown.len() + " · ".len();
    let local = truncate_chars(&local, width.saturating_sub(fixed_width));

    Some(Line::from(vec![
        Span::styled("available ", Style::new().fg(FG_DIM)),
        Span::styled(
            countdown.to_string(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::new().fg(FG_DIM)),
        Span::styled(local, Style::new().fg(FG_MUTED)),
    ]))
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

pub(super) fn render_task_preview(
    frame: &mut Frame,
    store: &TuiStore,
    selected: Option<usize>,
    area: Rect,
) {
    let Some(item) = store.selected_task(selected) else {
        return;
    };
    let block = Block::new()
        .title(" SELECTED ")
        .borders(Borders::TOP)
        .border_style(Style::new().fg(BORDER))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG));
    let inner = block.inner(area);
    let lines = task_preview_lines(item, inner.width as usize, inner.height as usize);

    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG)),
        inner,
    );
}

fn task_preview_lines(item: &TaskListItem, width: usize, height: usize) -> Vec<Line<'static>> {
    let labels = labels_display(&item.labels, ", ");
    let mut lines = vec![task_heading_line(item), task_preview_fields_line(item)];
    if let Some(availability) = availability_preview_line(item, now_seconds(), width) {
        lines.push(availability);
    }
    lines.push(Line::from(vec![
        Span::styled("labels ", Style::new().fg(FG_DIM)),
        Span::styled(labels, Style::new().fg(FG_MUTED)),
    ]));
    lines.extend(dependency_preview_lines(item));
    if let Some(parent) = &item.epic_parent {
        lines.push(epic_parent_preview_line(parent));
    }
    let open_child_links: Vec<_> = item
        .epic_children
        .iter()
        .filter(|link| link.unresolved)
        .collect();
    if !open_child_links.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                "CHILD TASKS ",
                Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({}/{})", open_child_links.len(), item.epic_children.len()),
                Style::new().fg(ACCENT),
            ),
        ]));
        let last_child_index = open_child_links.len().saturating_sub(1);
        for (index, link) in open_child_links.iter().take(5).enumerate() {
            let branch = if index == last_child_index {
                "└─"
            } else {
                "├─"
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {branch} "), Style::new().fg(FG_DIM)),
                Span::styled(
                    format!("{} ", link.display_ref),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(link.title.clone(), Style::new().fg(FG_MUTED)),
                Span::styled(format!(" {}", link.status), Style::new().fg(FG_DIM)),
            ]));
        }
        if open_child_links.len() > 5 {
            lines.push(Line::from(vec![Span::styled(
                format!("  ... +{} more", open_child_links.len() - 5),
                Style::new().fg(FG_DIM),
            )]));
        }
    }

    if lines.len() < height {
        if height - lines.len() > 1 {
            lines.push(Line::from(""));
        }
        let description_height = height.saturating_sub(lines.len());
        lines.extend(render_markdown_preview(
            &description_or_placeholder(&item.task.description),
            width,
            description_height,
        ));
    }
    lines.truncate(height);
    lines
}

fn epic_parent_preview_line(parent: &crate::query::TaskDependencyLink) -> Line<'static> {
    Line::from(vec![
        Span::styled("part of ", Style::new().fg(FG_DIM)),
        Span::styled(EPIC_MARKER, Style::new().fg(YELLOW)),
        Span::styled(" ", Style::new().fg(FG_DIM)),
        Span::styled(
            format!("{} {}", parent.display_ref, parent.title),
            Style::new().fg(FG_MUTED),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choices::{TaskPriority, TaskStatus};
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
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                created_at: "2026-06-20T00:00:00Z".to_string(),
                updated_at: "2026-06-20T00:00:00Z".to_string(),
                queue_activity_at: "2026-06-20T00:00:00Z".to_string(),
                available_at: String::new(),
                deleted: false,
                is_epic: false,
            },
            display_ref: "APP-1".to_string(),
            labels: Vec::new(),
            notes: Vec::new(),
            has_conflict: false,
            unresolved_blocker_count: 0,
            dependent_count: 0,
            depends_on: Vec::new(),
            blocks: Vec::new(),
            epic_children: Vec::new(),
            epic_parent: None,
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
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(3),
            Constraint::Length(5),
        ];
        terminal
            .draw(|frame| {
                let column_widths = task_list_column_widths(&columns, frame.area().width);
                let style = row_style(true, true, false);
                let cells = build_task_row_cells(
                    item,
                    0,
                    render_mode,
                    inline_title_editor,
                    &column_widths,
                    false,
                    None,
                );
                render_task_row_cells(frame, frame.area(), style, &columns, &cells);
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
        let labels = tasks
            .iter()
            .flat_map(|item| item.labels.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        for label in labels {
            store.create_label(label).await.unwrap();
        }
        for item in tasks {
            let draft = TaskDraft {
                title: item.task.title,
                description: item.task.description,
                project: None,
                status: item.task.status.as_str().to_string(),
                priority: item.task.priority.as_str().to_string(),
                labels: item.labels,
                available_at: item.task.available_at,
                is_epic: false,
            };
            store.create_task(draft, None).await.unwrap();
        }
        store
    }

    fn column_length(column: Constraint) -> u16 {
        match column {
            Constraint::Length(width) => width,
            other => panic!("expected fixed column width, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn label_column_width_uses_visible_task_labels() {
        let mut hidden_wide = task_item("zz hidden wide label");
        hidden_wide.labels = vec!["very-wide-label".to_string()];
        let mut store = test_store_with_tasks(vec![
            task_item("aa visible plain one"),
            task_item("bb visible plain two"),
            task_item("cc visible plain three"),
            hidden_wide,
        ])
        .await;
        store
            .tasks
            .sort_by(|left, right| left.task.title.cmp(&right.task.title));
        let area = Rect::new(0, 0, 100, 4);
        let mut table_state = TableState::default();

        let top_model = build_task_list_render_model(
            &store,
            &mut table_state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::new(),
        );

        assert_eq!(column_length(top_model.columns[2]), 0);

        table_state.select(Some(3));
        let scrolled_model = build_task_list_render_model(
            &store,
            &mut table_state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::new(),
        );

        assert_eq!(column_length(scrolled_model.columns[2]), 17);
    }

    #[test]
    fn label_column_width_collapses_without_visible_labels() {
        let tasks = vec![task_item("plain"), task_item("also plain")];

        assert_eq!(label_column_width_from_tasks(&tasks, false), 0);
    }

    #[test]
    fn label_column_width_reserves_lane_for_visible_labels() {
        let mut task = task_item("labeled");
        task.labels = vec!["search".to_string(), "ux".to_string()];

        assert_eq!(label_column_width_from_tasks(&[task], false), 11);
    }

    #[test]
    fn label_column_width_collapses_in_narrow_layout() {
        let mut task = task_item("labeled");
        task.labels = vec!["search".to_string()];

        assert_eq!(label_column_width_from_tasks(&[task], true), 0);
    }

    #[test]
    fn label_header_cell_aligns_with_label_column_content() {
        assert_eq!(label_header_cell("LABELS", 12).to_string(), "     LABELS ");
        assert_eq!(label_header_cell("LABELS", 6).to_string(), "LABELS");
    }

    #[test]
    fn queue_row_time_uses_queue_idle_duration() {
        let mut item = task_item("queued");
        item.task.created_at = "0".to_string();
        item.task.queue_activity_at = (9 * 86_400).to_string();
        item.queue.idle_seconds = Some(86_400);

        let cells = build_task_row_cells(
            &item,
            10 * 86_400,
            TaskListRenderMode::Queue,
            None,
            &[12, 40, 12, 6, 9, 10, 3, 5],
            false,
            None,
        );

        assert_eq!(cells[7].to_string(), "1d");
    }

    #[test]
    fn flat_row_marks_deferred_task_and_shows_availability_time() {
        let mut item = task_item("deferred");
        item.task.available_at = "200".to_string();

        let cells = build_task_row_cells(
            &item,
            100,
            TaskListRenderMode::Flat,
            None,
            &[12, 40, 12, 6, 9, 10, 3, 5],
            false,
            None,
        );

        assert_eq!(cells[3].to_string(), DEFERRED_MARKER);
        assert_eq!(cells[7].to_string(), "in1m");
        assert_eq!(cells[7].spans[0].style.fg, Some(ACCENT));
    }

    #[test]
    fn task_header_labels_age_column() {
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
        ];
        terminal
            .draw(|frame| {
                render_task_header(
                    frame,
                    frame.area(),
                    columns,
                    TaskListRenderMode::Flat,
                    false,
                )
            })
            .unwrap();

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("AGE"));
        assert!(!rendered.contains("IDLE"));
    }

    #[test]
    fn task_header_labels_mixed_deferred_time_column() {
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
        ];
        terminal
            .draw(|frame| {
                render_task_header(frame, frame.area(), columns, TaskListRenderMode::Flat, true)
            })
            .unwrap();

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("TIME"));
        assert!(!rendered.contains("AGE"));
    }

    #[test]
    fn metadata_column_width_collapses_without_metadata() {
        let tasks = vec![task_item("plain"), task_item("also plain")];

        assert_eq!(metadata_column_width_from_tasks(&tasks), 0);
    }

    #[test]
    fn metadata_column_width_uses_given_task_refs() {
        let plain = task_item("plain");
        let mut documented = task_item("documented");
        documented.notes = vec![crate::query::TaskNote {
            body: "one".to_string(),
            created_at: "001".to_string(),
        }];
        let visible_tasks = vec![&plain];
        let all_tasks = vec![&plain, &documented];

        assert_eq!(
            metadata_column_width_from_task_refs(&visible_tasks, None, false),
            0
        );
        assert_eq!(
            metadata_column_width_from_task_refs(&all_tasks, None, false),
            3
        );
    }

    #[test]
    fn metadata_column_width_reserves_lane_for_deferred_marker() {
        let mut task = task_item("deferred");
        task.task.available_at = "2999-01-01T00:00:00Z".to_string();

        assert_eq!(
            metadata_column_width_from_task_refs(&[&task], None, true),
            3
        );
        assert_eq!(
            metadata_column_width_from_task_refs(&[&task], None, false),
            0
        );
    }

    #[test]
    fn metadata_column_width_reserves_lane_for_metadata() {
        let mut task = task_item("documented");
        task.notes = vec![crate::query::TaskNote {
            body: "one".to_string(),
            created_at: "001".to_string(),
        }];

        assert_eq!(metadata_column_width_from_tasks(&[task]), 3);
    }

    #[test]
    fn metadata_column_width_reserves_lane_for_epics() {
        let mut task = task_item("epic");
        task.task.is_epic = true;

        assert_eq!(metadata_column_width_from_tasks(&[task]), 3);
    }

    #[test]
    fn priority_column_width_collapses_without_priority() {
        let tasks = vec![task_item("plain"), task_item("also plain")];

        assert_eq!(priority_column_width_from_tasks(&tasks), 0);
    }

    #[test]
    fn priority_column_width_reserves_lane_for_priority() {
        let mut task = task_item("prioritized");
        task.task.priority = TaskPriority::High;

        assert_eq!(priority_column_width_from_tasks(&[task]), 3);
    }

    #[tokio::test]
    async fn task_status_at_position_only_hits_status_column() {
        let store = test_store_with_tasks(vec![task_item("task")]).await;
        let table_state = TableState::default();
        let area = Rect::new(0, 0, 140, 10);
        let task_id = store.tasks[0].task.id.clone();

        let status_area = task_list_status_area(&store, &table_state, area, 1);
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

        let status_area = task_list_status_area(&store, &table_state, area, 1);
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
    fn marked_row_shows_ref_marker() {
        let item = task_item("marked");
        let line = task_ref_cell(&item, true);

        assert!(line.to_string().starts_with("●"));
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
            &[12, 40, 12, 6, 9, 10, 3, 5],
            false,
            None,
        );

        assert!(rendered.contains("original title"));
        assert!(!rendered.contains("deleted original title"));
        assert_eq!(cells[3].to_string(), "×");
        assert_eq!(cells[5].to_string(), "□ todo");
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

        assert_eq!(metadata_cell(&item, None, false).to_string(), "✎");
    }

    #[test]
    fn metadata_cell_marks_epics() {
        let mut item = task_item("epic");
        item.task.is_epic = true;

        let line = metadata_cell(&item, None, false);

        assert_eq!(line.to_string(), EPIC_MARKER);
        assert_eq!(line.spans[0].style.fg, Some(YELLOW));
    }

    #[test]
    fn metadata_cell_marks_children_of_selected_epic() {
        let mut item = task_item("child");
        item.epic_parent = Some(crate::query::TaskDependencyLink {
            task_id: "epic-1".to_string(),
            display_ref: "APP-EPIC".to_string(),
            title: "Selected epic".to_string(),
            status: "todo".to_string(),
            priority: "none".to_string(),
            unresolved: true,
        });

        let line = metadata_cell(&item, Some("epic-1"), false);

        assert_eq!(line.to_string(), EPIC_CHILD_MARKER);
        assert_eq!(line.spans[0].style.fg, Some(ACCENT));
        assert_eq!(
            metadata_cell(&item, Some("other-epic"), false).to_string(),
            ""
        );
        assert_eq!(
            metadata_column_width_from_task_refs(&[&item], Some("epic-1"), false),
            3
        );
    }

    #[test]
    fn metadata_cell_shows_dependency_counts() {
        let mut item = task_item("blocked");
        item.unresolved_blocker_count = 2;
        item.dependent_count = 1;

        assert_eq!(metadata_cell(&item, None, false).to_string(), "←2 →1");
    }

    #[test]
    fn metadata_cell_ignores_description_without_notes() {
        let mut item = task_item("plain");
        item.task.description = "details".to_string();

        assert_eq!(metadata_cell(&item, None, false).to_string(), "");
    }

    #[test]
    fn task_preview_fields_show_created_timestamp() {
        let item = task_item("preview");
        let rendered = task_preview_fields_line(&item).to_string();

        assert!(rendered.contains("created "));
    }

    #[test]
    fn task_preview_shows_future_availability() {
        let mut item = task_item("preview");
        item.task.available_at = "200".to_string();

        let line = availability_preview_line(&item, 100, 80).unwrap();

        assert!(line.to_string().starts_with("available in 1m · "));
        assert_eq!(line.spans[0].style.fg, Some(FG_DIM));
        assert_eq!(line.spans[1].style.fg, Some(ACCENT));
        assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[3].style.fg, Some(FG_MUTED));
    }

    #[test]
    fn task_preview_omits_elapsed_availability() {
        let mut item = task_item("preview");
        item.task.available_at = "100".to_string();

        assert!(availability_preview_line(&item, 200, 80).is_none());
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
            &[12, 40, 12, 6, 9, 10, 3, 5],
            false,
            None,
        );

        assert_eq!(cells.len(), 8);
        assert_eq!(cells[3].to_string(), "←1 →1 ✎");
        assert_eq!(cells[4].to_string(), "app ");

        item.task.deleted = true;
        let cells = build_task_row_cells(
            &item,
            0,
            TaskListRenderMode::Flat,
            None,
            &[12, 40, 12, 6, 9, 10, 3, 5],
            false,
            None,
        );
        assert_eq!(cells[3].to_string(), "× ←1 →1 ✎");
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
            &[12, 40, 12, 6, 9, 10, 3, 5],
            false,
            None,
        );

        assert!(cells[1].to_string().contains("edited title"));
    }

    #[test]
    fn epic_child_ref_prefix_aligns_tree_with_parent_marker() {
        let item = task_item("child");

        let cells =
            build_epic_child_row_cells(&item, false, 0, &[14, 40, 12, 6, 9, 10, 3, 5], false, None);

        assert_eq!(cells[0].to_string(), " ├─ APP-1 ");
    }

    #[test]
    fn preview_marks_epic_parent_with_star() {
        let parent = crate::query::TaskDependencyLink {
            task_id: "parent-task-id".to_string(),
            display_ref: "APP-EPIC".to_string(),
            title: "Build the epic container".to_string(),
            status: "inbox".to_string(),
            priority: "medium".to_string(),
            unresolved: true,
        };

        let line = epic_parent_preview_line(&parent);

        assert_eq!(
            line.to_string(),
            format!("part of {EPIC_MARKER} APP-EPIC Build the epic container")
        );
    }

    #[test]
    fn preview_renders_markdown_blocks_and_inline_styles() {
        let mut item = task_item("documented");
        item.task.description =
            "### Context\n\nFirst **bold** paragraph.\n\n- one\n- `two`".to_string();

        let lines = task_preview_lines(&item, 40, 12);
        let description = &lines[4..];
        let rendered = description
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(rendered, "Context\n\nFirst bold paragraph.\n\n- one\n- two");
        assert!(!rendered.contains("###"));
        assert!(
            description[2]
                .spans
                .iter()
                .any(|span| span.content == "bold"
                    && span.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn preview_bounds_wrapped_markdown_to_available_height() {
        let mut item = task_item("documented");
        item.task.description = "A description with enough words to wrap across many lines in the selected task preview.".to_string();

        let lines = task_preview_lines(&item, 20, 7);

        assert_eq!(lines.len(), 7);
        assert_eq!(lines[3].to_string(), "");
        assert!(lines[6].to_string().ends_with('…'));
        assert!(lines[4..].iter().all(|line| line.width() <= 20));
    }

    #[test]
    fn preview_uses_single_remaining_line_for_description() {
        let mut item = task_item("documented");
        item.task.description = "small body".to_string();

        let lines = task_preview_lines(&item, 20, 4);

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3].to_string(), "small body");
    }

    #[test]
    fn preview_shows_child_tasks_for_epic_parent() {
        let mut item = task_item("epic");
        item.epic_children = vec![
            crate::query::TaskDependencyLink {
                task_id: "child-1".to_string(),
                display_ref: "APP-C001".to_string(),
                title: "first child".to_string(),
                status: "todo".to_string(),
                priority: "none".to_string(),
                unresolved: true,
            },
            crate::query::TaskDependencyLink {
                task_id: "child-2".to_string(),
                display_ref: "APP-C002".to_string(),
                title: "second child".to_string(),
                status: "active".to_string(),
                priority: "none".to_string(),
                unresolved: true,
            },
        ];

        let lines = vec![
            task_heading_line(&item),
            task_preview_fields_line(&item),
            Line::from(vec![
                Span::styled("labels ", Style::new().fg(FG_DIM)),
                Span::styled(String::new(), Style::new().fg(FG_MUTED)),
            ]),
        ];
        let mut extended_lines = lines.clone();
        extended_lines.extend(dependency_preview_lines(&item));
        let open_child_links: Vec<_> = item
            .epic_children
            .iter()
            .filter(|link| link.unresolved)
            .collect();
        if !open_child_links.is_empty() {
            extended_lines.push(Line::from(vec![
                Span::styled(
                    "CHILD TASKS ",
                    Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("({}/{})", open_child_links.len(), item.epic_children.len()),
                    Style::new().fg(ACCENT),
                ),
            ]));
            let last_child_index = open_child_links.len().saturating_sub(1);
            for (index, link) in open_child_links.iter().take(5).enumerate() {
                let branch = if index == last_child_index {
                    "└─"
                } else {
                    "├─"
                };
                extended_lines.push(Line::from(vec![
                    Span::styled(format!("  {branch} "), Style::new().fg(FG_DIM)),
                    Span::styled(
                        format!("{} ", link.display_ref),
                        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(&link.title, Style::new().fg(FG_MUTED)),
                    Span::styled(format!(" {}", link.status), Style::new().fg(FG_DIM)),
                ]));
            }
        }

        let rendered = extended_lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("CHILD TASKS"));
        assert!(rendered.contains("(2/2)"));
        assert!(rendered.contains("  ├─ APP-C001"));
        assert!(rendered.contains("first child"));
        assert!(rendered.contains("  └─ APP-C002"));
        assert!(rendered.contains("second child"));
    }
}
