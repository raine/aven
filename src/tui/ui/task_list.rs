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
use crate::query::{TaskListItem, TaskSort};
use crate::queue::{now_seconds, unix_seconds};
use crate::tui::app::{Focus, WidgetState};
use crate::tui::overlay::TextInputView;
use crate::tui::store::TuiStore;
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, FG_MUTED, SELECTED, SELECTED_INACTIVE,
};
use crate::tui::widgets::{
    age_style, priority_icon, priority_short, status_chip, status_span, title_cell,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskRenderMode {
    Default,
    Queue,
}

impl TaskRenderMode {
    fn uses_queue_age(self) -> bool {
        matches!(self, Self::Queue)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TaskGroupRow {
    label: &'static str,
    count: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum TaskListRow {
    Group(TaskGroupRow),
    Task { task_index: usize },
}

struct TaskListView {
    rows: Vec<TaskListRow>,
    render_mode: TaskRenderMode,
}

impl TaskListView {
    fn new(store: &TuiStore) -> Self {
        Self::from_tasks(store.sort, &store.tasks)
    }

    fn from_tasks(sort: TaskSort, tasks: &[TaskListItem]) -> Self {
        let render_mode = if sort == TaskSort::Queue {
            TaskRenderMode::Queue
        } else {
            TaskRenderMode::Default
        };
        let rows = match render_mode {
            TaskRenderMode::Queue => queue_rows(tasks),
            TaskRenderMode::Default => task_rows(tasks),
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

pub(super) fn render_tasks(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    focus: Focus,
    area: Rect,
    inline_title_editor: Option<&TextInputView>,
) {
    let [table_area, preview_area] = if area.height >= 24 {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(8)]).areas(area)
    } else {
        [area, Rect::default()]
    };
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
    if area.height == 0 {
        return;
    }

    let project_width = project_column_width(store, area.width < 90);
    let columns = [
        Constraint::Length(12),
        Constraint::Fill(1),
        Constraint::Length(project_width),
        Constraint::Length(10),
        Constraint::Length(3),
        Constraint::Length(5),
    ];
    let row_areas = Layout::vertical(vec![Constraint::Length(1); area.height as usize]).split(area);
    render_task_header(frame, row_areas[0], columns);

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

    let now_seconds = now_seconds();
    let mut row = 1usize;
    for visual_row in scroll..view.rows.len() {
        if row >= row_areas.len() {
            break;
        }
        match &view.rows[visual_row] {
            TaskListRow::Group(group) => {
                render_group_row(frame, group.label, group.count, row_areas[row]);
            }
            TaskListRow::Task { task_index } => {
                let Some(item) = store.tasks.get(*task_index) else {
                    continue;
                };
                let selected = selected_task == Some(*task_index);
                render_task_row(
                    frame,
                    item,
                    row_style(selected, focus == Focus::Tasks),
                    row_areas[row],
                    TaskRowContext {
                        columns,
                        now_seconds,
                        render_mode: view.render_mode,
                        inline_title_editor: inline_title_editor.filter(|_| selected),
                    },
                );
            }
        }
        row += 1;
    }

    render_task_scrollbar(
        frame,
        scroll,
        view.rows.len(),
        viewport_rows,
        task_list_top_scroll(&view),
        area,
    );
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

fn render_task_header(frame: &mut Frame, area: Rect, columns: [Constraint; 6]) {
    let cells = Layout::horizontal(columns).areas::<6>(area);
    let style = Style::new().fg(BG).bg(BORDER).add_modifier(Modifier::BOLD);
    frame.render_widget(Block::new().style(style), area);
    for (area, label) in cells
        .into_iter()
        .zip([" REF", "TITLE", "PROJECT", "STATUS", "P", "IDLE"])
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

struct TaskRowContext<'a> {
    columns: [Constraint; 6],
    now_seconds: i64,
    render_mode: TaskRenderMode,
    inline_title_editor: Option<&'a TextInputView>,
}

fn render_task_row(
    frame: &mut Frame,
    item: &TaskListItem,
    style: Style,
    area: Rect,
    context: TaskRowContext<'_>,
) {
    frame.render_widget(Block::new().style(style), area);
    let cells = Layout::horizontal(context.columns).areas::<6>(area);
    let age_seconds = if context.render_mode.uses_queue_age() {
        item.queue.idle_seconds()
    } else {
        task_seconds_since(&item.task.created_at, context.now_seconds)
    };
    let age_style_input = if context.render_mode.uses_queue_age() {
        &item.task.queue_activity_at
    } else {
        &item.task.created_at
    };
    let title = context
        .inline_title_editor
        .map(|editor| inline_title_edit_cell(editor, cells[1].width as usize))
        .unwrap_or_else(|| title_cell(item, cells[1].width as usize));
    let values = [
        task_ref_cell(item),
        title,
        project_cell(item, cells[2].width as usize),
        status_chip(&item.task.status),
        Line::from(Span::styled(
            priority_icon(&item.task.priority),
            theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            age_seconds.map(compact_age).unwrap_or_default(),
            age_style(age_style_input, context.now_seconds),
        )),
    ];
    for (area, value) in cells.into_iter().zip(values) {
        frame.render_widget(Paragraph::new(value).style(style), area);
    }
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
    let hours = age_seconds / 3_600;
    if hours < 24 {
        return format!("{}h", hours.max(0));
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
            queue: Default::default(),
        }
    }

    fn render_task_row_buffer(
        item: &TaskListItem,
        inline_title_editor: Option<&TextInputView>,
    ) -> ratatui::buffer::Buffer {
        render_task_row_buffer_with_mode(item, TaskRenderMode::Default, inline_title_editor)
    }

    fn render_task_row_buffer_with_mode(
        item: &TaskListItem,
        render_mode: TaskRenderMode,
        inline_title_editor: Option<&TextInputView>,
    ) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let columns = [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(3),
            Constraint::Length(5),
        ];
        terminal
            .draw(|frame| {
                render_task_row(
                    frame,
                    item,
                    row_style(true, true),
                    frame.area(),
                    TaskRowContext {
                        columns,
                        now_seconds: 0,
                        render_mode,
                        inline_title_editor,
                    },
                );
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

    #[test]
    fn project_filtered_queue_view_groups_by_queue_band() {
        let tasks = vec![
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo medium", "todo", QueueBand::Triage),
            task_item_with("backlog", "backlog", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskSort::Queue, &tasks);

        assert_eq!(view.render_mode, TaskRenderMode::Queue);
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

        let view = TaskListView::from_tasks(TaskSort::Queue, &tasks);

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

        let view = TaskListView::from_tasks(TaskSort::Priority, &tasks);

        assert_eq!(view.render_mode, TaskRenderMode::Default);
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
        let view = TaskListView::from_tasks(TaskSort::Queue, &tasks);

        assert_eq!(view.visual_row(0), 1);
        assert_eq!(view.visual_row(1), 3);
        assert_eq!(view.visual_row(2), 4);
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

        let buffer = render_task_row_buffer_with_mode(&item, TaskRenderMode::Queue, None);
        let rendered = buffer_text(&buffer);

        assert!(rendered.contains("9d"));
        assert!(!rendered.contains("0h"));
    }

    #[test]
    fn empty_task_view_has_no_rows() {
        let view = TaskListView::from_tasks(TaskSort::Queue, &[]);

        assert!(view.rows.is_empty());
        assert_eq!(view.visual_row(0), 0);
    }

    #[test]
    fn compact_age_formats_hours_days_weeks_and_months() {
        assert_eq!(compact_age(6 * 3_600), "6h");
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
}
