use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};

use crate::query::{TaskListItem, TaskSort};
use crate::queue::{QueueBand, now_seconds, unix_seconds};
use crate::render::quote;
use crate::tui::app::{Focus, WidgetState};
use crate::tui::event::{COMMANDS, CommandLifecycle, CommandSpec, key_label, matching_commands};
use crate::tui::markdown::render_markdown;
use crate::tui::overlay::{
    ConfirmView, MultilineInputView, OverlayView, PickerItem, PickerView, TextInputView,
    TextPanelView,
};
use crate::tui::store::{SidebarTarget, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BG_PANEL, BLUE, BORDER, FG, FG_DIM, FG_MUTED, GREEN, ORANGE, PINK,
    RED, SELECTED, SELECTED_INACTIVE,
};
use crate::tui::widgets::{
    age_style, priority_icon, priority_short, status_chip, status_span, title_cell,
};

#[derive(Clone)]
pub(crate) struct ViewState {
    pub(crate) focus: Focus,
    pub(crate) overlay: Option<OverlayView>,
    pub(crate) detail_underlay: bool,
    pub(crate) message: Option<String>,
    pub(crate) pending_shortcut: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FooterMode {
    List,
    Detail,
}

impl ViewState {
    fn footer_mode(&self) -> FooterMode {
        if matches!(
            self.overlay,
            Some(OverlayView::Detail { .. } | OverlayView::DetailHelp { .. })
        ) {
            FooterMode::Detail
        } else {
            FooterMode::List
        }
    }
}

fn detail_underlay_scroll(overlay: &Option<OverlayView>) -> u16 {
    match overlay {
        Some(OverlayView::Detail { scroll }) => *scroll,
        Some(OverlayView::DetailHelp { .. }) => 0,
        _ => 0,
    }
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
    frame.render_widget(footer_bar(view.footer_mode()), footer);

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
    if view.detail_underlay {
        render_detail_underlay(frame, store, widgets, detail_underlay_scroll(&view.overlay));
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
    let content_area = Rect { height: 1, ..area };
    if area.width >= 84 {
        let status_width = if area.width < 120 { 9 } else { 26 };
        let [left, right] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(status_width)])
                .areas(content_area);
        frame.render_widget(header_line(store, left.width), left);
        frame.render_widget(header_status(area.width < 120), right);
    } else {
        frame.render_widget(header_line(store, content_area.width), content_area);
    }
}

fn header_line(store: &TuiStore, width: u16) -> Paragraph<'static> {
    let compact = width < 120;
    let mut spans = vec![
        Span::styled(" aven", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        separator(),
        Span::styled(
            if compact { "ws " } else { "workspace " },
            Style::new().fg(FG_DIM),
        ),
        Span::styled(
            store.active_workspace.key.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
    ];
    if show_view_badge(store, compact) {
        spans.extend([separator(), Span::styled("view ", Style::new().fg(FG_DIM))]);
        spans.push(view_badge(store));
    }
    spans.push(separator());
    spans.extend(header_metrics(store, compact));
    spans.extend(active_filter_spans(store));
    if !compact || width >= 84 {
        spans.extend(active_order_spans(store));
    }
    Paragraph::new(Line::from(spans)).style(Style::new().fg(FG).bg(BG))
}

fn show_view_badge(store: &TuiStore, compact: bool) -> bool {
    if !compact {
        return true;
    }
    !matches!(
        store.active_view,
        SidebarTarget::All
            | SidebarTarget::Todo
            | SidebarTarget::Inbox
            | SidebarTarget::Conflicts
            | SidebarTarget::Project(_)
    )
}

fn header_metrics(store: &TuiStore, compact: bool) -> Vec<Span<'static>> {
    let metrics = [
        (
            "queue",
            store.counts.all,
            ACCENT,
            store.active_view == SidebarTarget::All,
        ),
        (
            "todo",
            store.counts.todo,
            BLUE,
            store.active_view == SidebarTarget::Todo,
        ),
        (
            "inbox",
            store.counts.inbox,
            FG_MUTED,
            store.active_view == SidebarTarget::Inbox,
        ),
        (
            "conflicts",
            store.counts.conflicts,
            PINK,
            store.active_view == SidebarTarget::Conflicts,
        ),
    ];
    let mut spans = Vec::new();
    for (label, count, color, active) in metrics {
        if compact && count == 0 && !active {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.extend(metric(label, count, color, active));
    }
    spans
}

fn separator() -> Span<'static> {
    Span::styled(" │ ", Style::new().fg(BORDER))
}

fn view_badge(store: &TuiStore) -> Span<'static> {
    Span::styled(
        format!(" {} ", active_view_label(store)),
        Style::new()
            .fg(FG)
            .bg(BG_PANEL)
            .add_modifier(Modifier::BOLD),
    )
}

fn active_view_label(store: &TuiStore) -> String {
    match &store.active_view {
        SidebarTarget::All => "queue".to_string(),
        SidebarTarget::Inbox => "inbox".to_string(),
        SidebarTarget::Active => "active".to_string(),
        SidebarTarget::Backlog => "backlog".to_string(),
        SidebarTarget::Todo => "todo".to_string(),
        SidebarTarget::Done => "done".to_string(),
        SidebarTarget::Conflicts => "conflicts".to_string(),
        SidebarTarget::Project(project) => format!("project {project}"),
    }
}

fn active_view_status_matches(store: &TuiStore, status: &str) -> bool {
    matches!(
        (&store.active_view, status),
        (SidebarTarget::Inbox, "inbox")
            | (SidebarTarget::Active, "active")
            | (SidebarTarget::Backlog, "backlog")
            | (SidebarTarget::Todo, "todo")
            | (SidebarTarget::Done, "done")
    )
}

fn metric(label: &str, count: i64, color: Color, active: bool) -> Vec<Span<'static>> {
    let fill = if active { color } else { BG_PANEL };
    let fg = if active { BG } else { color };
    let style = Style::new().fg(fg).bg(fill).add_modifier(Modifier::BOLD);
    let edge_style = Style::new().fg(fill).bg(BG);
    vec![
        Span::styled("".to_string(), edge_style),
        Span::styled(format!("{label} {count}"), style),
        Span::styled("".to_string(), edge_style),
    ]
}

fn active_order_spans(store: &TuiStore) -> Vec<Span<'static>> {
    vec![
        separator(),
        Span::styled("order ", Style::new().fg(FG_DIM)),
        Span::styled(
            store.sort_label(),
            Style::new().fg(FG_MUTED).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", store.sort_direction_label()),
            Style::new().fg(FG_DIM),
        ),
    ]
}

