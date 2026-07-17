use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, TableState};

use crate::tui::app::Focus;
use crate::tui::columns::ColumnBoard;
use crate::tui::overlay::TextInputView;
use crate::tui::store::TuiStore;
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BG_LANE, BG_LANE_ACTIVE, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED,
    LANE_DIVIDER, ORANGE, RED, SELECTED_BG, YELLOW,
};
use crate::tui::ui::truncate::{truncate_chars, truncate_width};
use crate::tui::widgets::priority_short;
use unicode_width::UnicodeWidthStr;

use super::task_list::{EPIC_MARKER, TaskListHit, render_task_preview};

const CARD_CONTENT_HEIGHT: u16 = 4;
const CARD_HEIGHT: u16 = CARD_CONTENT_HEIGHT + 1;
const HEADER_HEIGHT: u16 = 2;

#[derive(Debug, Clone)]
struct LaneLayout {
    area: Rect,
    cards: Rect,
    start: usize,
    visible: usize,
}

#[derive(Debug, Clone)]
struct ColumnLayout {
    board: Rect,
    preview: Rect,
    lanes: Vec<LaneLayout>,
}

impl ColumnLayout {
    fn new(
        area: Rect,
        board: &ColumnBoard<'_>,
        selected: Option<usize>,
        preview_visible: bool,
    ) -> Self {
        let preview_height = if !preview_visible {
            0
        } else if area.height >= 32 {
            12
        } else if area.height >= 24 {
            8
        } else {
            0
        };
        let [board_area, preview] = if preview_height > 0 {
            Layout::vertical([Constraint::Fill(1), Constraint::Length(preview_height)]).areas(area)
        } else {
            [area, Rect::default()]
        };
        let constraints =
            vec![Constraint::Ratio(1, board.columns.len().max(1) as u32); board.columns.len()];
        let lane_areas = Layout::horizontal(constraints).split(board_area);
        let lanes = board
            .columns
            .iter()
            .zip(lane_areas.iter().copied())
            .enumerate()
            .map(|(column_index, (column, area))| {
                let cards = Rect::new(
                    area.x,
                    area.y.saturating_add(HEADER_HEIGHT),
                    area.width,
                    area.height.saturating_sub(HEADER_HEIGHT),
                );
                let visible = usize::from(cards.height / CARD_HEIGHT);
                let selected_row = selected.and_then(|task_index| {
                    board
                        .position(task_index)
                        .filter(|(index, _)| *index == column_index)
                        .map(|(_, row)| row)
                });
                let start = selected_row
                    .map(|row| row.saturating_sub(visible.saturating_sub(1)))
                    .unwrap_or(0)
                    .min(column.task_indices.len().saturating_sub(visible));
                LaneLayout {
                    area,
                    cards,
                    start,
                    visible,
                }
            })
            .collect();
        Self {
            board: board_area,
            preview,
            lanes,
        }
    }

    fn lane_at(&self, column: u16, row: u16) -> Option<usize> {
        self.lanes.iter().position(|lane| {
            column >= lane.area.x
                && column
                    < lane
                        .area
                        .x
                        .saturating_add(lane.area.width)
                        .saturating_sub(1)
                && row >= lane.area.y
                && row < lane.area.y.saturating_add(HEADER_HEIGHT)
        })
    }

    fn task_at(&self, board: &ColumnBoard<'_>, column: u16, row: u16) -> Option<(usize, u16)> {
        self.lanes
            .iter()
            .enumerate()
            .find_map(|(lane_index, lane)| {
                if column < lane.cards.x
                    || column
                        >= lane
                            .cards
                            .x
                            .saturating_add(lane.cards.width)
                            .saturating_sub(1)
                    || row < lane.cards.y
                    || row >= lane.cards.y.saturating_add(lane.cards.height)
                {
                    return None;
                }
                let row_offset = row - lane.cards.y;
                if row_offset % CARD_HEIGHT >= CARD_CONTENT_HEIGHT {
                    return None;
                }
                let visible_row = usize::from(row_offset / CARD_HEIGHT);
                if visible_row >= lane.visible {
                    return None;
                }
                board.columns[lane_index]
                    .task_indices
                    .get(lane.start + visible_row)
                    .copied()
                    .map(|task_index| {
                        (
                            task_index,
                            (lane.cards.y - self.board.y) + visible_row as u16 * CARD_HEIGHT,
                        )
                    })
            })
    }
}

