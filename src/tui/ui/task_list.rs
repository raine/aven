use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use super::ViewState;
use super::truncate::truncate_chars;
use crate::query::{TaskListItem, TaskSort};
use crate::queue::{QueueBand, now_seconds, unix_seconds};
use crate::tui::app::{Focus, WidgetState};
use crate::tui::store::{SidebarTarget, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, FG_MUTED, SELECTED, SELECTED_INACTIVE,
};
use crate::tui::widgets::{
    age_style, priority_icon, priority_short, status_chip, status_span, title_cell,
};

pub(super) fn render_tasks(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    view: &ViewState,
    area: Rect,
) {
    let [table_area, preview_area] = if area.height >= 24 {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(8)]).areas(area)
    } else {
        [area, Rect::default()]
    };
    render_task_list(frame, store, widgets.table.selected(), view, table_area);
    if preview_area.height > 0 {
        render_task_preview(frame, store, widgets.table.selected(), preview_area);
    }
}

fn render_task_list(
    frame: &mut Frame,
    store: &TuiStore,
    selected_task: Option<usize>,
    view: &ViewState,
    area: Rect,
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

    let viewport_rows = row_areas.len().saturating_sub(1);
    let selected_row = selected_task
        .map(|selected| task_visual_row(store, selected))
        .unwrap_or(0);
    let scroll = selected_row.saturating_sub(viewport_rows.saturating_sub(1));

    let now_seconds = now_seconds();
    let use_queue_groups = store.active_view == SidebarTarget::All && store.sort == TaskSort::Queue;
    let mut row = 1usize;
    let mut visual_row = 0usize;
    let mut last_status: Option<&str> = None;
    let mut last_queue_band: Option<QueueBand> = None;
    for (task_index, item) in store.tasks.iter().enumerate() {
        let is_new_queue_group = use_queue_groups && last_queue_band != Some(item.queue.band);
        let is_new_status_group =
            !use_queue_groups && last_status != Some(item.task.status.as_str());
        if is_new_queue_group || is_new_status_group {
            last_status = Some(&item.task.status);
            last_queue_band = Some(item.queue.band);
            if visual_row >= scroll && row < row_areas.len() {
                let count = if use_queue_groups {
                    store
                        .tasks
                        .iter()
                        .filter(|task| task.queue.band == item.queue.band)
                        .count()
                } else {
                    store
                        .tasks
                        .iter()
                        .filter(|task| task.task.status == item.task.status)
                        .count()
                };
                let label = if use_queue_groups {
                    item.queue.band.label()
                } else {
                    &item.task.status
                };
                render_group_row(frame, label, count, row_areas[row]);
                row += 1;
            }
            visual_row += 1;
        }
        if visual_row >= scroll && row < row_areas.len() {
            render_task_row(
                frame,
                item,
                row_style(
                    selected_task == Some(task_index),
                    view.focus == Focus::Tasks,
                ),
                row_areas[row],
                columns,
                now_seconds,
                use_queue_groups,
            );
            row += 1;
        }
        visual_row += 1;
        if row >= row_areas.len() {
            break;
        }
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

fn task_visual_row(store: &TuiStore, selected_task: usize) -> usize {
    let mut row = 0;
    let use_queue_groups = store.active_view == SidebarTarget::All && store.sort == TaskSort::Queue;
    let mut last_status: Option<&str> = None;
    let mut last_queue_band: Option<QueueBand> = None;
    for (task_index, item) in store.tasks.iter().enumerate() {
        let is_new_queue_group = use_queue_groups && last_queue_band != Some(item.queue.band);
        let is_new_status_group =
            !use_queue_groups && last_status != Some(item.task.status.as_str());
        if is_new_queue_group || is_new_status_group {
            last_status = Some(&item.task.status);
            last_queue_band = Some(item.queue.band);
            row += 1;
        }
        if task_index == selected_task {
            return row;
        }
        row += 1;
    }
    0
}

fn render_task_header(frame: &mut Frame, area: Rect, columns: [Constraint; 6]) {
    let cells = Layout::horizontal(columns).areas::<6>(area);
    let style = Style::new().fg(BG).bg(BORDER).add_modifier(Modifier::BOLD);
    frame.render_widget(Block::new().style(style), area);
    for (area, label) in cells
        .into_iter()
        .zip([" REF", "TITLE", "PROJECT", "STATUS", "P", "AGE"])
    {
        frame.render_widget(Paragraph::new(label).style(style), area);
    }
}

fn render_group_row(frame: &mut Frame, status: &str, count: usize, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ▸ ", Style::new().fg(ACCENT).bg(BG_ALT)),
            Span::styled(
                format!("{} ({count})", status.to_uppercase()),
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

fn render_task_row(
    frame: &mut Frame,
    item: &TaskListItem,
    style: Style,
    area: Rect,
    columns: [Constraint; 6],
    now_seconds: i64,
    use_queue_groups: bool,
) {
    frame.render_widget(Block::new().style(style), area);
    let cells = Layout::horizontal(columns).areas::<6>(area);
    let age_seconds = if use_queue_groups {
        item.queue.idle_seconds()
    } else {
        task_seconds_since(&item.task.created_at, now_seconds)
    };
    let age_style_input = if use_queue_groups {
        &item.task.updated_at
    } else {
        &item.task.created_at
    };
    let values = [
        task_ref_cell(item),
        title_cell(item, cells[1].width as usize),
        project_cell(item, cells[2].width as usize),
        status_chip(&item.task.status),
        Line::from(Span::styled(
            priority_icon(&item.task.priority),
            theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            age_seconds.map(compact_age).unwrap_or_default(),
            age_style(age_style_input, now_seconds),
        )),
    ];
    for (area, value) in cells.into_iter().zip(values) {
        frame.render_widget(Paragraph::new(value).style(style), area);
    }
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

pub(super) fn labels_display(labels: &[String], separator: &str) -> String {
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join(separator)
    }
}

pub(super) fn description_or_placeholder(description: &str) -> String {
    if description.is_empty() {
        "(no description)".to_string()
    } else {
        description.to_string()
    }
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

    #[test]
    fn compact_age_formats_hours_days_weeks_and_months() {
        assert_eq!(compact_age(6 * 3_600), "6h");
        assert_eq!(compact_age(13 * 86_400), "13d");
        assert_eq!(compact_age(9 * 7 * 86_400), "9w");
        assert_eq!(compact_age(122 * 86_400), "4mo");
    }

    #[test]
    fn project_cell_truncates_with_status_spacing() {
        let item = TaskListItem {
            task: crate::types::Task {
                id: "task-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                title: "Title".to_string(),
                description: String::new(),
                project_key: "very-long-project-name".to_string(),
                project_prefix: "VER".to_string(),
                status: "todo".to_string(),
                priority: "none".to_string(),
                created_at: "2026-06-20T00:00:00Z".to_string(),
                updated_at: "2026-06-20T00:00:00Z".to_string(),
                deleted: false,
            },
            display_ref: "VER-1".to_string(),
            labels: Vec::new(),
            notes: Vec::new(),
            has_conflict: false,
            queue: Default::default(),
        };

        let rendered = project_cell(&item, 10).to_string();

        assert_eq!(rendered, "very-lon… ");
    }

    #[test]
    fn description_or_placeholder_uses_empty_state_copy() {
        assert_eq!(description_or_placeholder(""), "(no description)");
        assert_eq!(description_or_placeholder("Body"), "Body");
    }

    #[test]
    fn labels_display_uses_none_for_empty_labels() {
        assert_eq!(labels_display(&[], ", "), "none");
        assert_eq!(
            labels_display(&["bug".to_string(), "mobile".to_string()], ", "),
            "bug, mobile"
        );
    }
}