fn active_filter_spans(store: &TuiStore) -> Vec<Span<'static>> {
    let mut parts = Vec::new();
    if let Some(project) = &store.filters.project {
        parts.push(format!("project={project}"));
    }
    if let Some(label) = &store.filters.label {
        parts.push(format!("label={label}"));
    }
    if let Some(status) = &store.filters.status
        && !active_view_status_matches(store, status)
    {
        parts.push(format!("status={status}"));
    }
    if let Some(priority) = &store.filters.priority {
        parts.push(format!("priority={priority}"));
    }
    if store.filters.include_deleted {
        parts.push("include_deleted".to_string());
    }
    if store.filters.conflicts_only && store.active_view != SidebarTarget::Conflicts {
        parts.push("conflicts".to_string());
    }
    if let Some(search) = &store.filters.search {
        parts.push(format!("search={}", quote(search)));
    }
    if parts.is_empty() {
        Vec::new()
    } else {
        vec![
            separator(),
            Span::styled("filter ", Style::new().fg(FG_DIM)),
            Span::styled(
                parts.join(" "),
                Style::new().fg(FG_MUTED).add_modifier(Modifier::BOLD),
            ),
        ]
    }
}

fn header_status(compact: bool) -> Paragraph<'static> {
    let mut spans = vec![
        Span::styled("●", Style::new().fg(GREEN)),
        Span::styled(" local", Style::new().fg(FG_DIM)),
    ];
    if !compact {
        spans.push(Span::styled(
            format!("  {}", today_short()),
            Style::new().fg(FG_DIM),
        ));
    }
    Paragraph::new(Line::from(spans))
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

fn footer_bar(mode: FooterMode) -> Paragraph<'static> {
    let mut spans = Vec::new();
    let hints: &[(&str, &str)] = match mode {
        FooterMode::List => &[
            ("j/k", "navigate"),
            ("Enter", "detail"),
            ("a/s/p/l/n/d/x/y", "task"),
            ("g/e/m/f/o/c/C", "prefixes"),
            ("/", "search"),
            (":", "command"),
            ("?", "help"),
            ("q", "quit"),
        ],
        FooterMode::Detail => &[
            ("j/k Pg", "scroll"),
            ("[/]", "prev/next"),
            ("e", "edit field"),
            ("n", "add note"),
            ("d", "done"),
            ("s/p/l", "edit"),
            ("y/Y", "copy"),
            ("?", "help"),
            ("Esc", "back"),
        ],
    };
    for (keys, label) in hints {
        spans.extend(key(keys));
        spans.push(cmd(label));
    }
    let hints = Line::from(spans);
    Paragraph::new(hints)
        .block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(BORDER)),
        )
        .style(Style::new().fg(FG).bg(BG))
}

fn render_toast(frame: &mut Frame, message: &str) {
    let tone = toast_tone(message);
    let fill = BG_PANEL;
    let content = Line::from(vec![
        Span::styled("", Style::new().fg(fill).bg(BG)),
        Span::styled("▌", Style::new().fg(tone.color).bg(fill)),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled(tone.icon, Style::new().fg(tone.color).bg(fill)),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled(
            message.to_string(),
            Style::new().fg(FG).bg(fill).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled("", Style::new().fg(fill).bg(BG)),
    ]);
    let width = (message.chars().count() as u16)
        .saturating_add(7)
        .clamp(20, frame.area().width.saturating_sub(5));
    let height = 1.min(frame.area().height);
    let x = frame.area().right().saturating_sub(width.saturating_add(3));
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
        Paragraph::new(content).style(Style::new().fg(FG).bg(BG)),
        area,
    );
}

struct ToastTone {
    icon: &'static str,
    color: Color,
}

fn toast_tone(message: &str) -> ToastTone {
    let lower = message.to_ascii_lowercase();
    if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("invalid")
        || lower.contains("unknown")
        || lower.contains("required")
        || lower.starts_with("no ")
        || lower.starts_with("nothing")
    {
        ToastTone {
            icon: "!",
            color: RED,
        }
    } else if lower.contains("ambiguous") || lower.contains("conflict") {
        ToastTone {
            icon: "•",
            color: ORANGE,
        }
    } else {
        ToastTone {
            icon: "✓",
            color: GREEN,
        }
    }
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

fn key(label: &str) -> Vec<Span<'static>> {
    let key_style = Style::new()
        .fg(FG_MUTED)
        .bg(BG_PANEL)
        .add_modifier(Modifier::BOLD);
    let separator_style = Style::new().fg(FG_DIM).bg(BG_PANEL);
    let edge_style = Style::new().fg(BG_PANEL).bg(BG);
    let mut spans = vec![Span::styled("".to_string(), edge_style)];
    for (index, part) in label.split('/').enumerate() {
        if index > 0 {
            spans.push(Span::styled("/".to_string(), separator_style));
        }
        spans.push(Span::styled(part.to_string(), key_style));
    }
    spans.push(Span::styled("".to_string(), edge_style));
    spans
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
        Some(SidebarTarget::Done) => "✓",
        Some(SidebarTarget::Conflicts) => "!",
        Some(SidebarTarget::Project(_)) => "●",
        None => " ",
    }
}

fn sidebar_label(entry: &crate::tui::store::SidebarEntry) -> String {
    match entry.target {
        Some(SidebarTarget::All) => "Queue".to_string(),
        Some(SidebarTarget::Inbox) => "Inbox".to_string(),
        Some(SidebarTarget::Active) => "All active".to_string(),
        Some(SidebarTarget::Backlog) => "Backlog".to_string(),
        Some(SidebarTarget::Todo) => "All todo".to_string(),
        Some(SidebarTarget::Done) => "Done".to_string(),
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

fn render_detail(frame: &mut Frame, item: &TaskListItem, scroll: u16) {
    let area = frame.area();
    let [_, body, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(area);
    frame.render_widget(Clear, body);
    frame.render_widget(Block::new().style(Style::new().bg(BG)), body);
    if body.width == 0 || body.height == 0 {
        return;
    }

    let [content_area, metadata_area] = if body.width >= 96 {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(34)]).areas(body)
    } else {
        [body, Rect::default()]
    };
    let content_area = content_area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    render_detail_content(frame, item, content_area, scroll);
    if metadata_area.width > 0 {
        render_detail_metadata(frame, item, metadata_area);
    }
}

fn keycap_style() -> Style {
    Style::new()
        .fg(FG)
        .bg(BG_PANEL)
        .add_modifier(Modifier::BOLD)
}

fn render_detail_content(frame: &mut Frame, item: &TaskListItem, area: Rect, scroll: u16) {
    let lines = detail_content_lines(item, area.width as usize);
    let visible = area.height as usize;
    let content_height = lines.len().max(1);
    let start = detail_scroll_start(scroll, content_height, visible);
    frame.render_widget(
        Paragraph::new(Text::from(
            lines.into_iter().skip(start).collect::<Vec<_>>(),
        ))
        .style(Style::new().fg(FG).bg(BG)),
        area,
    );
    if content_height > visible {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(FG_DIM).bg(BG))
                .thumb_style(Style::new().fg(FG_MUTED)),
            area,
            &mut ScrollbarState::new(content_height).position(start),
        );
    }
}