pub(super) fn render_columns(
    frame: &mut Frame,
    store: &TuiStore,
    table_state: &mut TableState,
    focus: Focus,
    area: Rect,
    inline_title_editor: Option<&TextInputView>,
    marked_task_ids: &BTreeSet<String>,
) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);
    let board = ColumnBoard::new(&store.task_columns, &store.tasks);
    let layout = ColumnLayout::new(
        area,
        &board,
        table_state.selected(),
        store.columns_preview_visible,
    );
    let active_column = table_state
        .selected()
        .and_then(|selected| board.position(selected).map(|(column, _)| column));
    for (index, column) in board.columns.iter().enumerate() {
        let lane = &layout.lanes[index];
        let active = active_column == Some(index);
        let lane_bg = if active { BG_LANE_ACTIVE } else { BG_LANE };
        frame.render_widget(Block::new().style(Style::new().bg(lane_bg)), lane.area);
        if index + 1 < board.columns.len() {
            frame.render_widget(
                Block::new()
                    .borders(Borders::RIGHT)
                    .border_style(Style::new().fg(BORDER)),
                lane.area,
            );
        }
        let header = Rect::new(lane.area.x, lane.area.y, lane.area.width, HEADER_HEIGHT);
        render_lane_header(frame, column, lane, active, header);
        if column.task_indices.is_empty() {
            frame.render_widget(
                Paragraph::new("(empty)").style(Style::new().fg(FG_DIM).bg(lane_bg)),
                lane.cards,
            );
        }
        for (visible_row, task_index) in column
            .task_indices
            .iter()
            .skip(lane.start)
            .take(lane.visible)
            .enumerate()
        {
            let Some(item) = store.tasks.get(*task_index) else {
                continue;
            };
            let selected = table_state.selected() == Some(*task_index);
            let card_bg = if selected {
                if focus == Focus::Tasks {
                    SELECTED_BG
                } else {
                    BG_PANEL
                }
            } else {
                BG_ALT
            };
            let mut style = Style::new().bg(card_bg);
            if item.task.status.is_terminal() && !selected {
                style = style.add_modifier(Modifier::DIM);
            }
            let card = Rect::new(
                lane.cards.x,
                lane.cards.y + visible_row as u16 * CARD_HEIGHT,
                lane.cards.width.saturating_sub(1),
                CARD_CONTENT_HEIGHT,
            );
            let width = card.width.saturating_sub(2) as usize;
            let title_lines = inline_title_editor
                .filter(|_| selected)
                .map(|editor| {
                    vec![
                        super::input::clipped_input_line(&editor.input, editor.cursor, width),
                        Line::from(""),
                    ]
                })
                .unwrap_or_else(|| card_title_lines(&item.task.title, width));
            let label = item.labels.first().map(String::as_str).unwrap_or("");
            let more = item.labels.len().saturating_sub(1);
            let labels = if more > 0 {
                format!("{label} +{more}")
            } else {
                label.to_string()
            };
            let mut marker_spans = terminal_status_spans(item);
            marker_spans.extend(card_marker_spans(
                item,
                marked_task_ids.contains(&item.task.id),
            ));
            let mut text = vec![card_heading_line(item, &[], width)];
            text.extend(title_lines);
            text.push(card_metadata_line(&labels, &marker_spans, width));
            frame.render_widget(Block::new().style(style), card);
            if selected {
                let rail = Rect::new(card.x, card.y, 1, card.height);
                let rail_color = if focus == Focus::Tasks {
                    ACCENT
                } else {
                    BORDER
                };
                frame.render_widget(
                    Paragraph::new(vec![Line::from("▌"); card.height as usize])
                        .style(Style::new().fg(rail_color).bg(card_bg)),
                    rail,
                );
            }
            let content = Rect::new(
                card.x.saturating_add(1),
                card.y,
                card.width.saturating_sub(2),
                card.height,
            );
            frame.render_widget(Paragraph::new(text).style(style), content);
            render_card_separator(
                frame,
                card,
                lane_bg,
                lane.start + visible_row + 1 == column.task_indices.len(),
            );
        }
    }
    if layout.preview.height > 0 {
        render_task_preview(frame, store, table_state.selected(), layout.preview);
    }
}

