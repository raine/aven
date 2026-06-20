use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap,
};

use crate::query::TaskListItem;
use crate::render::quote;
use crate::tui::app::{Focus, WidgetState};
use crate::tui::event::{COMMANDS, CommandLifecycle, CommandSpec, key_label, matching_commands};
use crate::tui::overlay::{
    ConfirmView, MultilineInputView, OverlayView, PickerView, TextInputView, TextPanelView,
};
use crate::tui::store::{SidebarTarget, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, GREEN, ORANGE, PINK, RED,
    SELECTED, SELECTED_INACTIVE,
};
use crate::tui::widgets::{priority_icon, priority_short, status_chip, status_span, title_cell};

#[derive(Clone)]
pub(crate) struct ViewState {
    pub(crate) focus: Focus,
    pub(crate) overlay: Option<OverlayView>,
    pub(crate) message: Option<String>,
    pub(crate) pending_shortcut: Vec<String>,
}

pub(crate) fn render(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    view: &ViewState,
) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), frame.area());

    if frame.area().width < 70 || frame.area().height < 18 {
        frame.render_widget(
            Paragraph::new("terminal too small for aven tui")
                .alignment(Alignment::Center)
                .style(Style::new().fg(FG).bg(BG)),
            frame.area(),
        );
        return;
    }

    let inner = frame.area();

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(inner);

    render_header(frame, store, header);
    if body.width < 100 {
        render_tasks(frame, store, widgets, view, body);
        if view.focus == Focus::Sidebar {
            render_sidebar_overlay(frame, store, widgets, view, body);
        }
    } else {
        let [sidebar, main] =
            Layout::horizontal([Constraint::Max(26), Constraint::Fill(1)]).areas(body);
        render_sidebar(frame, store, widgets, view, sidebar, false);
        render_tasks(frame, store, widgets, view, main);
    }
    frame.render_widget(footer_bar(), footer);

    if !view.pending_shortcut.is_empty()
        && !view
            .overlay
            .as_ref()
            .is_some_and(OverlayView::captures_input)
    {
        render_prefix_hints(frame, view);
    }
    if let Some(message) = &view.message {
        render_toast(frame, message);
    }
    if let Some(overlay) = &view.overlay {
        render_overlay(frame, store, widgets, overlay);
    }
}

fn render_header(frame: &mut Frame, store: &TuiStore, area: Rect) {
    frame.render_widget(
        Block::new()
            .borders(Borders::BOTTOM)
            .border_style(Style::new().fg(BORDER))
            .style(Style::new().bg(BG)),
        area,
    );
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(26)])
        .areas(Rect { height: 1, ..area });

    frame.render_widget(header_tabs(store), left);
    frame.render_widget(header_status(), right);
}

fn header_tabs(store: &TuiStore) -> Paragraph<'static> {
    let conflict_count = store.tasks.iter().filter(|task| task.has_conflict).count();
    let mut spans = vec![
        Span::styled("tasks", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("  workspace {}", store.active_workspace.key),
            Style::new().fg(FG_MUTED),
        ),
        Span::raw("   "),
        tab(
            "queue",
            Some(store.tasks.len()),
            store.sort_label() == "queue",
        ),
        Span::raw("   "),
        tab("projects", None, false),
        Span::raw("   "),
        tab("triage", Some(store.counts.inbox as usize), false),
        Span::raw("   "),
        tab("conflicts", Some(conflict_count), false),
    ];
    spans.extend(active_order_spans(store));
    spans.extend(active_filter_spans(store));
    Paragraph::new(Line::from(spans)).style(Style::new().fg(FG).bg(BG))
}

fn active_order_spans(store: &TuiStore) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!(
            "  order {} {}",
            store.sort_label(),
            store.sort_direction_label()
        ),
        Style::new().fg(FG_MUTED),
    )]
}

fn active_filter_spans(store: &TuiStore) -> Vec<Span<'static>> {
    let mut parts = Vec::new();
    if let Some(project) = &store.filters.project {
        parts.push(format!("project={project}"));
    }
    if let Some(label) = &store.filters.label {
        parts.push(format!("label={label}"));
    }
    if let Some(status) = &store.filters.status {
        parts.push(format!("status={status}"));
    }
    if let Some(priority) = &store.filters.priority {
        parts.push(format!("priority={priority}"));
    }
    if store.filters.include_deleted {
        parts.push("include_deleted".to_string());
    }
    if store.filters.conflicts_only {
        parts.push("conflicts".to_string());
    }
    if let Some(search) = &store.filters.search {
        parts.push(format!("search={}", quote(search)));
    }
    if parts.is_empty() {
        Vec::new()
    } else {
        vec![Span::styled(
            format!("  filter {}", parts.join(" ")),
            Style::new().fg(FG_MUTED),
        )]
    }
}

fn header_status() -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled("●", Style::new().fg(GREEN)),
        Span::styled(" local", Style::new().fg(FG_DIM)),
        Span::styled(format!("  {}", today_short()), Style::new().fg(FG_DIM)),
    ]))
    .alignment(Alignment::Right)
    .style(Style::new().fg(FG_DIM).bg(BG))
}