fn detail_scroll_start(scroll: u16, content_height: usize, visible: usize) -> usize {
    let max_start = content_height.saturating_sub(visible);
    (scroll as usize).min(max_start)
}

fn detail_content_lines(item: &TaskListItem, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{}  ", item.display_ref),
                Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
            ),
            status_span(&item.task.status),
            Span::raw("  "),
            Span::styled(
                priority_short(&item.task.priority),
                theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            item.task.title.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    lines.extend(quoted_block_lines(
        &description_or_placeholder(&item.task.description),
        width,
        Style::new().fg(FG_MUTED),
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "NOTES",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (", Style::new().fg(FG_DIM)),
        Span::styled("n", keycap_style()),
        Span::styled(" add)", Style::new().fg(FG_DIM)),
    ]));
    if item.notes.is_empty() {
        lines.push(Line::from(Span::styled("none", Style::new().fg(FG_MUTED))));
    } else {
        for note in &item.notes {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(note.created_at.clone(), Style::new().fg(FG_DIM)),
                Span::styled("  you", Style::new().fg(ACCENT)),
            ]));
            lines.extend(note_card_lines(&note.body, width));
        }
    }
    lines
}

fn quoted_block_lines(body: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(3).max(1);
    render_markdown(body, content_width)
        .into_iter()
        .map(|line| {
            let mut spans = line_with_base_style(line, style).spans;
            spans.insert(0, Span::styled("│ ", Style::new().fg(BORDER)));
            Line::from(spans)
        })
        .collect()
}

fn note_card_lines(body: &str, width: usize) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(4).max(1);
    render_markdown(body, content_width)
        .into_iter()
        .map(|line| {
            let mut spans = line_with_base_style(line, Style::new().fg(FG).bg(BG_PANEL)).spans;
            spans.insert(0, Span::styled("  ", Style::new().bg(BG_PANEL)));
            spans.push(Span::styled("  ", Style::new().bg(BG_PANEL)));
            Line::from(spans)
        })
        .collect()
}

fn line_with_base_style(mut line: Line<'static>, base: Style) -> Line<'static> {
    for span in &mut line.spans {
        span.style = base.patch(span.style);
    }
    line
}

fn render_detail_metadata(frame: &mut Frame, item: &TaskListItem, area: Rect) {
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_style(Style::new().fg(BORDER))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(detail_metadata_lines(item))).style(Style::new().fg(FG).bg(BG)),
        inner,
    );
}

fn detail_metadata_lines(item: &TaskListItem) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            " TASK ",
            Style::new().fg(BG).bg(BORDER).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("PROJECT"),
        Line::from(vec![
            Span::styled(
                "● ",
                Style::new().fg(theme::project_color(&item.task.project_key)),
            ),
            Span::styled(
                item.task.project_key.clone(),
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        metadata_label("STATUS"),
        status_chip(&item.task.status),
        Line::from(""),
        metadata_label("PRIORITY"),
        Line::from(Span::styled(
            priority_short(&item.task.priority),
            theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("LABELS"),
        Line::from(labels_display(&item.labels, ", ")),
        Line::from(""),
        metadata_label("REF"),
        Line::from(Span::styled(
            item.display_ref.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("CREATED"),
        Line::from(Span::styled(
            item.task.created_at.clone(),
            Style::new().fg(FG_MUTED),
        )),
        Line::from(""),
        metadata_label("UPDATED"),
        Line::from(Span::styled(
            item.task.updated_at.clone(),
            Style::new().fg(FG_MUTED),
        )),
    ];
    if item.has_conflict {
        lines.extend([
            Line::from(""),
            metadata_label("CONFLICTS"),
            Line::from(Span::styled(
                "yes",
                Style::new().fg(ORANGE).add_modifier(Modifier::BOLD),
            )),
        ]);
    }
    if item.task.deleted {
        lines.extend([
            Line::from(""),
            metadata_label("DELETED"),
            Line::from(Span::styled(
                "yes",
                Style::new().fg(RED).add_modifier(Modifier::BOLD),
            )),
        ]);
    }
    lines
}

fn metadata_label(label: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        label,
        Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
    ))
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

const DETAIL_HELP_SECTIONS: &[&str] = &["General", "Task detail", "Edit", "Status", "Priority"];

struct HelpTopic {
    keys: &'static str,
    description: &'static str,
    section: &'static str,
}

const DETAIL_HELP_TOPICS: &[HelpTopic] = &[
    HelpTopic {
        keys: "Esc/Enter",
        description: "return to the task list",
        section: "General",
    },
    HelpTopic {
        keys: "?",
        description: "toggle task detail help",
        section: "General",
    },
    HelpTopic {
        keys: "j/k Up/Down",
        description: "scroll one line",
        section: "Task detail",
    },
    HelpTopic {
        keys: "Ctrl-d/Ctrl-u PgDn/PgUp",
        description: "scroll one page",
        section: "Task detail",
    },
    HelpTopic {
        keys: "[/]",
        description: "select previous or next task",
        section: "Task detail",
    },
    HelpTopic {
        keys: "n",
        description: "add a note to this task",
        section: "Task detail",
    },
    HelpTopic {
        keys: "y/Y",
        description: "copy display ref or task id",
        section: "Task detail",
    },
    HelpTopic {
        keys: "e t/d/p/l",
        description: "edit title, description, project, labels",
        section: "Edit",
    },
    HelpTopic {
        keys: "s",
        description: "open status picker",
        section: "Status",
    },
    HelpTopic {
        keys: "d/x",
        description: "set status to done or canceled",
        section: "Status",
    },
    HelpTopic {
        keys: "m i/b/t/a",
        description: "set inbox, backlog, todo, active",
        section: "Status",
    },
    HelpTopic {
        keys: "p",
        description: "open priority picker",
        section: "Priority",
    },
    HelpTopic {
        keys: "m 0/l/m/h/u",
        description: "set none, low, medium, high, urgent",
        section: "Priority",
    },
];

fn render_help(frame: &mut Frame, scroll: u16) {
    let width = frame.area().width.saturating_sub(6).min(112);
    let height = frame.area().height.saturating_sub(4).min(28);
    let visible_rows = height.saturating_sub(2);
    let dialog = if let Some(title) = help_scroll_title(scroll, visible_rows) {
        Dialog::new("Shortcuts", width, height)
            .right_title(Line::from(Span::styled(title, Style::new().fg(FG_MUTED))))
    } else {
        Dialog::new("Shortcuts", width, height)
    };
    let content = dialog.render_block(frame);
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

fn render_detail_help(frame: &mut Frame, scroll: u16) {
    let area = centered(frame.area(), 72, 18);
    frame.render_widget(Clear, area);
    let mut block = overlay_block("Task detail shortcuts");
    let content = block.inner(area);
    if let Some(title) = detail_help_scroll_title(scroll, content.height) {
        block = block
            .title_top(Line::from(Span::styled(title, Style::new().fg(FG_MUTED))).right_aligned());
    }
    frame.render_widget(block, area);
    let lines = detail_help_lines();
    let visible = lines
        .into_iter()
        .skip(scroll as usize)
        .take(content.height as usize)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        content,
    );
}

fn detail_help_lines() -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for section in DETAIL_HELP_SECTIONS {
        let mut section_lines = DETAIL_HELP_TOPICS
            .iter()
            .filter(|topic| topic.section == *section)
            .peekable();
        if section_lines.peek().is_none() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            *section,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.extend(section_lines.map(detail_help_line));
    }
    lines
}

fn detail_help_line(topic: &HelpTopic) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<18}", topic.keys), Style::new().fg(FG_MUTED)),
        Span::styled(topic.description, Style::new().fg(FG_DIM)),
    ])
}

