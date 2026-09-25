use std::collections::BTreeSet;

use super::cells::{
    EpicSelectionContext, TaskListCellLayout, TaskRowState, TaskTimeContext, blank_task_row_cells,
    build_epic_child_row_cells_for_columns, build_epic_parent_row_cells_for_columns,
    build_task_row_cells_for_columns, is_deferred, task_state_prefix,
};
use super::layout::TableLayout;
use super::sizing::{task_list_columns, task_list_columns_for_tasks, visible_task_items};
use super::source::TaskListSource;
use super::view_model::{TaskGroupRow, TaskListProjection, TaskListRow, scrollbar_position};
use crate::config::TableColumn;
use crate::query::TaskSort;
use crate::queue::now_seconds;
use crate::tui::app::Focus;
use crate::tui::overlay::TextInputView;
use crate::tui::store::TaskListRenderMode;
use crate::tui::theme::{
    ACCENT, BG, BG_ALT, BORDER, INVERSE_FG, RELATED, SELECTED, SELECTED_INACTIVE,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, TableState,
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug)]
pub(super) struct TaskListRenderModel {
    pub(super) layout: TableLayout,
    pub(super) row_areas: Vec<Rect>,
    pub(super) rows: Vec<TaskListRenderRow>,
    pub(super) scroll: usize,
    pub(super) row_count: usize,
    pub(super) viewport_rows: usize,
    pub(super) top_scroll: usize,
    pub(super) render_mode: TaskListRenderMode,
    pub(super) has_deferred_rows: bool,
    pub(super) due_order: bool,
}

#[derive(Debug)]
pub(super) enum TaskListRenderRow {
    Group(TaskGroupRow),
    Task(TaskListTaskRow),
}

#[derive(Debug)]
pub(super) struct TaskListTaskRow {
    pub(super) style: Style,
    pub(super) cells: Vec<Line<'static>>,
    pub(super) state: TaskRowState,
}