fn render_card_separator(
    frame: &mut Frame,
    card: Rect,
    lane_bg: ratatui::style::Color,
    lane_end: bool,
) {
    let width = if lane_end {
        card.width.saturating_sub(2).min(5)
    } else {
        card.width
    };
    if width == 0 {
        return;
    }
    let x = if lane_end {
        card.x + (card.width.saturating_sub(width)) / 2
    } else {
        card.x
    };
    let area = Rect::new(x, card.y.saturating_add(card.height), width, 1);
    frame.render_widget(
        Paragraph::new("─".repeat(width as usize)).style(Style::new().fg(LANE_DIVIDER).bg(lane_bg)),
        area,
    );
}

fn render_lane_header(
    frame: &mut Frame,
    column: &crate::tui::columns::TaskColumn<'_>,
    lane: &LaneLayout,
    active: bool,
    area: Rect,
) {
    let inner_width = area.width.saturating_sub(4) as usize;
    let total = column.task_indices.len();
    let end = (lane.start + lane.visible).min(total);
    let status = if total > lane.visible {
        format!(
            "{}{}-{}/{}{}",
            if lane.start > 0 { "↑ " } else { "" },
            lane.start + 1,
            end,
            total,
            if end < total { " ↓" } else { "" }
        )
    } else {
        total.to_string()
    };
    let status_width = UnicodeWidthStr::width(status.as_str());
    let name_width = inner_width.saturating_sub(status_width + 1);
    let name = truncate_width(&column.config.name.to_uppercase(), name_width);
    let gap = " "
        .repeat(inner_width.saturating_sub(UnicodeWidthStr::width(name.as_str()) + status_width));
    let name_style = Style::new()
        .fg(if active { ACCENT } else { FG_MUTED })
        .bg(BG_ALT)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(name, name_style),
            Span::styled(gap, Style::new().bg(BG_ALT)),
            Span::styled(status, Style::new().fg(FG_DIM).bg(BG_ALT)),
        ]))
        .block(
            Block::new()
                .borders(Borders::BOTTOM)
                .border_style(Style::new().fg(if active { ACCENT } else { BORDER }))
                .padding(Padding::horizontal(2)),
        )
        .style(Style::new().bg(BG_ALT)),
        area,
    );
}

fn card_title_lines(title: &str, max_width: usize) -> Vec<Line<'static>> {
    let mut words = title.split_whitespace().peekable();
    let mut lines = [String::new(), String::new()];
    for line in &mut lines {
        while let Some(word) = words.peek().copied() {
            let needed = usize::from(!line.is_empty()) + word.chars().count();
            if !line.is_empty() && line.chars().count() + needed > max_width {
                break;
            }
            if line.is_empty() && word.chars().count() > max_width {
                *line = truncate_chars(word, max_width);
                words.next();
                break;
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            words.next();
        }
    }
    if words.peek().is_some() {
        lines[1] = truncate_chars(&format!("{} …", lines[1]), max_width);
    }
    lines
        .into_iter()
        .map(|line| Line::from(Span::styled(line, Style::new().fg(FG))))
        .collect()
}

fn card_heading_line(
    item: &crate::query::TaskListItem,
    markers: &[Span<'static>],
    max_width: usize,
) -> Line<'static> {
    let priority = if item.task.priority.as_str() == "none" {
        ""
    } else {
        priority_short(item.task.priority.as_str())
    };
    let markers_width = markers
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    let priority_width = UnicodeWidthStr::width(priority);
    let ref_width = max_width.saturating_sub(priority_width + markers_width + 1);
    let display_ref = truncate_width(&item.display_ref, ref_width);
    let used_width = UnicodeWidthStr::width(display_ref.as_str()) + priority_width + markers_width;
    let mut spans = if let Some((project, suffix)) = display_ref.split_once('-') {
        vec![
            Span::styled(
                project.to_string(),
                Style::new().fg(theme::project_color(&item.task.project_key)),
            ),
            Span::styled("-", Style::new().fg(FG_DIM)),
            Span::styled(suffix.to_string(), Style::new().fg(FG_MUTED)),
        ]
    } else {
        vec![Span::styled(display_ref, Style::new().fg(FG_MUTED))]
    };
    spans.push(Span::raw(" ".repeat(max_width.saturating_sub(used_width))));
    if !priority.is_empty() {
        spans.push(Span::styled(
            priority,
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        ));
    }
    spans.extend(markers.iter().cloned());
    Line::from(spans)
}