fn today_short() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_secs() / 86_400) as i64)
        .unwrap_or(0);
    let (_, month, day, weekday) = civil_from_unix_days(days);
    let months = [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!(
        "{} {} {}",
        weekdays[weekday as usize], months[month as usize], day
    )
}

fn civil_from_unix_days(days: i64) -> (i64, u32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    let weekday = (days + 4).rem_euclid(7);
    (year, month as u32, day as u32, weekday as u32)
}

fn footer_bar() -> Paragraph<'static> {
    let hints = Line::from(vec![
        key("j/k"),
        cmd("navigate"),
        key("Enter"),
        cmd("detail"),
        key("a/s/p/l/n/d/x/y"),
        cmd("task"),
        key("g/e/m/f/o/c/C"),
        cmd("prefixes"),
        key("/"),
        cmd("search"),
        key(":"),
        cmd("command"),
        key("?"),
        cmd("help"),
        key("q"),
        cmd("quit"),
    ]);
    Paragraph::new(hints)
        .block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(BORDER)),
        )
        .style(Style::new().fg(FG).bg(BG))
}

fn render_toast(frame: &mut Frame, message: &str) {
    let width = (message.chars().count() as u16)
        .saturating_add(4)
        .clamp(18, frame.area().width.saturating_sub(4));
    let height = 3.min(frame.area().height);
    let x = frame.area().right().saturating_sub(width.saturating_add(2));
    let y = frame
        .area()
        .bottom()
        .saturating_sub(height.saturating_add(3));
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(message.to_string())
            .block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(ORANGE))
                    .padding(Padding::horizontal(1)),
            )
            .style(Style::new().fg(FG).bg(BG_PANEL)),
        area,
    );
}

fn render_sidebar_overlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    view: &ViewState,
    area: Rect,
) {
    let width = area.width.saturating_sub(4).min(34);
    let height = area.height.saturating_sub(2).min(24);
    let area = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    render_sidebar(frame, store, widgets, view, area, true);
}

fn render_sidebar(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    view: &ViewState,
    area: Rect,
    overlay: bool,
) {
    let content_width = area.width.saturating_sub(if overlay { 2 } else { 1 }) as usize;
    let mut items: Vec<ListItem> = store
        .sidebar_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if entry.section {
                if entry.label.is_empty() || entry.label == "Smart Views" {
                    return ListItem::new(Line::from(""));
                }
                return ListItem::new(
                    Line::from(format!(" {} ", entry.label.to_uppercase())).style(
                        Style::new()
                            .fg(FG_DIM)
                            .bg(BG_ALT)
                            .add_modifier(Modifier::BOLD),
                    ),
                );
            }
            let marker = if index == widgets.sidebar.selected().unwrap_or(usize::MAX) {
                "≡"
            } else {
                sidebar_icon(entry)
            };
            let label = sidebar_label(entry);
            let is_active_view = entry.target.as_ref() == Some(&store.active_view);
            let color = match &entry.target {
                Some(SidebarTarget::Project(project)) => theme::project_color(project),
                Some(SidebarTarget::Active) => FG_MUTED,
                Some(SidebarTarget::Todo) => FG_DIM,
                _ => FG,
            };
            let label_style = if is_active_view {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(FG)
            };
            let line = sidebar_entry_line(
                entry,
                marker,
                &label,
                label_style,
                color,
                is_active_view,
                content_width,
            );
            ListItem::new(line)
        })
        .collect();

    let urgent_count = store
        .tasks
        .iter()
        .filter(|task| task.task.priority == "urgent")
        .count() as i64;
    let conflict_count = store.tasks.iter().filter(|task| task.has_conflict).count() as i64;
    items.extend([
        ListItem::new(Line::from("")),
        ListItem::new(
            Line::from("FILTERS").style(Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD)),
        ),
        filter_item("▲", "urgent", urgent_count, RED, area.width),
        filter_item("⚡", "conflicts", conflict_count, PINK, area.width),
    ]);

    let highlight_style = if view.focus == Focus::Sidebar {
        SELECTED
    } else {
        SELECTED_INACTIVE
    };
    let borders = if overlay {
        Borders::ALL
    } else {
        Borders::RIGHT
    };
    let list = List::new(items)
        .block(
            Block::new()
                .title(" VIEWS ")
                .borders(borders)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(BORDER))
                .style(Style::new().bg(BG)),
        )
        .highlight_style(highlight_style);
    frame.render_stateful_widget(list, area, &mut widgets.sidebar);
}

fn render_tasks(
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

fn tab(label: &str, count: Option<usize>, active: bool) -> Span<'static> {
    let text = match count {
        Some(count) if count > 0 => format!(" {label} {count} "),
        _ => format!(" {label} "),
    };
    let style = if active {
        Style::new()
            .fg(FG)
            .bg(BG_PANEL)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_MUTED)
    };
    Span::styled(text, style)
}

fn key(label: &str) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        Style::new()
            .fg(FG_MUTED)
            .bg(BG_PANEL)
            .add_modifier(Modifier::BOLD),
    )
}