fn detail_help_scroll_title(scroll: u16, visible_rows: u16) -> Option<String> {
    let max_rows = detail_help_lines().len();
    let visible_rows = visible_rows as usize;
    if max_rows <= visible_rows {
        return None;
    }
    let total = max_rows.saturating_sub(visible_rows).saturating_add(1);
    let current = (scroll as usize).saturating_add(1).min(total);
    Some(format!(" {current}/{total} "))
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

pub(crate) fn detail_help_scroll_cap(frame_height: u16) -> u16 {
    let visible_rows = frame_height.min(18).saturating_sub(2) as usize;
    detail_help_lines().len().saturating_sub(visible_rows) as u16
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

fn render_command(frame: &mut Frame, input: &str, cursor: usize) {
    let matches = matching_commands(input);
    let height = (matches.len().min(8) as u16).saturating_add(3);

    let mut lines = vec![input_line(":", input, cursor)];
    for command in matches.into_iter().take(8) {
        lines.push(command_line(command));
    }

    Dialog::new("Command", 72, height).render_text(frame, Text::from(lines));
}

fn render_search(frame: &mut Frame, input: &str, cursor: usize) {
    Dialog::new("Search", 54, 3).render_text(frame, input_line("/", input, cursor));
}

fn render_overlay_content(frame: &mut Frame, overlay: &OverlayView) {
    match overlay {
        OverlayView::Help { scroll } => render_help(frame, *scroll),
        OverlayView::DetailHelp { scroll } => render_detail_help(frame, *scroll),
        OverlayView::Search { input, cursor } => render_search(frame, input, *cursor),
        OverlayView::Command { input, cursor } => render_command(frame, input, *cursor),
        OverlayView::TextInput(state) => render_text_input(frame, state),
        OverlayView::MultilineInput(state) => render_multiline_input(frame, state),
        OverlayView::Picker(state) => render_picker(frame, state),
        OverlayView::Confirm(state) => render_confirm(frame, state),
        OverlayView::TextPanel(state) => render_text_panel(frame, state),
        OverlayView::Detail { .. } => {}
    }
}

fn render_detail_underlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    scroll: u16,
) {
    if let Some(task) = store.selected_task(widgets.table.selected()) {
        render_detail(frame, task, scroll);
    }
}

fn render_overlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    overlay: &OverlayView,
) {
    if matches!(
        overlay,
        OverlayView::Detail { .. } | OverlayView::DetailHelp { .. }
    ) {
        let scroll = match overlay {
            OverlayView::Detail { scroll } => *scroll,
            OverlayView::DetailHelp { .. } => 0,
            _ => 0,
        };
        render_detail_underlay(frame, store, widgets, scroll);
        if matches!(overlay, OverlayView::DetailHelp { .. }) {
            render_overlay_content(frame, overlay);
        }
        return;
    }
    render_overlay_content(frame, overlay);
}

fn render_text_input(frame: &mut Frame, state: &TextInputView) {
    if let Some((project, priority)) = add_task_title_metadata(&state.title) {
        let dialog = Dialog::new("Add task", 60, 5);
        let width = dialog.area(frame).width;
        let dialog = dialog.right_title(add_task_metadata_title(project, priority, width));
        let content = dialog.render_block(frame);
        let input = add_task_title_input_line(&state.input, state.cursor, content.width as usize);
        let text = Text::from(vec![input, Line::from(""), add_task_hint_line()]);
        frame.render_widget(
            Paragraph::new(text).style(Style::new().fg(FG).bg(BG_ALT)),
            content,
        );
        return;
    }

    let text = Text::from(vec![
        Line::from(Span::styled(&state.prompt, Style::new().fg(FG_DIM))),
        input_line("", &state.input, state.cursor),
        Line::from(""),
        dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
    ]);
    Dialog::new(&state.title, 54, 6).render_text(frame, text);
}

fn add_task_title_metadata(title: &str) -> Option<(&str, &str)> {
    let value = title.strip_prefix("Add task  project=")?;
    value.split_once(" priority=")
}

fn add_task_title_input_line(input: &str, cursor: usize, width: usize) -> Line<'static> {
    if input.is_empty() {
        return Line::from(vec![
            cursor_cell("t"),
            Span::styled("itle", Style::new().fg(FG_DIM)),
        ]);
    }
    clipped_input_line(input, cursor, width)
}

fn add_task_hint_line() -> Line<'static> {
    dialog_hint_line(&[
        ("Enter", "create"),
        ("Tab", "project"),
        ("Ctrl+P", "priority"),
        ("Esc", "cancel"),
    ])
}

