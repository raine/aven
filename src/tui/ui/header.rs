use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::render::quote;
use crate::tui::store::{TaskScope, TaskView, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_PANEL, BLUE, BORDER, FG, FG_DIM, FG_MUTED, GREEN, INVERSE_FG, ORANGE,
    PINK, RED,
};

pub(super) fn render_header(frame: &mut Frame, store: &TuiStore, area: Rect) {
    frame.render_widget(
        Block::new()
            .borders(Borders::BOTTOM)
            .border_style(Style::new().fg(BORDER))
            .style(Style::new().bg(BG)),
        area,
    );
    let content_area = Rect {
        height: 1,
        width: area.width.saturating_sub(1),
        ..area
    };
    if area.width >= 84 {
        let status_width = header_status_width(store);
        let [left, right] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(status_width)])
                .areas(content_area);
        frame.render_widget(header_line(store, left.width), left);
        frame.render_widget(header_status(store), right);
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
        separator(),
        Span::styled("scope ", Style::new().fg(FG_DIM)),
    ];
    spans.extend(scope_badge(store));
    spans.extend([separator(), Span::styled("view ", Style::new().fg(FG_DIM))]);
    spans.extend(view_badge(store));
    spans.push(separator());
    spans.extend(header_metrics(store, compact));
    spans.extend(active_filter_spans(store));
    if !compact || width >= 84 {
        spans.extend(active_order_spans(store));
    }
    Paragraph::new(Line::from(spans)).style(Style::new().fg(FG).bg(BG))
}

fn header_metrics(store: &TuiStore, compact: bool) -> Vec<Span<'static>> {
    let view = store.view_state.view;
    let metrics = [
        ("queue", store.counts.open, ACCENT, view == TaskView::Queue),
        ("open", store.counts.open, GREEN, view == TaskView::Open),
        ("todo", store.counts.todo, BLUE, view == TaskView::Todo),
        (
            "inbox",
            store.counts.inbox,
            FG_MUTED,
            view == TaskView::Inbox,
        ),
        (
            "conflicts",
            store.counts.conflicts,
            PINK,
            view == TaskView::Conflicts,
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

fn scope_badge(store: &TuiStore) -> Vec<Span<'static>> {
    let badge_style = Style::new()
        .fg(FG)
        .bg(BG_PANEL)
        .add_modifier(Modifier::BOLD);
    match &store.view_state.scope {
        TaskScope::Workspace => vec![Span::styled(" workspace ", badge_style)],
        TaskScope::Project(project) => vec![
            Span::styled(" project ", badge_style),
            Span::styled(
                project.clone(),
                badge_style.fg(theme::project_color(project)),
            ),
            Span::styled(" ", badge_style),
        ],
    }
}

fn view_badge(store: &TuiStore) -> Vec<Span<'static>> {
    let badge_style = Style::new()
        .fg(FG)
        .bg(BG_PANEL)
        .add_modifier(Modifier::BOLD);
    vec![Span::styled(
        format!(" {} ", active_view_label(store)),
        badge_style,
    )]
}

fn active_view_label(store: &TuiStore) -> &'static str {
    match store.view_state.view {
        TaskView::Queue => "queue",
        TaskView::Open => "open",
        TaskView::Inbox => "inbox",
        TaskView::Active => "active",
        TaskView::Backlog => "backlog",
        TaskView::Todo => "todo",
        TaskView::Done => "done",
        TaskView::Conflicts => "conflicts",
    }
}

fn metric(label: &str, count: i64, color: Color, active: bool) -> Vec<Span<'static>> {
    let fill = if active { color } else { BG_PANEL };
    let fg = if active { INVERSE_FG } else { color };
    let style = Style::new().fg(fg).bg(fill).add_modifier(Modifier::BOLD);
    let edge_style = Style::new().fg(fill).bg(BG);
    vec![
        Span::styled("".to_string(), edge_style),
        Span::styled(format!("{label} {count}"), style),
        Span::styled("".to_string(), edge_style),
    ]
}

fn active_order_spans(store: &TuiStore) -> Vec<Span<'static>> {
    let mut spans = vec![
        separator(),
        Span::styled("order ", Style::new().fg(FG_DIM)),
        Span::styled(
            store.sort_label(),
            Style::new().fg(FG_MUTED).add_modifier(Modifier::BOLD),
        ),
    ];
    if store.view_state.view != TaskView::Queue {
        spans.push(Span::styled(
            format!(" {}", store.sort_direction_label()),
            Style::new().fg(FG_DIM),
        ));
    }
    spans
}

