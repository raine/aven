use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::render::quote;
use crate::tui::store::{SidebarTarget, TuiStore};
use crate::tui::theme::{ACCENT, BG, BG_PANEL, BLUE, BORDER, FG, FG_DIM, FG_MUTED, GREEN, PINK};

pub(super) fn render_header(frame: &mut Frame, store: &TuiStore, area: Rect) {
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