fn cmd(label: &str) -> Span<'static> {
    Span::styled(format!(" {label}  "), Style::new().fg(FG_DIM))
}

fn badge(count: i64, active: bool) -> Span<'static> {
    if count <= 0 {
        return Span::raw(" ");
    }
    let color = if active { ACCENT } else { FG_MUTED };
    Span::styled(
        format!("{count:>2}"),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn sidebar_icon(entry: &crate::tui::store::SidebarEntry) -> &'static str {
    match entry.target {
        Some(SidebarTarget::All) => "○",
        Some(SidebarTarget::Inbox) => "▣",
        Some(SidebarTarget::Todo) => "□",
        Some(SidebarTarget::Active) => "●",
        Some(SidebarTarget::Backlog) => "◌",
        Some(SidebarTarget::Conflicts) => "!",
        Some(SidebarTarget::Project(_)) => "●",
        None => " ",
    }
}

fn sidebar_label(entry: &crate::tui::store::SidebarEntry) -> String {
    match entry.target {
        Some(SidebarTarget::All) => "Today's queue".to_string(),
        Some(SidebarTarget::Inbox) => "Inbox".to_string(),
        Some(SidebarTarget::Active) => "All active".to_string(),
        Some(SidebarTarget::Backlog) => "Backlog".to_string(),
        Some(SidebarTarget::Todo) => "All todo".to_string(),
        Some(SidebarTarget::Conflicts) => "Conflicts".to_string(),
        Some(SidebarTarget::Project(_)) => entry
            .label
            .split_once(' ')
            .map(|(_, name)| name)
            .unwrap_or(&entry.label)
            .trim_end_matches('*')
            .to_string(),
        None => entry.label.clone(),
    }
}

fn sidebar_entry_line(
    entry: &crate::tui::store::SidebarEntry,
    marker: &str,
    label: &str,
    label_style: Style,
    marker_color: Color,
    active: bool,
    width: usize,
) -> Line<'static> {
    let marker_cell = format!("{marker} ");
    let count = entry.count.to_string();
    let reserved_width = marker_cell.chars().count() + count.chars().count() + 1;
    let label_width = width.saturating_sub(reserved_width);
    let label = truncate_sidebar_label(label, label_width);
    let used_width = marker_cell.chars().count() + label.chars().count() + count.chars().count();
    let spacer_width = width.saturating_sub(used_width).max(1);
    let count_style = if active {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_MUTED).add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(marker_cell, Style::new().fg(marker_color)),
        Span::styled(label, label_style),
        Span::raw(" ".repeat(spacer_width)),
        Span::styled(count, count_style),
    ])
}

