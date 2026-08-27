use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, TableState, Wrap,
};

use super::timestamps::local_timestamp_display;
use crate::change_log::op_type;
use crate::query::RecentActionItem;
use crate::queue::{now_seconds, unix_seconds};
use crate::tui::app::Focus;
use crate::tui::list_surface::ListSurface;
use crate::tui::store::TuiStore;
use crate::tui::text::truncate_width;
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BLUE, BORDER, FG, FG_DIM, FG_MUTED, GREEN, PINK, RED, SELECTED,
    SELECTED_INACTIVE, YELLOW,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecentActionHit {
    pub(crate) action_index: usize,
    pub(crate) viewport_row: u16,
}

pub(crate) fn recent_action_at_position(
    store: &TuiStore,
    table_state: &TableState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<RecentActionHit> {
    let list_area = recent_action_areas(area)[0];
    if column < list_area.x
        || column >= list_area.x.saturating_add(list_area.width)
        || row <= list_area.y
        || row >= list_area.y.saturating_add(list_area.height)
    {
        return None;
    }
    let viewport_rows = list_area.height.saturating_sub(1) as usize;
    if viewport_rows == 0 || store.recent_actions.is_empty() {
        return None;
    }
    let selected = table_state
        .selected()
        .unwrap_or(0)
        .min(store.recent_actions.len() - 1);
    let scroll = scroll_offset(
        table_state.offset(),
        selected,
        store.recent_actions.len(),
        viewport_rows,
    );
    let viewport_row = row.saturating_sub(list_area.y).saturating_sub(1);
    let action_index = scroll.saturating_add(viewport_row as usize);
    (action_index < store.recent_actions.len()).then_some(RecentActionHit {
        action_index,
        viewport_row,
    })
}

pub(super) fn render_recent_actions(
    frame: &mut Frame,
    store: &TuiStore,
    list: &mut ListSurface,
    focus: Focus,
    area: Rect,
) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);
    if area.height == 0 || area.width == 0 {
        return;
    }

    let [list_area, detail_area] = if store.recent_actions.is_empty() {
        [area, Rect::default()]
    } else {
        recent_action_areas(area)
    };
    render_action_list(frame, store, list.table_state_mut(), focus, list_area);
    if detail_area.height > 0 {
        render_action_detail(frame, store, list.selected_task(), detail_area);
    }
}

fn recent_action_areas(area: Rect) -> [Rect; 2] {
    if area.height >= 24 {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(8)]).areas(area)
    } else {
        [area, Rect::default()]
    }
}

fn render_action_list(
    frame: &mut Frame,
    store: &TuiStore,
    table_state: &mut TableState,
    focus: Focus,
    area: Rect,
) {
    let rows = Layout::vertical(vec![Constraint::Length(1); area.height as usize]).split(area);
    if rows.is_empty() {
        return;
    }
    render_header(frame, rows[0]);
    if store.recent_actions.is_empty() {
        let body = Rect {
            y: area.y.saturating_add(1),
            height: area.height.saturating_sub(1),
            ..area
        };
        super::empty_state::render_empty_state(
            frame,
            body,
            super::empty_state::recent_actions_empty_state(store),
        );
        return;
    }

    let viewport_rows = rows.len().saturating_sub(1);
    if viewport_rows == 0 {
        return;
    }
    let selected = table_state
        .selected()
        .unwrap_or(0)
        .min(store.recent_actions.len() - 1);
    table_state.select(Some(selected));
    let scroll = scroll_offset(
        table_state.offset(),
        selected,
        store.recent_actions.len(),
        viewport_rows,
    );
    *table_state.offset_mut() = scroll;
    let now = now_seconds();
    for (viewport_index, action) in store
        .recent_actions
        .iter()
        .enumerate()
        .skip(scroll)
        .take(viewport_rows)
    {
        let row_area = rows[viewport_index - scroll + 1];
        render_action_row(
            frame,
            action,
            now,
            row_area,
            selected == viewport_index,
            focus == Focus::Tasks,
        );
    }
    render_scrollbar(
        frame,
        scroll,
        store.recent_actions.len(),
        viewport_rows,
        area,
    );
}

fn render_header(frame: &mut Frame, area: Rect) {
    let style = Style::new()
        .fg(FG_DIM)
        .bg(BG_ALT)
        .add_modifier(Modifier::BOLD);
    let columns = action_columns(area.width);
    let cells = Layout::horizontal(columns).areas::<5>(area);
    for (cell, label) in
        cells
            .into_iter()
            .zip([" WHEN", "   ACTION", " REF", " PROJECT", "SUMMARY"])
    {
        frame.render_widget(Paragraph::new(label).style(style), cell);
    }
}