fn active_filter_spans(store: &TuiStore) -> Vec<Span<'static>> {
    let modifiers = &store.view_state.filter_modifiers;
    let mut parts = Vec::new();
    if let Some(label) = &modifiers.label {
        parts.push(vec![filter_part(format!("label={label}"))]);
    }
    if let Some(priority) = &modifiers.priority {
        parts.push(vec![filter_part(format!("priority={priority}"))]);
    }
    if modifiers.include_deleted {
        parts.push(vec![filter_part("include_deleted")]);
    }
    if let Some(search) = &modifiers.search {
        parts.push(vec![filter_part(format!("search={}", quote(search)))]);
    }
    if parts.is_empty() {
        Vec::new()
    } else {
        let mut spans = vec![
            separator(),
            Span::styled("filter ", Style::new().fg(FG_DIM)),
        ];
        spans.extend(join_filter_parts(parts));
        spans
    }
}

fn filter_part(content: impl Into<std::borrow::Cow<'static, str>>) -> Span<'static> {
    Span::styled(
        content,
        Style::new().fg(FG_MUTED).add_modifier(Modifier::BOLD),
    )
}

fn join_filter_parts(parts: Vec<Vec<Span<'static>>>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for part in parts {
        if !spans.is_empty() {
            spans.push(filter_part(" "));
        }
        spans.extend(part);
    }
    spans
}

fn header_status_width(store: &TuiStore) -> u16 {
    let (_, label) = sync_status_label(store);
    label.len() as u16 + 2
}

fn header_status(store: &TuiStore) -> Paragraph<'static> {
    let (dot_color, label) = sync_status_label(store);
    let spans = vec![
        Span::styled("●", Style::new().fg(dot_color)),
        Span::styled(format!(" {label}"), Style::new().fg(FG_DIM)),
    ];
    Paragraph::new(Line::from(spans))
        .alignment(Alignment::Right)
        .style(Style::new().fg(FG_DIM).bg(BG))
}

fn sync_status_label(store: &TuiStore) -> (Color, String) {
    let status = &store.sync_status;
    if status.has_sync_error() || status.conflicts > 0 {
        return (RED, "sync!".to_string());
    }
    if !status.enabled {
        return (FG_DIM, "local".to_string());
    }
    if status.pending_changes > 0 {
        return (ORANGE, format!("sync {}", status.pending_changes));
    }
    (GREEN, "sync".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::store::{TaskFilterModifiers, TaskOrder, TaskViewState};

    async fn test_store() -> TuiStore {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let default = crate::workspaces::ensure_default_workspace(&mut conn)
            .await
            .unwrap();
        crate::workspaces::set_active_workspace(default);
        drop(conn);
        let mut store = TuiStore::new(pool).await.unwrap();
        store.counts = crate::query::SidebarCounts {
            open: 3,
            inbox: 1,
            active: 0,
            backlog: 0,
            todo: 2,
            conflicts: 1,
            done: 4,
        };
        store
    }

    fn spans_text(spans: Vec<Span<'static>>) -> String {
        Line::from(spans).to_string()
    }

    #[tokio::test]
    async fn header_parts_render_scope_view_metrics_filters_and_order() {
        let mut store = test_store().await;
        store.view_state = TaskViewState {
            scope: TaskScope::Project("mobile-app".to_string()),
            view: TaskView::Open,
            filter_modifiers: TaskFilterModifiers {
                label: Some("backend".to_string()),
                priority: Some("urgent".to_string()),
                include_deleted: true,
                hide_done: false,
                conflicts_only: true,
                ready_only: false,
                blocked_only: false,
                search: Some("needle".to_string()),
            },
            order: TaskOrder::Priority,
            direction: crate::query::SortDirection::Desc,
        };

        assert_eq!(store.active_workspace.key, "default");
        assert_eq!(spans_text(scope_badge(&store)), " project mobile-app ");
        assert_eq!(spans_text(view_badge(&store)), " open ");
        assert!(spans_text(header_metrics(&store, false)).contains("open 3"));
        assert!(spans_text(header_metrics(&store, false)).contains("todo 2"));
        assert!(spans_text(header_metrics(&store, false)).contains("conflicts 1"));
        assert_eq!(
            spans_text(active_filter_spans(&store)),
            " │ filter label=backend priority=urgent include_deleted search=\"needle\""
        );
        assert_eq!(
            spans_text(active_order_spans(&store)),
            " │ order priority desc"
        );
        assert!(!spans_text(active_filter_spans(&store)).contains("project="));
        assert_ne!(spans_text(view_badge(&store)), " project mobile-app ");
    }

    #[tokio::test]
    async fn queue_header_shows_ranked_order_without_direction() {
        let mut store = test_store().await;
        store.view_state.view = TaskView::Queue;
        store.view_state.direction = crate::query::SortDirection::Desc;

        assert_eq!(spans_text(view_badge(&store)), " queue ");
        assert_eq!(spans_text(active_order_spans(&store)), " │ order ranked");
    }
}