fn terminal_status_spans(item: &crate::query::TaskListItem) -> Vec<Span<'static>> {
    let label = match item.task.status.as_str() {
        "done" => "✓ done",
        "canceled" => "× canceled",
        _ => return Vec::new(),
    };
    vec![
        Span::raw(" "),
        Span::styled(
            label,
            theme::status_style(item.task.status.as_str()).add_modifier(Modifier::BOLD),
        ),
    ]
}

fn card_marker_spans(item: &crate::query::TaskListItem, marked: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if marked {
        spans.push(Span::styled(" ●", Style::new().fg(ACCENT)));
    }
    if item.has_conflict {
        spans.push(Span::styled(" ⚡", Style::new().fg(RED)));
    }
    if item.unresolved_blocker_count > 0 {
        spans.push(Span::styled(
            format!(" ←{}", item.unresolved_blocker_count),
            Style::new().fg(ORANGE),
        ));
    }
    if item.task.status.is_open()
        && crate::tui::time::due_state_at(&item.task.due_on, crate::queue::now_seconds())
            .needs_action()
    {
        spans.push(Span::styled(
            " !",
            Style::new().fg(RED).add_modifier(Modifier::BOLD),
        ));
    }
    if item.task.is_epic {
        spans.push(Span::styled(
            format!(" {EPIC_MARKER}"),
            Style::new().fg(YELLOW),
        ));
    }
    spans
}