fn render_action_row(
    frame: &mut Frame,
    action: &RecentActionItem,
    now: i64,
    area: Rect,
    selected: bool,
    focused: bool,
) {
    let style = if selected {
        if focused { SELECTED } else { SELECTED_INACTIVE }
    } else {
        Style::new().bg(BG)
    };
    frame.render_widget(Block::new().style(style), area);
    let columns = action_columns(area.width);
    let cells = Layout::horizontal(columns).areas::<5>(area);
    let row_style = style;
    let when = compact_age_since(&action.created_at, now).unwrap_or_else(|| "?".to_string());
    let ref_cell = action_ref_cell(action, row_style.bg.unwrap_or(BG));
    let project = action_project_cell(
        action.target.project_key.as_deref(),
        cells[3].width as usize,
        row_style.bg.unwrap_or(BG),
    );
    let sync_marker = if action.synced { "✓" } else { "•" };
    let summary = truncate_width(&action.summary, cells[4].width.saturating_sub(1) as usize);
    let values = [
        Line::from(vec![Span::styled(
            format!(" {when}"),
            Style::new().fg(FG_MUTED).bg(row_style.bg.unwrap_or(BG)),
        )]),
        Line::from(vec![
            Span::styled(" ", row_style),
            Span::styled(
                action_icon(action),
                action_style(action)
                    .bg(row_style.bg.unwrap_or(BG))
                    .remove_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", row_style),
            Span::styled(
                truncate_width(&action.verb, cells[1].width.saturating_sub(4) as usize),
                row_style.fg(FG),
            ),
        ]),
        ref_cell,
        project,
        Line::from(vec![
            Span::styled(summary, row_style.fg(FG)),
            Span::styled(
                format!(" {sync_marker}"),
                Style::new().fg(FG_DIM).bg(row_style.bg.unwrap_or(BG)),
            ),
        ]),
    ];
    for (cell, value) in cells.into_iter().zip(values) {
        frame.render_widget(Paragraph::new(value).style(row_style), cell);
    }
}

fn action_ref_cell(action: &RecentActionItem, bg: ratatui::style::Color) -> Line<'static> {
    let Some(display_ref) = &action.target.display_ref else {
        return Line::from(Span::styled(
            format!(" {}", short_id(&action.entity_id)),
            Style::new().fg(FG_MUTED).bg(bg),
        ));
    };
    let Some((project, suffix)) = display_ref.split_once('-') else {
        return Line::from(Span::styled(
            format!(" {display_ref}"),
            Style::new().fg(FG_MUTED).bg(bg),
        ));
    };
    let project_key = action.target.project_key.as_deref().unwrap_or(project);
    Line::from(vec![
        Span::styled(" ", Style::new().bg(bg)),
        Span::styled(
            project.to_string(),
            Style::new().fg(theme::project_color(project_key)).bg(bg),
        ),
        Span::styled("-", Style::new().fg(FG_DIM).bg(bg)),
        Span::styled(suffix.to_string(), Style::new().fg(FG_MUTED).bg(bg)),
    ])
}

fn action_project_cell(
    project_key: Option<&str>,
    max_width: usize,
    bg: ratatui::style::Color,
) -> Line<'static> {
    let Some(project_key) = project_key else {
        return Line::from("");
    };
    let project = truncate_width(project_key, max_width.saturating_sub(1));
    Line::from(vec![
        Span::styled(project, Style::new().fg(FG_MUTED).bg(bg)),
        Span::styled(" ", Style::new().bg(bg)),
    ])
}