pub(super) fn render_task_list(
    frame: &mut Frame,
    source: &TaskListSource<'_>,
    table_state: &mut TableState,
    focus: Focus,
    area: Rect,
    inline_title_editor: Option<&TextInputView>,
    marked_task_ids: &BTreeSet<crate::ids::TaskId>,
) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);
    let model = build_task_list_render_model(
        source,
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
        model.layout,
        model.render_mode,
        model.has_deferred_rows,
        model.due_order,
        source.table.compact_status,
    );

    if source.tasks.is_empty() {
        let body = Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(1),
        );
        super::super::empty_state::render_empty_state(
            frame,
            body,
            crate::tui::ui::empty_state::task_empty_state_for(&source.empty_state),
        );
        return;
    }

    for (index, row) in model.rows.iter().enumerate() {
        let Some(row_area) = model.row_areas.get(index + 1).copied() else {
            break;
        };
        match row {
            TaskListRenderRow::Group(group) => {
                render_group_row(frame, &group.label, group.count, row_area);
            }
            TaskListRenderRow::Task(row) => {
                render_task_row_from_model(frame, row_area, &model.layout, row);
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

pub(super) fn build_task_list_render_model(
    source: &TaskListSource<'_>,
    table_state: &mut TableState,
    focus: Focus,
    area: Rect,
    inline_title_editor: Option<&TextInputView>,
    marked_task_ids: &BTreeSet<crate::ids::TaskId>,
) -> TaskListRenderModel {
    let row_areas = Layout::vertical(vec![Constraint::Length(1); area.height as usize]).split(area);
    if row_areas.is_empty() {
        return TaskListRenderModel {
            layout: TableLayout::resolve(
                &task_list_columns(source, area.width < 90),
                &source.table.columns,
                area.width,
            ),
            row_areas: row_areas.to_vec(),
            rows: Vec::new(),
            scroll: 0,
            row_count: 0,
            viewport_rows: 0,
            top_scroll: 0,
            render_mode: source.view_state.render_mode(),
            has_deferred_rows: false,
            due_order: source.view_state.sort() == TaskSort::DueOn
                && !source.table.columns.contains(&TableColumn::Due),
        };
    }

    let viewport_rows = row_areas.len().saturating_sub(1);
    let projection = TaskListProjection::from_table_state(&source.view, table_state, viewport_rows);
    projection.commit_scroll(table_state);
    let selected_task = projection.selected_task;
    let epic_selection = EpicSelectionContext::from_selected(
        selected_task.and_then(|index| source.tasks.get(index)),
    );
    let visible_rows = projection.visible_rows();
    let visible_tasks = visible_task_items(source, &visible_rows);
    let columns =
        task_list_columns_for_tasks(source, area.width < 90, &visible_tasks, epic_selection);

    let now = now_seconds();
    let due_order = source.view_state.sort() == TaskSort::DueOn;
    let has_deferred_rows = projection.view.render_mode == TaskListRenderMode::Flat
        && visible_tasks.iter().any(|item| is_deferred(item, now));
    let configured_columns = &source.table.columns;
    let show_due_in_time = !configured_columns.contains(&TableColumn::Due);
    let compact_status = source.table.compact_status;
    let layout = TableLayout::resolve(&columns, configured_columns, area.width);
    let state_column = layout.state_column();
    let column_widths = layout.widths();
    let mut rows = Vec::new();
    for (_, row) in visible_rows {
        match row {
            TaskListRow::Group(group) => rows.push(TaskListRenderRow::Group(group.clone())),
            TaskListRow::Task { task_index } => {
                let Some(item) = source.tasks.get(*task_index) else {
                    rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                        style: row_style(false, focus == Focus::Tasks, false, false, false),
                        cells: blank_task_row_cells(),
                        state: TaskRowState::default(),
                    }));
                    continue;
                };
                let selected = selected_task == Some(*task_index);
                let marked = marked_task_ids.contains(&item.task.id);
                let related = epic_selection.highlights_parent(item);
                let style = row_style(
                    selected,
                    focus == Focus::Tasks,
                    marked,
                    related,
                    item.unresolved_blocker_count > 0,
                );
                let cells = if projection.view.render_mode == TaskListRenderMode::Epics
                    && item.task.is_epic
                {
                    build_epic_parent_row_cells_for_columns(
                        item,
                        TaskTimeContext {
                            now_seconds: now,
                            render_mode: TaskListRenderMode::Epics,
                            due_order,
                            show_due: show_due_in_time,
                        },
                        &source.view_state.expanded_epic_ids,
                        inline_title_editor.filter(|_| selected),
                        TaskListCellLayout {
                            widths: &column_widths,
                            state_column,
                            compact_status,
                        },
                        TaskRowState {
                            selected,
                            focused: focus == Focus::Tasks,
                            marked,
                        },
                        epic_selection,
                    )
                } else {
                    build_task_row_cells_for_columns(
                        item,
                        TaskTimeContext {
                            now_seconds: now,
                            render_mode: projection.view.render_mode,
                            due_order,
                            show_due: show_due_in_time,
                        },
                        inline_title_editor.filter(|_| selected),
                        TaskListCellLayout {
                            widths: &column_widths,
                            state_column,
                            compact_status,
                        },
                        TaskRowState {
                            selected,
                            focused: focus == Focus::Tasks,
                            marked,
                        },
                        epic_selection,
                    )
                };
                rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                    style,
                    cells,
                    state: TaskRowState {
                        selected,
                        focused: focus == Focus::Tasks,
                        marked,
                    },
                }));
            }
            TaskListRow::EpicChild {
                parent_index: _,
                task_index,
                last,
            } => {
                let Some(item) = source.tasks.get(*task_index) else {
                    rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                        style: row_style(false, focus == Focus::Tasks, false, false, false),
                        cells: blank_task_row_cells(),
                        state: TaskRowState::default(),
                    }));
                    continue;
                };
                let selected = selected_task == Some(*task_index);
                let marked = marked_task_ids.contains(&item.task.id);
                rows.push(TaskListRenderRow::Task(TaskListTaskRow {
                    style: row_style(
                        selected,
                        focus == Focus::Tasks,
                        marked,
                        false,
                        item.unresolved_blocker_count > 0,
                    ),
                    cells: build_epic_child_row_cells_for_columns(
                        item,
                        *last,
                        inline_title_editor.filter(|_| selected),
                        TaskTimeContext {
                            now_seconds: now,
                            render_mode: TaskListRenderMode::Epics,
                            due_order,
                            show_due: show_due_in_time,
                        },
                        TaskListCellLayout {
                            widths: &column_widths,
                            state_column,
                            compact_status,
                        },
                        TaskRowState {
                            selected,
                            focused: focus == Focus::Tasks,
                            marked,
                        },
                        epic_selection,
                    ),
                    state: TaskRowState {
                        selected,
                        focused: focus == Focus::Tasks,
                        marked,
                    },
                }));
            }
        }
    }

    TaskListRenderModel {
        layout,
        row_areas: row_areas.to_vec(),
        rows,
        scroll: projection.scroll,
        row_count: projection.row_count(),
        viewport_rows,
        top_scroll: projection.top_scroll(),
        render_mode: projection.view.render_mode,
        has_deferred_rows,
        due_order: due_order && show_due_in_time,
    }
}

