use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, Padding, Paragraph, Row, Table,
    TableState, Wrap,
};

use crate::query::TaskListItem;
use crate::render::quote;
use crate::tui::app::{Focus, WidgetState};
use crate::tui::store::{SidebarTarget, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, ORANGE, PINK, RED, SELECTED,
    SELECTED_INACTIVE,
};
use crate::tui::widgets::{priority_short, title_cell};

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

    let shell = frame.area();
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(BORDER))
            .style(Style::new().bg(BG)),
        shell,
    );
    let inner = shell.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(3),
    ])
    .areas(inner);

    let [sidebar, main] =
        Layout::horizontal([Constraint::Max(26), Constraint::Fill(1)]).areas(body);

    render_header(frame, store, header);
    render_sidebar(frame, store, widgets, view, sidebar);
    render_tasks(frame, store, widgets, view, main);
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

fn render_sidebar(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    view: &ViewState,
    area: Rect,
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
    let list = List::new(items)
        .block(
            Block::new()
                .title(" VIEWS ")
                .borders(Borders::RIGHT)
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
    let header = Row::new(["  REF", "TITLE", "PROJECT / LABELS", "STATUS", "PRIORITY"])
        .style(
            Style::new()
                .fg(FG_DIM)
                .bg(BG_ALT)
                .add_modifier(Modifier::BOLD),
        )
        .height(1);

    let [table_area, preview_area] = if area.height >= 24 {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(8)]).areas(area)
    } else {
        [area, Rect::default()]
    };
    let (rows, selected) = task_rows(store, widgets.table.selected());

    let highlight_style = if view.focus == Focus::Tasks {
        SELECTED
    } else {
        SELECTED_INACTIVE
    };
    let table = Table::new(
        rows,
        [
            Constraint::Min(8),
            Constraint::Fill(2),
            Constraint::Max(30),
            Constraint::Length(10),
            Constraint::Length(11),
        ],
    )
    .header(header)
    .block(Block::new().style(Style::new().bg(BG)))
    .row_highlight_style(highlight_style);

    let mut visual_state = TableState::default().with_selected(selected);
    frame.render_stateful_widget(table, table_area, &mut visual_state);
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

fn task_rows(store: &TuiStore, selected_task: Option<usize>) -> (Vec<Row<'static>>, Option<usize>) {
    let mut rows = Vec::new();
    let mut visual_selected = None;
    let mut last_status: Option<&str> = None;

    for (task_index, item) in store.tasks.iter().enumerate() {
        if last_status != Some(item.task.status.as_str()) {
            last_status = Some(&item.task.status);
            let count = store
                .tasks
                .iter()
                .filter(|task| task.task.status == item.task.status)
                .count();
            rows.push(group_row(&item.task.status, count));
        }
        if selected_task == Some(task_index) {
            visual_selected = Some(rows.len());
        }
        rows.push(task_row(item, task_index));
    }
    (rows, visual_selected)
}

fn group_row(status: &str, count: usize) -> Row<'static> {
    Row::new([
        Cell::from(""),
        Cell::from(Line::from(vec![
            Span::styled("▸ ", Style::new().fg(ACCENT)),
            Span::styled(
                format!("{} - {count}", status.to_uppercase()),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ])),
        Cell::from(""),
        Cell::from(""),
        Cell::from(""),
    ])
    .style(Style::new().bg(BG_ALT))
}

fn task_row(item: &TaskListItem, _index: usize) -> Row<'static> {
    let dot_color = match item.task.priority.as_str() {
        "urgent" => RED,
        "high" => ORANGE,
        "medium" => ACCENT,
        "low" => FG_DIM,
        _ => BORDER,
    };
    Row::new([
        Cell::from(Line::from(vec![
            Span::styled("● ", Style::new().fg(dot_color)),
            Span::styled(short_ref(&item.display_ref), Style::new().fg(FG_MUTED)),
        ])),
        Cell::from(title_cell(item)),
        Cell::from(project_cell(item)),
        Cell::from(Span::styled(
            format!(" {} ", item.task.status),
            theme::status_style(&item.task.status).add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            priority_short(&item.task.priority),
            theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
        )),
    ])
    .style(Style::new().bg(BG))
}

fn short_ref(display_ref: &str) -> String {
    display_ref
        .rsplit_once('-')
        .map(|(_, suffix)| suffix)
        .unwrap_or(display_ref)
        .to_string()
}

fn project_cell(item: &TaskListItem) -> Line<'static> {
    let mut spans = vec![Span::styled(
        item.task.project_key.clone(),
        Style::new()
            .fg(theme::project_color(&item.task.project_key))
            .add_modifier(Modifier::BOLD),
    )];
    for label in &item.labels {
        spans.push(Span::raw(" "));
        spans.push(crate::tui::widgets::label_pill(label));
    }
    Line::from(spans)
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