fn render_action_detail(frame: &mut Frame, store: &TuiStore, selected: Option<usize>, area: Rect) {
    let Some(action) = store.selected_recent_action(selected) else {
        return;
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                action_icon(action),
                action_style(action).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                &action.summary,
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("at ", Style::new().fg(FG_DIM)),
            Span::styled(
                local_timestamp_display(&action.created_at),
                Style::new().fg(FG_MUTED),
            ),
            Span::styled("  kind ", Style::new().fg(FG_DIM)),
            Span::styled(action.entity_type.clone(), Style::new().fg(FG_MUTED)),
            Span::styled("  id ", Style::new().fg(FG_DIM)),
            Span::styled(short_id(&action.change_id), Style::new().fg(FG_MUTED)),
        ]),
    ];
    lines.push(Line::from(vec![
        Span::styled("op ", Style::new().fg(FG_DIM)),
        Span::styled(action.op_type.clone(), Style::new().fg(FG_MUTED)),
    ]));
    if let Some(display_ref) = &action.target.display_ref {
        lines.push(Line::from(vec![
            Span::styled("task ", Style::new().fg(FG_DIM)),
            Span::styled(
                display_ref.clone(),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  title ", Style::new().fg(FG_DIM)),
            Span::styled(
                action.target.title.clone().unwrap_or_default(),
                Style::new().fg(FG_MUTED),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("status ", Style::new().fg(FG_DIM)),
            Span::styled(
                action.target.status.clone().unwrap_or_default(),
                Style::new().fg(FG_MUTED),
            ),
        ]));
    }
    if let Some(field) = &action.field {
        lines.push(Line::from(vec![
            Span::styled("field ", Style::new().fg(FG_DIM)),
            Span::styled(field.clone(), Style::new().fg(FG_MUTED)),
        ]));
    }
    if let Some(detail) = &action.detail {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            detail.clone(),
            Style::new().fg(FG_MUTED),
        )));
    }
    let title = if action.target.deleted {
        " ACTION DETAIL × "
    } else {
        " ACTION DETAIL "
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(
                Block::new()
                    .title(title)
                    .borders(Borders::TOP)
                    .border_style(Style::new().fg(BORDER)),
            )
            .wrap(Wrap { trim: false })
            .style(Style::new().fg(FG).bg(BG)),
        area,
    );
}

fn action_columns(width: u16) -> [Constraint; 5] {
    if width < 96 {
        [
            Constraint::Length(8),
            Constraint::Length(13),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Fill(1),
        ]
    } else {
        [
            Constraint::Length(10),
            Constraint::Length(15),
            Constraint::Length(14),
            Constraint::Length(16),
            Constraint::Fill(1),
        ]
    }
}

pub(super) fn action_icon(action: &RecentActionItem) -> &'static str {
    match action.op_type.as_str() {
        op_type::CREATE_TASK => "+",
        op_type::SET_FIELD => field_action_icon(action),
        op_type::LABEL_ADD => "+",
        op_type::LABEL_REMOVE => "-",
        op_type::NOTE_ADD => "✎",
        op_type::NOTE_DELETE => "×",
        op_type::DEPENDENCY_ADD => "←",
        op_type::DEPENDENCY_REMOVE => "-",
        op_type::EPIC_LINK_ADD => "★",
        op_type::EPIC_LINK_REMOVE => "☆",
        op_type::CREATE_PROJECT | op_type::SET_PROJECT_METADATA | op_type::PROJECT_DELETE => "◆",
        op_type::CREATE_LABEL
        | op_type::SET_LABEL_NAME
        | op_type::LABEL_DELETE
        | op_type::LABEL_RESTORE => "#",
        op_type::CREATE_WORKSPACE | op_type::SET_WORKSPACE_FIELD => "◎",
        _ => fallback_action_icon(action),
    }
}

fn field_action_icon(action: &RecentActionItem) -> &'static str {
    match action.field.as_deref().unwrap_or_default() {
        "title" | "description" => "✎",
        "status" => "●",
        "priority" => "▲",
        "project" => "◆",
        "deleted" if action.summary.starts_with("restored") => "↺",
        "deleted" => "×",
        "is_epic" => "★",
        _ => "✎",
    }
}

fn fallback_action_icon(action: &RecentActionItem) -> &'static str {
    match action.accent.as_str() {
        "green" => "+",
        "blue" => "◆",
        "yellow" => "▲",
        "pink" => "✦",
        "red" => "×",
        _ => "•",
    }
}

pub(super) fn action_style(action: &RecentActionItem) -> Style {
    let color = match action.accent.as_str() {
        "green" => GREEN,
        "blue" => BLUE,
        "yellow" => YELLOW,
        "pink" => PINK,
        "red" => RED,
        _ => FG_DIM,
    };
    Style::new().fg(color)
}

fn scroll_offset(current: usize, selected: usize, row_count: usize, viewport_rows: usize) -> usize {
    if row_count <= viewport_rows {
        return 0;
    }
    if selected < current {
        selected
    } else if selected >= current.saturating_add(viewport_rows) {
        selected.saturating_sub(viewport_rows.saturating_sub(1))
    } else {
        current.min(row_count.saturating_sub(viewport_rows))
    }
}