fn add_task_metadata_title(project: &str, priority: &str, width: u16) -> Line<'static> {
    let value_width = (width as usize).saturating_sub(24).max(4) / 2;
    let value_style = Style::new().fg(Color::Rgb(194, 174, 255));
    Line::from(vec![
        Span::styled(" project: ", Style::new().fg(FG_MUTED)),
        Span::styled(truncate_chars(project, value_width), value_style),
        Span::styled(" · ", Style::new().fg(FG_DIM)),
        Span::styled("prio: ", Style::new().fg(FG_MUTED)),
        Span::styled(truncate_chars(priority, value_width), value_style),
    ])
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let mut truncated = value.chars().take(max_chars - 1).collect::<String>();
    truncated.push('…');
    truncated
}

fn render_multiline_input(frame: &mut Frame, state: &MultilineInputView) {
    if state.title == "Add note" {
        render_add_note_input(frame, state);
        return;
    }

    let visible_rows = 10usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(5).min(16);
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
        if row_index == state.row {
            lines.push(input_line("", line, state.column));
        } else {
            lines.push(Line::from(line.clone()));
        }
    }
    lines.push(Line::from(""));
    lines.push(multiline_hint_line());
    Dialog::new(&state.title, 60, height.saturating_add(1))
        .wrap()
        .render_text(frame, Text::from(lines));
}

fn render_add_note_input(frame: &mut Frame, state: &MultilineInputView) {
    let visible_rows = 8usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(3).min(12);
    let area = centered(frame.area(), 60, height);
    let start = state.row.saturating_sub(visible_rows.saturating_sub(1));
    let mut lines = Vec::new();
    for (row_index, line) in state
        .lines
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
    {
        lines.push(add_note_input_line(
            line,
            if row_index == state.row {
                Some(state.column)
            } else {
                None
            },
        ));
    }
    lines.push(multiline_hint_line());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(overlay_block("Add note"))
            .wrap(Wrap { trim: false })
            .style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn add_note_input_line(line: &str, cursor: Option<usize>) -> Line<'static> {
    if line.is_empty() && cursor.is_some() {
        return Line::from(vec![
            Span::styled("▌", Style::new().fg(FG)),
            Span::styled("note body", Style::new().fg(FG_DIM)),
        ]);
    }
    match cursor {
        Some(cursor) => Line::from(input_cursor_spans(line, cursor, InputWidth::Full)),
        None => Line::from(line.to_string()),
    }
}

fn multiline_hint_line() -> Line<'static> {
    dialog_hint_line(&[("Ctrl+S", "submit"), ("Esc", "cancel")])
}

fn render_picker(frame: &mut Frame, state: &PickerView) {
    if let Some(submit_label) = project_picker_submit_label(&state.title) {
        render_project_picker(frame, state, submit_label);
        return;
    }

    let visible_count = state.visible_indices.len().max(1);
    let viewport_rows = 8usize;
    let height = (visible_count.min(viewport_rows) as u16).saturating_add(5);
    let selected_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    let start = selected_position.saturating_sub(viewport_rows.saturating_sub(1));
    let mut lines = vec![
        input_line("/", &state.filter, state.filter_cursor),
        Line::from(""),
    ];
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
        if state.title == "Edit task: priority" {
            lines.push(priority_picker_line(item, *index == state.selected));
        } else {
            lines.push(Line::from(format!("{marker}{}{check}", item.label)));
        }
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line(state.multi, "submit"));
    Dialog::new(&state.title, 60, height.saturating_add(1)).render_text(frame, Text::from(lines));
}

fn priority_picker_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let marker = if selected { "▸ " } else { "  " };
    Line::from(vec![
        Span::raw(marker),
        Span::styled(
            format!("{} ", priority_icon(&item.value)),
            theme::priority_style(&item.value).add_modifier(Modifier::BOLD),
        ),
        Span::styled(item.label.clone(), theme::priority_style(&item.value)),
    ])
}

fn picker_hint_line(multi: bool, submit_label: &str) -> Line<'static> {
    let mut items = vec![("Up/Down", "move"), ("Ctrl+N/P", "move")];
    if multi {
        items.push(("Space", "toggle"));
    }
    items.extend([("Enter", submit_label), ("Esc", "cancel")]);
    dialog_hint_line(&items)
}

fn render_project_picker(frame: &mut Frame, state: &PickerView, submit_label: &'static str) {
    let viewport_rows = 10usize;
    let height = (viewport_rows as u16).saturating_add(6);
    let selected_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    let start = selected_position.saturating_sub(viewport_rows.saturating_sub(1));
    let mut lines = vec![
        prefixed_input_line(
            Span::styled("/", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            &state.filter,
            state.filter_cursor,
        ),
        Line::from(vec![
            Span::styled("  PREFIX ", Style::new().fg(FG_DIM).bg(BG_PANEL)),
            Span::styled("PROJECT", Style::new().fg(FG_DIM).bg(BG_PANEL)),
        ]),
    ];
    let list_start = lines.len();
    for index in state.visible_indices.iter().skip(start).take(viewport_rows) {
        lines.push(project_picker_line(
            &state.items[*index],
            *index == state.selected,
        ));
    }
    if state.visible_indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching projects",
            Style::new().fg(FG_DIM),
        )));
    }
    while lines.len().saturating_sub(list_start) < viewport_rows {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    lines.push(project_picker_hint_line(submit_label));
    Dialog::new(&state.title, 70, height).render_text(frame, Text::from(lines));
}

fn project_picker_submit_label(title: &str) -> Option<&'static str> {
    match title {
        "Go: project" => Some("open"),
        "Delete project" => Some("delete"),
        _ => None,
    }
}

fn project_picker_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let (prefix, name) = item
        .label
        .split_once(' ')
        .unwrap_or((item.label.as_str(), item.value.as_str()));
    let marker = if selected { "▸" } else { " " };
    let row_style = if selected {
        SELECTED
    } else {
        Style::new().bg(BG_ALT)
    };
    let project_style = Style::new()
        .fg(theme::project_color(&item.value))
        .add_modifier(Modifier::BOLD)
        .bg(row_style.bg.unwrap_or(BG_ALT));
    let name_style = Style::new()
        .fg(if selected { FG } else { FG_MUTED })
        .bg(row_style.bg.unwrap_or(BG_ALT));
    Line::from(vec![
        Span::styled(format!("{marker} "), row_style),
        Span::styled(format!("{prefix:<7}"), project_style),
        Span::styled(" ", row_style),
        Span::styled(name.to_string(), name_style),
    ])
}