fn card_metadata_line(labels: &str, markers: &[Span<'static>], max_width: usize) -> Line<'static> {
    let marker_width = markers
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    let labels = truncate_width(labels, max_width.saturating_sub(marker_width));
    let labels_width = UnicodeWidthStr::width(labels.as_str());
    let mut spans = vec![Span::styled(labels, Style::new().fg(FG_MUTED))];
    spans.push(Span::raw(
        " ".repeat(max_width.saturating_sub(labels_width + marker_width)),
    ));
    spans.extend(markers.iter().cloned());
    Line::from(spans)
}

pub(crate) fn column_lane_at_position(
    store: &TuiStore,
    table_state: &TableState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<usize> {
    let board = ColumnBoard::new(&store.task_columns, &store.tasks);
    let layout = ColumnLayout::new(
        area,
        &board,
        table_state.selected(),
        store.columns_preview_visible,
    );
    layout.lane_at(column, row)
}

pub(super) fn column_task_at_position(
    store: &TuiStore,
    table_state: &TableState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<TaskListHit> {
    let board = ColumnBoard::new(&store.task_columns, &store.tasks);
    let layout = ColumnLayout::new(
        area,
        &board,
        table_state.selected(),
        store.columns_preview_visible,
    );
    let (task_index, viewport_row) = layout.task_at(&board, column, row)?;
    Some(TaskListHit {
        task_index,
        task_id: store.tasks.get(task_index)?.task.id.clone(),
        viewport_row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choices::{TaskPriority, TaskStatus};
    use crate::query::TaskListItem;

    fn item(index: usize) -> TaskListItem {
        TaskListItem {
            task: crate::types::Task {
                id: index.to_string(),
                workspace_id: "0000000000000001".parse().unwrap(),
                title: format!("task {index}"),
                description: String::new(),
                project_id: "0000000000000001".parse().unwrap(),
                project_key: "app".into(),
                project_prefix: "APP".into(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                created_at: String::new(),
                updated_at: String::new(),
                queue_activity_at: String::new(),
                available_at: String::new(),
                due_on: String::new(),
                deleted: false,
                is_epic: false,
            },
            display_ref: format!("APP-{index}"),
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

    #[test]
    fn layout_reserves_preview_and_keeps_selected_card_visible() {
        let config = vec![crate::config::TaskColumnConfig {
            name: "Work".into(),
            statuses: vec!["todo".into()],
        }];
        let tasks = (0..10).map(item).collect::<Vec<_>>();
        let board = ColumnBoard::new(&config, &tasks);
        let layout = ColumnLayout::new(Rect::new(0, 0, 80, 32), &board, Some(9), true);

        assert_eq!(layout.preview.height, 12);
        assert!(layout.lanes[0].start > 0);
        assert!(layout.lanes[0].start + layout.lanes[0].visible > 9);
    }

    #[test]
    fn layout_gives_preview_space_back_to_board_when_hidden() {
        let config = vec![crate::config::TaskColumnConfig {
            name: "Work".into(),
            statuses: vec!["todo".into()],
        }];
        let tasks = (0..10).map(item).collect::<Vec<_>>();
        let board = ColumnBoard::new(&config, &tasks);

        let layout = ColumnLayout::new(Rect::new(0, 0, 80, 32), &board, Some(0), false);

        assert_eq!(layout.preview.height, 0);
        assert_eq!(layout.board.height, 32);
        assert_eq!(layout.lanes[0].visible, 6);
    }

    #[test]
    fn card_title_wraps_to_two_lines_and_truncates_overflow() {
        let lines = card_title_lines("Support multiple watched directories in settings", 24);
        assert_eq!(lines[0].to_string(), "Support multiple watched");
        assert_eq!(lines[1].to_string(), "directories in settings");

        let lines = card_title_lines(
            "Support multiple watched directories in settings across every workspace",
            24,
        );
        assert!(lines[1].to_string().ends_with('…'));
        assert!(lines[1].to_string().chars().count() <= 24);
    }

    #[test]
    fn card_heading_uses_table_ref_and_priority_colors() {
        let mut task = item(0);
        task.task.priority = TaskPriority::Urgent;
        let line = card_heading_line(&task, &[], 30);

        assert_eq!(
            line.spans[0].style.fg,
            Some(theme::project_color(&task.task.project_key))
        );
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
        assert_eq!(line.spans[2].style.fg, Some(FG_MUTED));
        assert_eq!(line.spans[4].style.fg, theme::priority_style("urgent").fg);
    }

    #[test]
    fn terminal_cards_show_status_markers() {
        let mut task = item(0);
        task.task.status = TaskStatus::Done;
        let done = terminal_status_spans(&task);
        assert_eq!(
            done.iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            " ✓ done"
        );
        assert_eq!(done[1].style.fg, theme::status_style("done").fg);

        task.task.status = TaskStatus::Canceled;
        let canceled = terminal_status_spans(&task);
        assert_eq!(
            canceled
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            " × canceled"
        );
        assert_eq!(canceled[1].style.fg, theme::status_style("canceled").fg);
    }

    #[test]
    fn overdue_cards_show_deadline_marker() {
        let mut task = item(0);
        task.task.due_on = "2000-01-01".to_string();
        let markers = card_marker_spans(&task, false);

        assert!(markers.iter().any(|span| span.content == " !"));
        assert!(markers.iter().any(|span| span.style.fg == Some(RED)));

        task.task.status = TaskStatus::Done;
        assert!(card_marker_spans(&task, false).is_empty());
    }

    #[test]
    fn lane_hit_testing_uses_header_geometry() {
        let config = vec![
            crate::config::TaskColumnConfig {
                name: "Inbox".into(),
                statuses: vec!["inbox".into()],
            },
            crate::config::TaskColumnConfig {
                name: "Ready".into(),
                statuses: vec!["todo".into()],
            },
        ];
        let tasks = vec![item(0)];
        let board = ColumnBoard::new(&config, &tasks);
        let layout = ColumnLayout::new(Rect::new(10, 2, 80, 20), &board, None, true);

        assert_eq!(layout.lane_at(10, 2), Some(0));
        assert_eq!(layout.lane_at(50, 3), Some(1));
        assert_eq!(layout.lane_at(10, 4), None);
    }

    #[test]
    fn hit_testing_uses_rendered_card_geometry() {
        let config = vec![crate::config::TaskColumnConfig {
            name: "Work".into(),
            statuses: vec!["todo".into()],
        }];
        let tasks = vec![item(0), item(1)];
        let board = ColumnBoard::new(&config, &tasks);
        let layout = ColumnLayout::new(Rect::new(10, 2, 80, 20), &board, Some(0), true);
        let cards = layout.lanes[0].cards;

        assert_eq!(layout.task_at(&board, cards.x, cards.y), Some((0, 2)));
        assert_eq!(
            layout.task_at(&board, cards.x + 1, cards.y + CARD_HEIGHT),
            Some((1, 7))
        );
        assert_eq!(
            layout.task_at(&board, cards.x + 1, cards.y + CARD_CONTENT_HEIGHT),
            None
        );
        assert_eq!(layout.task_at(&board, cards.x + 1, cards.y - 1), None);
    }
}