fn render_scrollbar(
    frame: &mut Frame,
    scroll: usize,
    row_count: usize,
    viewport_rows: usize,
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
    let mut state = ScrollbarState::new(row_count)
        .position(scroll)
        .viewport_content_length(viewport_rows);
    frame.render_stateful_widget(
        scrollbar,
        Rect {
            y: area.y.saturating_add(1),
            height: area.height.saturating_sub(1),
            ..area
        },
        &mut state,
    );
}

fn compact_age_since(value: &str, now_seconds: i64) -> Option<String> {
    let seconds = now_seconds.saturating_sub(unix_seconds(value)?).max(0);
    let minutes = seconds / 60;
    if minutes < 60 {
        return Some(format!("{}m", minutes.max(0)));
    }
    let hours = minutes / 60;
    if hours < 24 {
        return Some(format!("{hours}h"));
    }
    let days = hours / 24;
    if days < 14 {
        return Some(format!("{days}d"));
    }
    let weeks = days / 7;
    if weeks < 13 {
        return Some(format!("{weeks}w"));
    }
    Some(format!("{}mo", days / 30))
}

fn short_id(id: &str) -> String {
    id.chars().take(6).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{RecentActionItem, RecentActionTarget};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn action_item(project_key: &str) -> RecentActionItem {
        RecentActionItem {
            change_id: "change-1".to_string(),
            entity_type: "task".to_string(),
            entity_id: "task-1".to_string(),
            op_type: op_type::CREATE_TASK.to_string(),
            field: None,
            created_at: "2026-07-05T00:00:00Z".to_string(),
            synced: true,
            target: RecentActionTarget {
                display_ref: Some("APP-1".to_string()),
                title: Some("Task".to_string()),
                project_key: Some(project_key.to_string()),
                status: Some("todo".to_string()),
                deleted: false,
            },
            verb: "create".to_string(),
            summary: "created task: Task".to_string(),
            detail: None,
            accent: "green".to_string(),
            grouped_change_count: 1,
        }
    }

    fn row_buffer_with_selection(
        action: &RecentActionItem,
        width: u16,
        selected: bool,
    ) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_action_row(frame, action, 0, frame.area(), selected, selected);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_buffer(action: &RecentActionItem, width: u16) -> ratatui::buffer::Buffer {
        row_buffer_with_selection(action, width, false)
    }

    fn row_text(action: &RecentActionItem, width: u16) -> String {
        row_buffer(action, width)
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn selected_action_row_keeps_icon_regular_and_text_bold() {
        let mut action = action_item("app");
        action.op_type = op_type::NOTE_DELETE.to_string();
        action.verb = "delete".to_string();
        action.accent = "red".to_string();

        let buffer = row_buffer_with_selection(&action, 70, true);
        let icon = &buffer[(9, 0)];
        let verb = &buffer[(11, 0)];

        assert_eq!(icon.symbol(), "×");
        assert_eq!(icon.style().fg, Some(RED));
        assert_eq!(icon.style().bg, SELECTED.bg);
        assert!(!icon.style().add_modifier.contains(Modifier::BOLD));
        assert_eq!(verb.symbol(), "d");
        assert_eq!(verb.style().bg, SELECTED.bg);
        assert!(verb.style().add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn action_ref_cell_uses_task_ref_style() {
        let action = action_item("app");

        let buffer = row_buffer(&action, 70);

        assert_eq!(buffer[(22, 0)].symbol(), "A");
        assert_eq!(
            buffer[(22, 0)].style().fg,
            Some(theme::project_color("app"))
        );
        assert_eq!(buffer[(25, 0)].symbol(), "-");
        assert_eq!(buffer[(25, 0)].style().fg, Some(FG_DIM));
        assert_eq!(buffer[(26, 0)].symbol(), "1");
        assert_eq!(buffer[(26, 0)].style().fg, Some(FG_MUTED));
    }

    #[test]
    fn action_project_cell_truncates_with_summary_spacing() {
        let action = action_item("claude-code-procreated");

        let rendered = row_text(&action, 70);

        assert!(rendered.contains("claude-cod… created task"));
        assert!(!rendered.contains("claude-code-procreated task"));
    }

    #[test]
    fn action_project_cell_keeps_trailing_summary_spacing() {
        assert_eq!(action_project_cell(Some("app"), 12, BG).to_string(), "app ");
    }
}