fn project_picker_hint_line(submit_label: &'static str) -> Line<'static> {
    picker_hint_line(false, submit_label)
}

fn render_confirm(frame: &mut Frame, state: &ConfirmView) {
    let width = state.prompt.chars().count().saturating_add(4).max(32) as u16;
    let text = Text::from(vec![
        Line::from(state.prompt.as_str()),
        Line::from(""),
        confirm_hint_line(),
    ]);
    Dialog::new(&state.title, width, 5).render_text(frame, text);
}

fn confirm_hint_line() -> Line<'static> {
    dialog_hint_line(&[("y", "yes"), ("n", "no"), ("Esc", "cancel")])
}

fn render_text_panel(frame: &mut Frame, state: &TextPanelView) {
    let visible_rows = 12usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(4).min(16);
    let start = (state.scroll as usize).min(state.lines.len().saturating_sub(1));
    let mut lines = state
        .lines
        .iter()
        .skip(start)
        .take(visible_rows)
        .map(|line| Line::from(line.as_str()))
        .collect::<Vec<_>>();
    lines.push(dialog_hint_line(&[
        ("j/k", "scroll"),
        ("Enter/Esc", "close"),
    ]));
    Dialog::new(&state.title, 60, height)
        .wrap()
        .render_text(frame, Text::from(lines));
}

fn input_line(prefix: &'static str, input: &str, cursor: usize) -> Line<'static> {
    if prefix.is_empty() {
        return Line::from(input_cursor_spans(input, cursor, InputWidth::Full));
    }
    prefixed_input_line(Span::raw(prefix), input, cursor)
}

fn prefixed_input_line(prefix: Span<'static>, input: &str, cursor: usize) -> Line<'static> {
    let mut spans = vec![prefix];
    spans.extend(input_cursor_spans(input, cursor, InputWidth::Full));
    Line::from(spans)
}

fn clipped_input_line(input: &str, cursor: usize, width: usize) -> Line<'static> {
    Line::from(input_cursor_spans(
        input,
        cursor,
        InputWidth::Clipped(width),
    ))
}

#[derive(Clone, Copy)]
enum InputWidth {
    Full,
    Clipped(usize),
}

fn input_cursor_spans(input: &str, cursor: usize, width: InputWidth) -> Vec<Span<'static>> {
    let cursor = char_boundary_at_or_before(input, cursor);
    let input_chars = input.chars().count();
    let max_width = match width {
        InputWidth::Full => input_chars.saturating_add(1),
        InputWidth::Clipped(width) => width,
    };
    let Some(cursor_char) = input[cursor..].chars().next() else {
        let before = input
            .chars()
            .skip(input_chars.saturating_sub(max_width.saturating_sub(1)))
            .collect::<String>();
        return vec![Span::raw(before), cursor_cell(" ")];
    };
    let cursor_end = cursor + cursor_char.len_utf8();
    let before = &input[..cursor];
    let after = &input[cursor_end..];
    let after_chars = after.chars().count();
    let value_width = input_chars.saturating_add(1).min(max_width);
    let before_visible = value_width.saturating_sub(1 + after_chars);
    let before = before
        .chars()
        .skip(before.chars().count().saturating_sub(before_visible))
        .collect::<String>();
    vec![
        Span::raw(before),
        cursor_cell(cursor_char.to_string()),
        Span::raw(
            after
                .chars()
                .take(max_width.saturating_sub(1))
                .collect::<String>(),
        ),
    ]
}

fn cursor_cell(content: impl Into<std::borrow::Cow<'static, str>>) -> Span<'static> {
    Span::styled(content, Style::new().fg(BG_ALT).bg(FG))
}

fn char_boundary_at_or_before(input: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(input.len());
    while cursor > 0 && !input.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

struct Dialog<'a> {
    title: &'a str,
    width: u16,
    height: u16,
    wrap: bool,
    right_title: Option<Line<'a>>,
}

impl<'a> Dialog<'a> {
    fn new(title: &'a str, width: u16, height: u16) -> Self {
        Self {
            title,
            width,
            height,
            wrap: false,
            right_title: None,
        }
    }

    fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }

    fn right_title(mut self, title: Line<'a>) -> Self {
        self.right_title = Some(title);
        self
    }

    fn area(&self, frame: &Frame) -> Rect {
        centered(frame.area(), self.width, self.height)
    }

    fn render_block(self, frame: &mut Frame) -> Rect {
        let area = self.area(frame);
        frame.render_widget(Clear, area);
        let mut block = overlay_block(self.title);
        if let Some(title) = self.right_title {
            block = block.title_top(title.right_aligned());
        }
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    }

    fn render_text<'text>(self, frame: &mut Frame, text: impl Into<Text<'text>>) {
        let wrap = self.wrap;
        let inner = self.render_block(frame);
        let mut paragraph = Paragraph::new(text).style(Style::new().fg(FG).bg(BG_ALT));
        if wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        frame.render_widget(paragraph, inner);
    }
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

