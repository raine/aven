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
use crate::tui::store::{SidebarTarget, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, ORANGE, PINK, RED, SELECTED,
    SELECTED_INACTIVE,
};
use crate::tui::widgets::{priority_icon, priority_short, title_cell};

#[derive(Clone)]
pub(crate) struct ViewState {
    pub(crate) focus: Focus,
    pub(crate) detail_open: bool,
    pub(crate) help_open: bool,
    pub(crate) search_open: bool,
    pub(crate) search_input: String,
    pub(crate) message: Option<String>,
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
            Paragraph::new("terminal too small for atm tui")
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
        Constraint::Length(3),
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
    frame.render_widget(footer_bar(view), footer);

    if view.detail_open
        && let Some(task) = store.selected_task(widgets.table.selected())
    {
        render_detail(frame, task);
    }
    if view.help_open {
        render_help(frame);
    }
    if view.search_open {
        render_search(frame, view);
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
    let search = store.filters.search.as_ref().map(|search| {
        Span::styled(
            format!("  search {}", quote(search)),
            Style::new().fg(FG_MUTED),
        )
    });
    let mut spans = vec![
        Span::styled("tasks", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
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
    if let Some(search) = search {
        spans.push(search);
    }
    Paragraph::new(Line::from(spans)).style(Style::new().fg(FG).bg(BG))
}

fn header_status() -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled("●", Style::new().fg(ACCENT)),
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

fn footer_bar(view: &ViewState) -> Paragraph<'static> {
    let focus = match view.focus {
        Focus::Sidebar => "sidebar",
        Focus::Tasks => "tasks",
    };
    let message = view
        .message
        .as_deref()
        .map(|message| format!("  {message}"))
        .unwrap_or_default();
    let first = Line::from(vec![
        Span::styled("atm", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(format!("focus {focus}"), Style::new().fg(FG_MUTED)),
        Span::styled(message, Style::new().fg(ORANGE)),
    ]);
    let second = Line::from(vec![
        key("j/k"),
        cmd("navigate"),
        key("Enter"),
        cmd("detail"),
        key("s"),
        cmd("sort"),
        key("1-6"),
        cmd("status"),
        key("p"),
        cmd("priority"),
        key("/"),
        cmd("search"),
        key("d"),
        cmd("delete"),
        key("?"),
        cmd("help"),
        key("q"),
        cmd("quit"),
    ]);
    Paragraph::new(Text::from(vec![first, second]))
        .block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(BORDER)),
        )
        .style(Style::new().fg(FG).bg(BG))
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
            let label_width = area.width.saturating_sub(6) as usize;
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
            let line = Line::from(vec![
                Span::styled(format!("{marker} "), Style::new().fg(color)),
                Span::styled(format!("{:<label_width$}", label), label_style),
                badge(entry.count, is_active_view),
            ]);
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
        Some(SidebarTarget::Todo) => "□",
        Some(SidebarTarget::Active) => "●",
        Some(SidebarTarget::Project(_)) => "●",
        None => " ",
    }
}

fn sidebar_label(entry: &crate::tui::store::SidebarEntry) -> String {
    match entry.target {
        Some(SidebarTarget::All) => "Today's queue".to_string(),
        Some(SidebarTarget::Active) => "All active".to_string(),
        Some(SidebarTarget::Todo) => "All todo".to_string(),
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
    let columns = if area.width < 90 {
        [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(project_width),
            Constraint::Length(8),
            Constraint::Length(3),
            Constraint::Length(5),
        ]
    } else {
        [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(project_width),
            Constraint::Length(10),
            Constraint::Length(3),
            Constraint::Length(5),
        ]
    };
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
        .map(|item| item.task.project_key.chars().count() as u16)
        .max()
        .unwrap_or(7)
        .max(7)
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
        .zip(["REF", "TITLE", "PROJECT", "STATUS", "P", "AGE"])
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
            item.display_ref.clone(),
            Style::new().fg(FG_MUTED),
        )),
        title_cell(item),
        project_cell(item),
        Line::from(Span::styled(
            format!(" {} ", item.task.status),
            theme::status_style(&item.task.status).add_modifier(Modifier::BOLD),
        )),
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

fn render_task_preview(frame: &mut Frame, store: &TuiStore, selected: Option<usize>, area: Rect) {
    let Some(item) = store.selected_task(selected) else {
        return;
    };
    let labels = if item.labels.is_empty() {
        "none".to_string()
    } else {
        item.labels.join(", ")
    };
    let text = Text::from(vec![
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
        ]),
        Line::from(vec![
            Span::styled("project ", Style::new().fg(FG_DIM)),
            Span::styled(
                item.task.project_key.clone(),
                Style::new()
                    .fg(theme::project_color(&item.task.project_key))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  status ", Style::new().fg(FG_DIM)),
            Span::styled(
                format!(" {} ", item.task.status),
                theme::status_style(&item.task.status).add_modifier(Modifier::BOLD),
            ),
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
        Line::from(if item.task.description.is_empty() {
            "(no description)".to_string()
        } else {
            item.task.description.clone()
        }),
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
    frame.render_widget(Clear, area);
    let labels = if item.labels.is_empty() {
        "none".to_string()
    } else {
        item.labels.join(",")
    };
    let deleted = if item.task.deleted { " yes" } else { " no" };
    let text = Text::from(vec![
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
        ]),
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
        Line::from(if item.task.description.is_empty() {
            "(no description)".to_string()
        } else {
            item.task.description.clone()
        }),
    ]);
    let block = overlay_block("Detail");
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn render_help(frame: &mut Frame) {
    let area = centered(frame.area(), 64, 11);
    frame.render_widget(Clear, area);
    let text = Text::from(vec![
        Line::from("j/k or arrows move the focused list"),
        Line::from("Tab switches between views and tasks"),
        Line::from("Enter selects a view or toggles task detail"),
        Line::from("1 inbox  2 backlog  3 todo  4 active  5 done  6 canceled"),
        Line::from("p/P cycle priority, d deletes, u restores"),
        Line::from("/ searches title and description, s cycles sort"),
        Line::from("r refreshes from SQLite, q quits"),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(overlay_block("Help"))
            .style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn render_search(frame: &mut Frame, view: &ViewState) {
    let area = centered(frame.area(), 54, 3);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("/{}▌", view.search_input))
            .block(overlay_block("Search"))
            .style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn overlay_block(title: &'static str) -> Block<'static> {
    Block::new()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG_ALT))
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
}