pub(super) fn render_task_scrollbar(
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

pub(super) fn list_scrollbar_area(area: Rect) -> Rect {
    Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    }
}

pub(super) fn render_task_header(
    frame: &mut Frame,
    area: Rect,
    layout: TableLayout,
    render_mode: TaskListRenderMode,
    has_deferred_rows: bool,
    due_order: bool,
    compact_status: bool,
) {
    let style = Style::new()
        .fg(INVERSE_FG)
        .bg(BORDER)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Block::new().style(style), area);
    let time_header = match render_mode {
        TaskListRenderMode::Queue => "IDLE",
        TaskListRenderMode::Upcoming => "WHEN",
        _ if due_order => "DUE",
        TaskListRenderMode::Epics => "ACT",
        TaskListRenderMode::Flat if has_deferred_rows => "TIME",
        _ => "AGE",
    };
    for column in TableColumn::ALL {
        let label = match column {
            TableColumn::Ref => "   REF",
            TableColumn::Title => "TITLE",
            TableColumn::Labels if render_mode == TaskListRenderMode::Epics => "SUMMARY",
            TableColumn::Labels => "LABELS",
            TableColumn::Metadata => "",
            TableColumn::Project => "PROJECT",
            TableColumn::Status => status_header(compact_status),
            TableColumn::Priority => "P",
            TableColumn::Time => time_header,
            TableColumn::Due => "DUE",
        };
        let area = layout.cell(column, area);
        let label = if column == TableColumn::Labels && render_mode != TaskListRenderMode::Epics {
            label_header_cell(label, area.width as usize)
        } else {
            Line::from(label)
        };
        frame.render_widget(Paragraph::new(label).style(style), area);
    }
}

pub(super) fn label_header_cell(label: &str, max_width: usize) -> Line<'static> {
    let label_width = label.width();
    if label_width >= max_width {
        return Line::from(label.to_string());
    }
    let padding = max_width.saturating_sub(label_width + 1);
    Line::from(format!("{}{label} ", " ".repeat(padding)))
}

pub(super) fn status_header(compact_status: bool) -> &'static str {
    if compact_status { "S" } else { "STATUS" }
}