fn dialog_hint_line(items: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::new().fg(FG_MUTED)));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}"), Style::new().fg(FG_MUTED)));
    }
    Line::from(spans)
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
    let title = format!("{} …", view.pending_shortcut.join(" "));
    Dialog::new(&title, 72, height).render_text(
        frame,
        Text::from(lines.into_iter().take(8).collect::<Vec<_>>()),
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
    fn toast_uses_icon_and_message() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_toast(frame, "filters cleared"))
            .unwrap();
        let rendered = buffer_text(terminal.backend());
        assert!(rendered.contains("✓ filters cleared"));
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
                hide_done: false,
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
    fn header_hides_conflict_filter_for_conflicts_view() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.active_view = SidebarTarget::Conflicts;
            store.filters.conflicts_only = true;
            active_filter_spans(&store)
                .into_iter()
                .map(|span| span.content)
                .collect::<Vec<_>>()
                .join("")
        });
        assert!(!rendered.contains("conflicts"));
    }

    #[test]
    fn header_hides_status_filter_for_matching_view() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.active_view = SidebarTarget::Backlog;
            store.filters.status = Some("backlog".to_string());
            active_filter_spans(&store)
                .into_iter()
                .map(|span| span.content)
                .collect::<Vec<_>>()
                .join("")
        });
        assert!(!rendered.contains("status=backlog"));
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
    fn header_queue_count_ignores_filtered_tasks() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.counts.all = 177;
            store.counts.todo = 34;
            store.tasks.clear();
            let backend = TestBackend::new(120, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render_header(frame, &store, frame.area()))
                .unwrap();
            buffer_text(terminal.backend())
        });
        assert!(rendered.contains("queue 177"));
        assert!(rendered.contains("todo 34"));
    }

    #[test]
    fn header_preserves_metric_padding_on_narrow_widths() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.counts.all = 34;
            store.counts.todo = 33;
            store.counts.inbox = 1;
            let backend = TestBackend::new(96, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render_header(frame, &store, frame.area()))
                .unwrap();
            buffer_text(terminal.backend())
        });
        assert!(rendered.contains("inbox 1"));
        assert!(rendered.contains("● local"));
    }

    #[test]
    fn compact_header_uses_short_workspace_and_drops_redundant_items() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.counts.all = 34;
            store.counts.todo = 33;
            store.counts.inbox = 1;
            store.counts.conflicts = 0;
            let backend = TestBackend::new(110, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render_header(frame, &store, frame.area()))
                .unwrap();
            buffer_text(terminal.backend())
        });
        assert!(rendered.contains("ws default"));
        assert!(!rendered.contains("workspace default"));
        assert!(!rendered.contains("view queue"));
        assert!(!rendered.contains("conflicts 0"));
        assert!(rendered.contains("order queue asc"));
    }

    #[test]
    fn compact_header_shows_project_as_filter_instead_of_view() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rendered = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let pool = crate::db::open_db(&dir.path().join("test.db"))
                .await
                .unwrap();
            let mut store = TuiStore::new(pool).await.unwrap();
            store.active_view = SidebarTarget::Project("agent-offload".to_string());
            store.filters.project = Some("agent-offload".to_string());
            let backend = TestBackend::new(110, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render_header(frame, &store, frame.area()))
                .unwrap();
            buffer_text(terminal.backend())
        });
        assert!(!rendered.contains("view project agent-offload"));
        assert!(rendered.contains("filter project=agent-offload"));
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
            let backend = TestBackend::new(150, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render_header(frame, &store, frame.area()))
                .unwrap();
            buffer_text(terminal.backend())
        });
        assert!(rendered.contains("workspace client-work"));
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
    fn detail_content_includes_notes() {
        let item = detail_test_item();
        let rendered = detail_content_lines(&item, 60)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Fix token refresh race"));
        assert!(rendered.contains("Confirmed race in useTokenRefresh.ts"));
        assert!(rendered.contains("2026-06-20T12:00:00Z"));
    }

    #[test]
    fn detail_content_renders_markdown_description_and_notes() {
        let mut item = detail_test_item();
        item.task.description = "## Context\n- **One** item".to_string();
        item.notes[0].body = "Use `aven show` after edits".to_string();

        let rendered = detail_content_lines(&item, 60)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Context"));
        assert!(rendered.contains("- One item"));
        assert!(rendered.contains("aven show"));
    }

    #[test]
    fn detail_description_lines_keep_quote_rail() {
        let mut item = detail_test_item();
        item.task.description = "## Context\nsecond line".to_string();
        let lines = detail_content_lines(&item, 60);
        let description_lines: Vec<_> = lines
            .into_iter()
            .filter(|line| {
                let text = line.to_string();
                text.contains("Context") || text.contains("second")
            })
            .collect();
        assert!(!description_lines.is_empty());
        for line in description_lines {
            assert!(
                line.spans
                    .first()
                    .is_some_and(|span| span.content.as_ref() == "│ "),
                "missing quote rail: {line:?}"
            );
        }
    }

    #[test]
    fn detail_note_lines_keep_card_background() {
        let mut item = detail_test_item();
        item.notes[0].body = "Use `aven` here".to_string();
        let lines = detail_content_lines(&item, 60);
        let note_lines: Vec<_> = lines
            .into_iter()
            .filter(|line| line.to_string().contains("aven"))
            .collect();
        assert_eq!(note_lines.len(), 1);
        let line = &note_lines[0];
        assert!(
            line.spans
                .first()
                .is_some_and(|span| span.style.bg == Some(BG_PANEL)),
            "missing left card padding background"
        );
        assert!(
            line.spans
                .last()
                .is_some_and(|span| span.style.bg == Some(BG_PANEL)),
            "missing right card padding background"
        );
        assert!(
            line.spans
                .iter()
                .any(|span| span.content.as_ref() == "aven" && span.style.bg == Some(BG_PANEL)),
            "note body span missing card background"
        );
    }

    #[test]
    fn detail_metadata_includes_operational_fields() {
        let item = detail_test_item();
        let rendered = detail_metadata_lines(&item)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("PROJECT\n● app"));
        assert!(rendered.contains("STATUS\n● active"));
        assert!(rendered.contains("PRIORITY\n▲ urgent"));
        assert!(rendered.contains("LABELS\nbug, mobile"));
        assert!(rendered.contains("CONFLICTS\nyes"));
    }

    #[test]
    fn detail_scroll_start_is_capped_by_visible_rows() {
        assert_eq!(detail_scroll_start(0, 20, 5), 0);
        assert_eq!(detail_scroll_start(8, 20, 5), 8);
        assert_eq!(detail_scroll_start(30, 20, 5), 15);
        assert_eq!(detail_scroll_start(4, 3, 5), 0);
    }

    #[test]
    fn detail_footer_lists_scroll_and_task_navigation_keys() {
        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(footer_bar(FooterMode::Detail), frame.area()))
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(rendered.contains("j/k Pg"));
        assert!(rendered.contains("[/]"));
        assert!(rendered.contains("prev/next"));
    }

    fn detail_test_item() -> TaskListItem {
        TaskListItem {
            task: crate::types::Task {
                id: "7KQ9A1X".to_string(),
                workspace_id: "workspace-1".to_string(),
                title: "Fix token refresh race".to_string(),
                description: "Two token refresh requests fire together.".to_string(),
                project_key: "app".to_string(),
                project_prefix: "APP".to_string(),
                status: "active".to_string(),
                priority: "urgent".to_string(),
                created_at: "2026-06-19T12:00:00Z".to_string(),
                updated_at: "2026-06-20T12:00:00Z".to_string(),
                deleted: false,
            },
            display_ref: "APP-7KQ9A1X".to_string(),
            labels: vec!["bug".to_string(), "mobile".to_string()],
            notes: vec![crate::query::TaskNote {
                body: "Confirmed race in useTokenRefresh.ts".to_string(),
                created_at: "2026-06-20T12:00:00Z".to_string(),
            }],
            has_conflict: true,
            queue: Default::default(),
        }
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
            cursor: 5,
        });
        assert!(rendered.contains("Search"));
        assert!(rendered.contains("/query"));
    }

    #[test]
    fn overlay_render_includes_command_title_and_input() {
        let rendered = render_overlay_view(OverlayView::Command {
            input: "ref".to_string(),
            cursor: 3,
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
    fn detail_help_overlay_shows_detail_shortcuts() {
        let rendered = render_overlay_view(OverlayView::DetailHelp { scroll: 0 });
        assert!(rendered.contains("Task detail shortcuts"));
        assert!(rendered.contains("return to the task list"));
        assert!(rendered.contains("scroll one page"));
        assert!(rendered.contains("select previous or next task"));
        assert!(rendered.contains("add a note to this task"));
        assert!(!rendered.contains("view updated"));
    }

    #[test]
    fn detail_help_compacts_repeated_prefixes() {
        let rendered = detail_help_lines()
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("e t/d/p/l"));
        assert!(rendered.contains("m i/b/t/a"));
        assert!(rendered.contains("m 0/l/m/h/u"));
        assert!(!rendered.contains("m 0/m l/m m/m h/m u"));
    }

    #[test]
    fn detail_help_scroll_cap_uses_detail_rows() {
        assert!(detail_help_scroll_cap(10) > 0);
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
    fn add_task_overlay_renders_metadata_title_and_footer() {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
            title: "Add task  project=aven priority=high".to_string(),
            prompt: "Title".to_string(),
            input: "ship dialogs".to_string(),
            cursor: 12,
        }));
        assert!(rendered.contains("Add task"));
        assert!(rendered.contains("project: aven"));
        assert!(rendered.contains("prio: high"));
        assert!(rendered.contains("ship dialogs"));
        assert!(rendered.contains("Ctrl+P priority"));
    }

    #[test]
    fn hint_lines_style_keys() {
        let add_task_keys = styled_key_contents(add_task_hint_line());
        assert_eq!(add_task_keys, vec!["Enter", "Tab", "Ctrl+P", "Esc"]);

        let multiline_keys = styled_key_contents(multiline_hint_line());
        assert_eq!(multiline_keys, vec!["Ctrl+S", "Esc"]);

        let confirm_keys = styled_key_contents(confirm_hint_line());
        assert_eq!(confirm_keys, vec!["y", "n", "Esc"]);

        let dialog_keys =
            styled_key_contents(dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]));
        assert_eq!(dialog_keys, vec!["Enter", "Esc"]);
    }

    fn styled_key_contents(line: Line<'static>) -> Vec<String> {
        line.spans
            .iter()
            .filter(|span| span.style.fg == Some(FG))
            .map(|span| span.content.to_string())
            .collect()
    }

    #[test]
    fn add_task_empty_title_input_shows_placeholder() {
        let line = add_task_title_input_line("", 0, 20);
        assert_eq!(line.spans[0].content.as_ref(), "t");
        assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[0].style.bg, Some(FG));
        assert_eq!(line.spans[1].content.as_ref(), "itle");
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    }

    #[test]
    fn add_task_title_input_draws_cursor_as_cell() {
        let line = add_task_title_input_line("abc", 1, 20);
        assert_eq!(line.spans[0].content.as_ref(), "a");
        assert_eq!(line.spans[1].content.as_ref(), "b");
        assert_eq!(line.spans[1].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[1].style.bg, Some(FG));
        assert_eq!(line.spans[2].content.as_ref(), "c");
    }

    #[test]
    fn add_task_title_input_draws_end_cursor_as_blank_cell() {
        let line = add_task_title_input_line("abc", 3, 20);
        assert_eq!(line.spans[0].content.as_ref(), "abc");
        assert_eq!(line.spans[1].content.as_ref(), " ");
        assert_eq!(line.spans[1].style.bg, Some(FG));
    }

    #[test]
    fn add_task_title_input_scrolls_to_cursor_cell() {
        let line = add_task_title_input_line("abcdef", 5, 4);
        assert_eq!(line.spans[0].content.as_ref(), "cde");
        assert_eq!(line.spans[1].content.as_ref(), "f");
    }

    #[test]
    fn input_cursor_handles_byte_indexed_unicode_cursor() {
        let line = input_line("", "aéz", 3);
        assert_eq!(line.spans[0].content.as_ref(), "aé");
        assert_eq!(line.spans[1].content.as_ref(), "z");
        assert_eq!(line.spans[1].style.bg, Some(FG));
    }

    #[test]
    fn add_task_metadata_title_labels_values() {
        let rendered = add_task_metadata_title("aven", "none", 60).to_string();
        assert!(rendered.contains("project: aven"));
        assert!(rendered.contains("prio: none"));
        assert!(rendered.contains(" · "));
        assert!(!rendered.contains("Tab"));
        assert!(!rendered.contains("Ctrl+P"));
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
    fn add_note_empty_input_shows_placeholder() {
        let line = add_note_input_line("", Some(0));
        assert_eq!(line.spans[0].content.as_ref(), "▌");
        assert_eq!(line.spans[1].content.as_ref(), "note body");
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    }

    #[test]
    fn multiline_hint_styles_keys() {
        let line = multiline_hint_line();
        let keys = line
            .spans
            .iter()
            .filter(|span| span.style.fg == Some(FG))
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["Ctrl+S", "Esc"]);
    }

    #[test]
    fn add_note_overlay_uses_placeholder_and_key_styles() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            title: "Add note".to_string(),
            prompt: "note body:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }));
        assert!(rendered.contains("Add note"));
        assert!(rendered.contains("note body"));
        assert!(rendered.contains("Ctrl+S submit"));
    }

    #[test]
    fn overlay_render_includes_picker_filter_and_hints() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: "app".to_string(),
            filter_cursor: 3,
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
        assert!(rendered.contains("Ctrl+N/P"));
        assert!(rendered.contains("Space"));
        assert!(rendered.contains("toggle"));
    }

    #[test]
    fn priority_picker_shows_priority_icons() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Edit task: priority".to_string(),
            filter: String::new(),
            filter_cursor: 0,
            items: vec![PickerItem {
                label: "urgent".to_string(),
                value: "urgent".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains(priority_icon("urgent")));
        assert!(rendered.contains("urgent"));
        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("submit"));
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
            filter_cursor: 0,
            items,
            selected: 10,
            multi: false,
            visible_indices: (0..12).collect(),
        }));
        assert!(rendered.contains("▸ Item 10"));
        assert!(!rendered.contains("Item 0"));
    }

    #[test]
    fn project_picker_uses_structured_columns() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Go: project".to_string(),
            filter: "claude".to_string(),
            filter_cursor: 6,
            items: vec![PickerItem {
                label: "CC claude-code".to_string(),
                value: "claude-code".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains("PREFIX"));
        assert!(rendered.contains("PROJECT"));
        assert!(rendered.contains("CC"));
        assert!(rendered.contains("claude-code"));
        assert!(rendered.contains("Enter open"));
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