fn truncate_sidebar_label(label: &str, max_width: usize) -> String {
    let label_len = label.chars().count();
    if label_len <= max_width {
        return label.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut truncated = label.chars().take(max_width - 1).collect::<String>();
    truncated.push('…');
    truncated
}

fn filter_item(icon: &str, label: &str, count: i64, color: Color, width: u16) -> ListItem<'static> {
    let icon_cell = if icon == "⚡" {
        format!("{icon} ")
    } else {
        format!("{icon}  ")
    };
    let count_width = if count > 0 { 2 } else { 1 };
    let label_width = (width as usize).saturating_sub(icon_cell.chars().count() + count_width + 2);
    ListItem::new(Line::from(vec![
        Span::styled(icon_cell, Style::new().fg(color)),
        Span::styled(format!("{label:<label_width$}"), Style::new().fg(FG_MUTED)),
        badge(count, false),
    ]))
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

    let mut row = 1usize;
    let mut visual_row = 0usize;
    let mut last_status: Option<&str> = None;
    for (task_index, item) in store.tasks.iter().enumerate() {
        if last_status != Some(item.task.status.as_str()) {
            last_status = Some(&item.task.status);
            if visual_row >= scroll && row < row_areas.len() {
                let count = store
                    .tasks
                    .iter()
                    .filter(|task| task.task.status == item.task.status)
                    .count();
                render_group_row(frame, &item.task.status, count, row_areas[row]);
                row += 1;
            }
            visual_row += 1;
        }
        if visual_row >= scroll && row < row_areas.len() {
            render_task_row(
                frame,
                item,
                selected_task == Some(task_index),
                view.focus == Focus::Tasks,
                row_areas[row],
                columns,
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
    let mut last_status: Option<&str> = None;
    for (task_index, item) in store.tasks.iter().enumerate() {
        if last_status != Some(item.task.status.as_str()) {
            last_status = Some(&item.task.status);
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

fn render_task_row(
    frame: &mut Frame,
    item: &TaskListItem,
    selected: bool,
    focused: bool,
    area: Rect,
    columns: [Constraint; 6],
) {
    let style = if selected {
        if focused { SELECTED } else { SELECTED_INACTIVE }
    } else {
        Style::new().bg(BG)
    };
    frame.render_widget(Block::new().style(style), area);
    let cells = Layout::horizontal(columns).areas::<6>(area);
    let values = [
        Line::from(Span::styled(
            format!(" {}", item.display_ref),
            Style::new().fg(FG_MUTED),
        )),
        title_cell(item, cells[1].width as usize),
        project_cell(item),
        status_chip(&item.task.status),
        Line::from(Span::styled(
            priority_icon(&item.task.priority),
            theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            task_age(&item.task.created_at),
            Style::new().fg(FG_DIM),
        )),
    ];
    for (area, value) in cells.into_iter().zip(values) {
        frame.render_widget(Paragraph::new(value).style(style), area);
    }
}

fn task_age(created_at: &str) -> String {
    let Some(created_seconds) = unix_seconds(created_at) else {
        return String::new();
    };
    let now_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(created_seconds);
    compact_age(now_seconds.saturating_sub(created_seconds))
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

fn unix_seconds(value: &str) -> Option<i64> {
    let (date, time) = value.trim_end_matches('Z').split_once('T')?;
    let mut date = date.split('-');
    let year = date.next()?.parse::<i64>().ok()?;
    let month = date.next()?.parse::<u32>().ok()?;
    let day = date.next()?.parse::<u32>().ok()?;
    let mut time = time.split(':');
    let hour = time.next()?.parse::<i64>().ok()?;
    let minute = time.next()?.parse::<i64>().ok()?;
    let second = time.next()?.parse::<i64>().ok()?;
    Some(unix_days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn unix_days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i64;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn project_cell(item: &TaskListItem) -> Line<'static> {
    Line::from(Span::styled(
        item.task.project_key.clone(),
        Style::new()
            .fg(theme::project_color(&item.task.project_key))
            .add_modifier(Modifier::BOLD),
    ))
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

fn labels_display(labels: &[String], separator: &str) -> String {
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join(separator)
    }
}

fn description_or_placeholder(description: &str) -> String {
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

fn render_detail(frame: &mut Frame, item: &TaskListItem) {
    let width = frame.area().width.saturating_sub(8).min(84);
    let height = frame.area().height.saturating_sub(4).min(18);
    let area = centered(frame.area(), width, height);
    let labels = labels_display(&item.labels, ",");
    let deleted = if item.task.deleted { " yes" } else { " no" };
    let text = Text::from(vec![
        task_heading_line(item),
        Line::from(""),
        Line::from(format!(
            "project={} status={} priority={} deleted={}",
            item.task.project_key, item.task.status, item.task.priority, deleted
        )),
        Line::from(format!(
            "created={} updated={}",
            item.task.created_at, item.task.updated_at
        )),
        Line::from(format!("labels={labels}")),
        Line::from(""),
        Line::from(description_or_placeholder(&item.task.description)),
    ]);
    render_overlay_paragraph(frame, area, "Detail", text, true);
}

const HELP_COLUMNS: &[&[&str]] = &[
    &["General", "Navigation", "Tasks", "Status", "Priority"],
    &[
        "Views",
        "Add/Create",
        "Metadata",
        "Edit",
        "Filters",
        "Order",
        "Conflict",
        "Config",
    ],
];

fn render_help(frame: &mut Frame, scroll: u16) {
    let area = centered(
        frame.area(),
        frame.area().width.saturating_sub(6).min(112),
        frame.area().height.saturating_sub(4).min(28),
    );
    frame.render_widget(Clear, area);
    let mut inner = overlay_block("Shortcuts");
    let content = inner.inner(area);
    if let Some(title) = help_scroll_title(scroll, content.height) {
        inner = inner
            .title_top(Line::from(Span::styled(title, Style::new().fg(FG_MUTED))).right_aligned());
    }
    frame.render_widget(inner, area);
    let [left, _, right] = Layout::horizontal([
        Constraint::Ratio(1, 2),
        Constraint::Length(4),
        Constraint::Ratio(1, 2),
    ])
    .areas(content);
    for (column, sections) in [left, right].into_iter().zip(HELP_COLUMNS.iter()) {
        render_help_column(frame, column, sections, scroll);
    }
}

fn render_help_column(frame: &mut Frame, area: Rect, sections: &[&'static str], scroll: u16) {
    let lines = help_column_lines(sections);
    let visible = lines
        .into_iter()
        .skip(scroll as usize)
        .take(area.height as usize)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn help_scroll_title(scroll: u16, visible_rows: u16) -> Option<String> {
    let max_rows = HELP_COLUMNS
        .iter()
        .map(|sections| help_column_lines(sections).len())
        .max()
        .unwrap_or(0);
    let visible_rows = visible_rows as usize;
    if max_rows <= visible_rows {
        return None;
    }
    let total = max_rows.saturating_sub(visible_rows).saturating_add(1);
    let current = (scroll as usize).saturating_add(1).min(total);
    Some(format!(" {current}/{total} "))
}

fn help_column_lines(sections: &[&'static str]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for section in sections {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            *section,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        for command in COMMANDS
            .iter()
            .filter(|command| command.section == *section)
        {
            lines.push(help_command_line(command));
        }
    }
    lines
}

pub(crate) fn help_scroll_cap(frame_height: u16) -> u16 {
    let visible_rows = frame_height.saturating_sub(4).min(28).saturating_sub(2) as usize;
    HELP_COLUMNS
        .iter()
        .map(|sections| {
            help_column_lines(sections)
                .len()
                .saturating_sub(visible_rows)
        })
        .max()
        .unwrap_or(0) as u16
}

fn command_name_style(command: &CommandSpec) -> Style {
    match command.lifecycle {
        CommandLifecycle::Implemented => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        CommandLifecycle::Planned { .. } => Style::new().fg(FG_MUTED),
        CommandLifecycle::Disabled { .. } => Style::new().fg(FG_DIM),
    }
}

fn lifecycle_badge(lifecycle: CommandLifecycle) -> Option<Span<'static>> {
    match lifecycle {
        CommandLifecycle::Implemented => None,
        CommandLifecycle::Planned { .. } => {
            Some(Span::styled(" planned ", Style::new().fg(ORANGE)))
        }
        CommandLifecycle::Disabled { .. } => {
            Some(Span::styled(" disabled ", Style::new().fg(FG_DIM)))
        }
    }
}

fn command_hint_line(leading: Span<'static>, command: &CommandSpec) -> Line<'static> {
    let mut spans = vec![
        leading,
        Span::styled(
            format!(":{:<18}", command.name),
            command_name_style(command),
        ),
    ];
    if let Some(badge) = lifecycle_badge(command.lifecycle) {
        spans.push(badge);
    }
    spans.push(Span::styled(command.description, Style::new().fg(FG_DIM)));
    Line::from(spans)
}

fn help_command_line(command: &CommandSpec) -> Line<'static> {
    let keys = command
        .keys
        .iter()
        .map(|key| key.label)
        .collect::<Vec<_>>()
        .join("/");
    let mut spans = vec![Span::styled(
        format!("{keys:<14}"),
        Style::new().fg(FG_MUTED),
    )];
    if let Some(badge) = lifecycle_badge(command.lifecycle) {
        spans.push(badge);
    }
    spans.push(Span::styled(command.description, Style::new().fg(FG_DIM)));
    Line::from(spans)
}

fn command_line(command: &CommandSpec) -> Line<'static> {
    let keys = command
        .keys
        .iter()
        .map(|key| key.label)
        .collect::<Vec<_>>()
        .join("/");
    command_hint_line(
        Span::styled(format!("{keys:<10}"), Style::new().fg(FG_MUTED)),
        command,
    )
}

fn render_command(frame: &mut Frame, input: &str) {
    let matches = matching_commands(input);
    let height = (matches.len().min(8) as u16).saturating_add(3);
    let area = centered(frame.area(), 72, height);

    let mut lines = vec![Line::from(format!(":{input}▌"))];
    for command in matches.into_iter().take(8) {
        lines.push(command_line(command));
    }

    render_overlay_paragraph(frame, area, "Command", Text::from(lines), false);
}

fn render_search(frame: &mut Frame, input: &str) {
    let area = centered(frame.area(), 54, 3);
    render_overlay_paragraph(frame, area, "Search", format!("/{input}▌"), false);
}

fn render_overlay_content(frame: &mut Frame, overlay: &OverlayView) {
    match overlay {
        OverlayView::Help { scroll } => render_help(frame, *scroll),
        OverlayView::Search { input } => render_search(frame, input),
        OverlayView::Command { input } => render_command(frame, input),
        OverlayView::TextInput(state) => render_text_input(frame, state),
        OverlayView::MultilineInput(state) => render_multiline_input(frame, state),
        OverlayView::Picker(state) => render_picker(frame, state),
        OverlayView::Confirm(state) => render_confirm(frame, state),
        OverlayView::TextPanel(state) => render_text_panel(frame, state),
        OverlayView::Detail => {}
    }
}

fn render_overlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    overlay: &OverlayView,
) {
    if matches!(overlay, OverlayView::Detail) {
        if let Some(task) = store.selected_task(widgets.table.selected()) {
            render_detail(frame, task);
        }
        return;
    }
    render_overlay_content(frame, overlay);
}

fn render_text_input(frame: &mut Frame, state: &TextInputView) {
    let area = centered(frame.area(), 54, 5);
    let input = insert_cursor(&state.input, state.cursor);
    let text = Text::from(vec![
        Line::from(Span::styled(&state.prompt, Style::new().fg(FG_DIM))),
        Line::from(input),
        Line::from(Span::styled(
            "Enter submit  Esc cancel",
            Style::new().fg(FG_MUTED),
        )),
    ]);
    render_overlay_paragraph(frame, area, &state.title, text, false);
}

fn render_multiline_input(frame: &mut Frame, state: &MultilineInputView) {
    let visible_rows = 10usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(5).min(16);
    let area = centered(frame.area(), 60, height);
    let start = state.row.saturating_sub(visible_rows.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        &state.prompt,
        Style::new().fg(FG_DIM),
    ))];
    for (row_index, line) in state
        .lines
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
    {
        let text = if row_index == state.row {
            insert_cursor(line, state.column)
        } else {
            line.clone()
        };
        lines.push(Line::from(text));
    }
    lines.push(Line::from(Span::styled(
        "Ctrl+S submit  Esc cancel",
        Style::new().fg(FG_MUTED),
    )));
    render_overlay_paragraph(frame, area, &state.title, Text::from(lines), true);
}

fn render_picker(frame: &mut Frame, state: &PickerView) {
    let visible_count = state.visible_indices.len().max(1);
    let viewport_rows = 8usize;
    let height = (visible_count.min(viewport_rows) as u16).saturating_add(5);
    let area = centered(frame.area(), 60, height);
    let selected_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    let start = selected_position.saturating_sub(viewport_rows.saturating_sub(1));
    let mut lines = vec![Line::from(format!("/{}▌", state.filter)), Line::from("")];
    for index in state.visible_indices.iter().skip(start).take(viewport_rows) {
        let item = &state.items[*index];
        let marker = if *index == state.selected {
            "▸ "
        } else {
            "  "
        };
        let check = if state.multi && item.selected {
            " ✓"
        } else {
            ""
        };
        lines.push(Line::from(format!("{marker}{}{check}", item.label)));
    }
    let hints = if state.multi {
        "Up/Down or Ctrl+N/P move  Space toggle  Enter submit  Esc cancel"
    } else {
        "Up/Down or Ctrl+N/P move  Enter submit  Esc cancel"
    };
    lines.push(Line::from(Span::styled(hints, Style::new().fg(FG_MUTED))));
    render_overlay_paragraph(frame, area, &state.title, Text::from(lines), false);
}

fn render_confirm(frame: &mut Frame, state: &ConfirmView) {
    let area = centered(frame.area(), 48, 5);
    let text = Text::from(vec![
        Line::from(state.prompt.as_str()),
        Line::from(Span::styled(
            "y yes  n no  Esc cancel",
            Style::new().fg(FG_MUTED),
        )),
    ]);
    render_overlay_paragraph(frame, area, &state.title, text, false);
}

fn render_text_panel(frame: &mut Frame, state: &TextPanelView) {
    let visible_rows = 12usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(4).min(16);
    let area = centered(frame.area(), 60, height);
    let start = (state.scroll as usize).min(state.lines.len().saturating_sub(1));
    let mut lines = state
        .lines
        .iter()
        .skip(start)
        .take(visible_rows)
        .map(|line| Line::from(line.as_str()))
        .collect::<Vec<_>>();
    lines.push(Line::from(Span::styled(
        "j/k scroll  Enter/Esc close",
        Style::new().fg(FG_MUTED),
    )));
    render_overlay_paragraph(frame, area, &state.title, Text::from(lines), true);
}

fn insert_cursor(input: &str, cursor: usize) -> String {
    let cursor = cursor.min(input.len());
    let (before, after) = input.split_at(cursor);
    format!("{before}▌{after}")
}

fn overlay_block(title: &str) -> Block<'_> {
    Block::new()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG_ALT))
}

fn render_overlay_paragraph<'a>(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    text: impl Into<Text<'a>>,
    wrap: bool,
) {
    frame.render_widget(Clear, area);
    let mut paragraph = Paragraph::new(text)
        .block(overlay_block(title))
        .style(Style::new().fg(FG).bg(BG_ALT));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, area);
}

fn prefix_hint_lines(pending: &[String]) -> Vec<Line<'static>> {
    COMMANDS
        .iter()
        .flat_map(|command| {
            command.keys.iter().filter_map(move |key| {
                if key.codes.len() <= pending.len() {
                    return None;
                }
                let labels: Vec<String> = key.codes.iter().map(|code| key_label(*code)).collect();
                if labels.len() <= pending.len()
                    || !labels
                        .iter()
                        .zip(pending.iter())
                        .all(|(actual, expected)| actual == expected)
                {
                    return None;
                }
                let next_key = labels[pending.len()].clone();
                Some(command_hint_line(
                    Span::styled(
                        format!(" {:<6} ", next_key),
                        Style::new().fg(FG_MUTED).bg(BG_PANEL),
                    ),
                    command,
                ))
            })
        })
        .collect()
}