pub(super) fn render_group_row(frame: &mut Frame, label: &str, count: usize, area: Rect) {
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

pub(super) fn row_style(
    selected: bool,
    focused: bool,
    marked: bool,
    related: bool,
    blocked: bool,
) -> Style {
    let style = if selected {
        if focused { SELECTED } else { SELECTED_INACTIVE }
    } else if related {
        RELATED
    } else if marked {
        Style::new().bg(BG_ALT)
    } else {
        Style::new().bg(BG)
    };
    if blocked && !selected {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

pub(super) fn render_task_row_from_model(
    frame: &mut Frame,
    area: Rect,
    layout: &TableLayout,
    row: &TaskListTaskRow,
) {
    render_task_row_cells(frame, area, row.style, layout, &row.cells, row.state);
}

pub(super) fn render_task_row_cells(
    frame: &mut Frame,
    area: Rect,
    style: Style,
    layout: &TableLayout,
    values: &[Line<'static>],
    state: TaskRowState,
) {
    frame.render_widget(Block::new().style(style), area);
    let state_area = layout.state_gutter(area);
    if state_area.width > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(task_state_prefix(
                state.selected,
                state.focused,
                state.marked,
            )))
            .style(style),
            state_area,
        );
    }
    for (column, value) in TableColumn::ALL.into_iter().zip(values) {
        let area = layout.cell(column, area);
        frame.render_widget(Paragraph::new(value.clone()).style(style), area);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use crate::tui::theme::FG;

    #[test]
    fn label_header_cell_aligns_with_label_column_content() {
        assert_eq!(label_header_cell("LABELS", 12).to_string(), "     LABELS ");
        assert_eq!(label_header_cell("LABELS", 6).to_string(), "LABELS");
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
            Constraint::Length(6),
        ];
        terminal
            .draw(|frame| {
                render_task_header(
                    frame,
                    frame.area(),
                    TableLayout::resolve(&columns, &TableColumn::DEFAULT, frame.area().width),
                    TaskListRenderMode::Flat,
                    false,
                    false,
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
            Constraint::Length(6),
        ];
        terminal
            .draw(|frame| {
                render_task_header(
                    frame,
                    frame.area(),
                    TableLayout::resolve(&columns, &TableColumn::DEFAULT, frame.area().width),
                    TaskListRenderMode::Flat,
                    true,
                    false,
                    false,
                )
            })
            .unwrap();

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("TIME"));
        assert!(!rendered.contains("AGE"));
    }

    #[test]
    fn due_order_labels_and_populates_due_column() {
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
                render_task_header(
                    frame,
                    frame.area(),
                    TableLayout::resolve(&columns, &TableColumn::DEFAULT, frame.area().width),
                    TaskListRenderMode::Flat,
                    false,
                    true,
                    false,
                )
            })
            .unwrap();
        assert!(buffer_text(terminal.backend().buffer()).contains("DUE"));

        let mut item = task_list_item("future deadline");
        item.task.due_on = Some("2999-01-01".to_string());
        let cell = task_time_cell(&item, 0, TaskListRenderMode::Flat, true, true);
        assert_eq!(cell.to_string(), "Jan1");
        assert_eq!(cell.spans[0].style.fg, Some(ACCENT));
    }

    #[test]
    fn default_status_column_shows_text_header_and_status() {
        let list = TaskListFixture::new(vec![task_list_item("task")]);
        let buffer = render_task_list_buffer(&list.source(), 140, 8);
        let rendered = buffer_text(&buffer);

        assert!(rendered.contains("STATUS"), "{rendered}");
        assert!(rendered.contains("□ todo"), "{rendered}");
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
    fn selected_row_renders_inline_title_editor() {
        let item = task_list_item("original title");
        let editor = TextInputView {
            kind: TextInputKind::EditTitle,
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
        let item = task_list_item("original title");
        let editor = TextInputView {
            kind: TextInputKind::EditTitle,
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
        let item = task_list_item("original title");

        let buffer = render_task_row_buffer(&item, None);
        let rendered = buffer_text(&buffer);

        assert!(rendered.contains("original title"));
    }

    #[test]
    fn blocked_rows_are_dimmed_unless_selected() {
        let blocked = row_style(false, true, false, false, true);
        assert!(blocked.add_modifier.contains(Modifier::DIM));

        assert_eq!(row_style(true, true, false, false, true), SELECTED);
    }
}