fn render_prefix_hints(frame: &mut Frame, view: &ViewState) {
    let lines = prefix_hint_lines(&view.pending_shortcut);
    if lines.is_empty() {
        return;
    }
    let height = (lines.len().min(8) as u16).saturating_add(2);
    let area = centered(frame.area(), 72, height);
    frame.render_widget(Clear, area);
    let title = format!("{} …", view.pending_shortcut.join(" "));
    let block = overlay_block(&title);
    frame.render_widget(
        Paragraph::new(Text::from(lines.into_iter().take(8).collect::<Vec<_>>()))
            .block(block)
            .style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width.min(area.width.saturating_sub(2)))])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(
        height.min(area.height.saturating_sub(2)),
    )])
    .flex(Flex::Center)
    .areas(area);
    area
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::{
        ConfirmView, MultilineInputView, OverlayView, PickerItem, PickerView, TextInputView,
        TextPanelView,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn buffer_text(backend: &TestBackend) -> String {
        backend
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn render_overlay_view(overlay: OverlayView) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_overlay_content(frame, &overlay))
            .unwrap();
        buffer_text(terminal.backend())
    }

    #[test]
    fn compact_age_formats_hours_days_weeks_and_months() {
        assert_eq!(compact_age(6 * 3_600), "6h");
        assert_eq!(compact_age(13 * 86_400), "13d");
        assert_eq!(compact_age(9 * 7 * 86_400), "9w");
        assert_eq!(compact_age(122 * 86_400), "4mo");
    }

    #[test]
    fn unix_seconds_parses_utc_timestamp() {
        assert_eq!(unix_seconds("1970-01-02T01:02:03Z"), Some(90_123));
    }

    #[test]
    fn prefix_hint_lines_use_shared_catalog() {
        let lines = prefix_hint_lines(&["m".to_string()]);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":status-active"));
        assert!(rendered.contains(" a "));
    }

    #[test]
    fn command_line_includes_multi_key_label() {
        let command = COMMANDS
            .iter()
            .find(|command| command.name == "status-active")
            .unwrap();
        let line = command_line(command);
        let rendered = line.to_string();
        assert!(rendered.contains("m a"));
    }

    #[test]
    fn command_line_marks_planned_actions() {
        let command = COMMANDS
            .iter()
            .find(|command| command.name == "view-deleted")
            .unwrap();
        let rendered = command_line(command).to_string();
        assert!(rendered.contains("planned"));
    }

    #[test]
    fn prefix_hint_lines_mark_planned_actions() {
        let rendered = prefix_hint_lines(&["g".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":view-deleted"));
        assert!(rendered.contains("planned"));
    }

    #[test]
    fn prefix_hint_lines_show_config_shortcuts_without_planned_badge() {
        let rendered = prefix_hint_lines(&["C".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":config-status"));
        assert!(rendered.contains(":config-show"));
        assert!(rendered.contains(":config-paths"));
        assert!(rendered.contains(":config-init"));
        assert!(!rendered.contains("planned"));
    }

    #[test]
    fn header_shows_active_filters() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.filters = crate::query::TaskFilters {
                project: Some("mobile-app".to_string()),
                label: Some("needs-review".to_string()),
                status: Some("todo".to_string()),
                priority: Some("high".to_string()),
                include_deleted: true,
                conflicts_only: true,
                search: Some("needle".to_string()),
            };
            active_filter_spans(&store)
                .into_iter()
                .map(|span| span.content)
                .collect::<Vec<_>>()
                .join("")
        });
        assert!(rendered.contains("project=mobile-app"));
        assert!(rendered.contains("label=needs-review"));
        assert!(rendered.contains("status=todo"));
        assert!(rendered.contains("priority=high"));
        assert!(rendered.contains("include_deleted"));
        assert!(rendered.contains("conflicts"));
        assert!(rendered.contains("search="));
    }

    #[test]
    fn header_shows_active_ordering() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.sort = crate::query::TaskSort::Priority;
            store.sort_direction = crate::query::SortDirection::Desc;
            active_order_spans(&store)
                .into_iter()
                .map(|span| span.content)
                .collect::<Vec<_>>()
                .join("")
        });
        assert!(rendered.contains("order priority desc"));
    }

    #[test]
    fn header_shows_active_workspace() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.active_workspace.key = "client-work".to_string();
            let backend = TestBackend::new(120, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render_header(frame, &store, frame.area()))
                .unwrap();
            buffer_text(terminal.backend())
        });
        assert!(rendered.contains("workspace client-work"));
    }

    #[test]
    fn command_line_marks_disabled_actions() {
        let command = COMMANDS
            .iter()
            .find(|command| command.name == "order-due")
            .unwrap();
        assert!(command_line(command).to_string().contains("disabled"));
    }

    #[test]
    fn prefix_hint_lines_mark_disabled_actions() {
        let rendered = prefix_hint_lines(&["o".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":order-due"));
        assert!(rendered.contains("disabled"));
    }

    #[test]
    fn overlay_render_includes_text_panel_content_and_hint() {
        let rendered = render_overlay_view(OverlayView::TextPanel(TextPanelView {
            title: "Conflict details".to_string(),
            lines: vec![
                "field=title".to_string(),
                "local a: local title".to_string(),
            ],
            scroll: 0,
        }));
        assert!(rendered.contains("Conflict details"));
        assert!(rendered.contains("field=title"));
        assert!(rendered.contains("Enter/Esc close"));
    }

    #[test]
    fn overlay_render_includes_search_title_and_input() {
        let rendered = render_overlay_view(OverlayView::Search {
            input: "query".to_string(),
        });
        assert!(rendered.contains("Search"));
        assert!(rendered.contains("/query"));
    }

    #[test]
    fn overlay_render_includes_command_title_and_input() {
        let rendered = render_overlay_view(OverlayView::Command {
            input: "ref".to_string(),
        });
        assert!(rendered.contains("Command"));
        assert!(rendered.contains(":ref"));
    }

    #[test]
    fn overlay_render_includes_help_title() {
        let rendered = render_overlay_view(OverlayView::Help { scroll: 0 });
        assert!(rendered.contains("Shortcuts"));
    }

    #[test]
    fn help_overlay_omits_command_names() {
        let rendered = render_overlay_view(OverlayView::Help { scroll: 0 });
        assert!(rendered.contains("quit the TUI"));
        assert!(!rendered.contains(":quit"));
    }

    #[test]
    fn help_overlay_shows_scroll_position() {
        let rendered = render_overlay_view(OverlayView::Help { scroll: 1 });
        assert!(rendered.contains("2/"));
    }

    #[test]
    fn overlay_render_includes_text_input_prompt_and_hints() {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
            title: "Edit title".to_string(),
            prompt: "New title".to_string(),
            input: "alpha".to_string(),
            cursor: 5,
        }));
        assert!(rendered.contains("Edit title"));
        assert!(rendered.contains("New title"));
        assert!(rendered.contains("Enter submit"));
    }

    #[test]
    fn overlay_render_includes_multiline_ctrl_s_hint() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            title: "Description".to_string(),
            prompt: "Body".to_string(),
            lines: vec!["line one".to_string()],
            row: 0,
            column: 4,
        }));
        assert!(rendered.contains("Description"));
        assert!(rendered.contains("Ctrl+S submit"));
    }

    #[test]
    fn overlay_render_includes_picker_filter_and_hints() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: "app".to_string(),
            items: vec![PickerItem {
                label: "APP app".to_string(),
                value: "app".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: true,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains("Project"));
        assert!(rendered.contains("/app"));
        assert!(rendered.contains("Ctrl+N/P move"));
        assert!(rendered.contains("Space toggle"));
    }

    #[test]
    fn picker_viewport_keeps_selected_item_visible() {
        let items = (0..12)
            .map(|index| PickerItem {
                label: format!("Item {index}"),
                value: index.to_string(),
                selected: false,
            })
            .collect::<Vec<_>>();
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: String::new(),
            items,
            selected: 10,
            multi: false,
            visible_indices: (0..12).collect(),
        }));
        assert!(rendered.contains("▸ Item 10"));
        assert!(!rendered.contains("Item 0"));
    }

    #[test]
    fn text_panel_scroll_offset_changes_visible_content() {
        let rendered = render_overlay_view(OverlayView::TextPanel(TextPanelView {
            title: "Long panel".to_string(),
            lines: (0..20).map(|index| format!("Line {index}")).collect(),
            scroll: 8,
        }));
        assert!(rendered.contains("Line 8"));
        assert!(!rendered.contains("Line 0"));
    }

    #[test]
    fn command_rows_render_all_lifecycle_badges_from_catalog() {
        for command in COMMANDS {
            let rendered = command_line(command).to_string();
            assert!(rendered.contains(command.name));
            match command.lifecycle {
                CommandLifecycle::Implemented => {
                    assert!(!rendered.contains("planned"));
                    assert!(!rendered.contains("disabled"));
                }
                CommandLifecycle::Planned { .. } => assert!(rendered.contains("planned")),
                CommandLifecycle::Disabled { .. } => assert!(rendered.contains("disabled")),
            }
        }
    }

    #[test]
    fn help_columns_cover_every_command_section() {
        let sections = HELP_COLUMNS
            .iter()
            .flat_map(|column| column.iter().copied())
            .collect::<Vec<_>>();
        for command in COMMANDS {
            assert!(
                sections.contains(&command.section),
                ":{} section {} is not rendered by help",
                command.name,
                command.section
            );
        }
    }

    #[test]
    fn prefix_hint_lines_include_every_catalog_continuation() {
        let mut prefixes: Vec<Vec<String>> = Vec::new();

        for command in COMMANDS {
            for key in command.keys {
                for len in 1..key.codes.len() {
                    let prefix = key.codes[..len]
                        .iter()
                        .map(|code| key_label(*code))
                        .collect::<Vec<_>>();
                    if !prefixes.contains(&prefix) {
                        prefixes.push(prefix);
                    }
                }
            }
        }

        for prefix in prefixes {
            let rendered = prefix_hint_lines(&prefix)
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");

            for command in COMMANDS {
                for key in command.keys {
                    let labels = key
                        .codes
                        .iter()
                        .map(|code| key_label(*code))
                        .collect::<Vec<_>>();
                    if labels.len() > prefix.len() && labels.starts_with(&prefix) {
                        assert!(
                            rendered.contains(&format!(":{}", command.name)),
                            "prefix {} missing :{}",
                            prefix.join(" "),
                            command.name
                        );
                        assert!(
                            rendered.contains(&format!(" {:<6} ", labels[prefix.len()])),
                            "prefix {} missing next key {}",
                            prefix.join(" "),
                            labels[prefix.len()]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn overlay_render_includes_confirm_prompt_and_hints() {
        let rendered = render_overlay_view(OverlayView::Confirm(ConfirmView {
            title: "Delete".to_string(),
            prompt: "Delete task?".to_string(),
        }));
        assert!(rendered.contains("Delete"));
        assert!(rendered.contains("Delete task?"));
        assert!(rendered.contains("y yes"));
    }
}
