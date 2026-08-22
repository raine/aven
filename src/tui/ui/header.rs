use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::tui::store::{TaskLayout, TaskQuery, TaskScope, TuiStore};
use crate::tui::theme::{
    self, ACCENT, BG, BG_PANEL, BLUE, BORDER, FG, FG_DIM, FG_MUTED, GREEN, INVERSE_FG, ORANGE,
    PINK, RED,
};

const HEADER_STATUS_GAP: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeaderTarget {
    Home,
    Changelog,
    Workspace { column: u16 },
    Scope { column: u16 },
    Query { column: u16 },
    Layout,
    MetricView(TaskQuery),
    Order { column: u16 },
    Update,
    SyncStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeaderHitbox {
    start: u16,
    end: u16,
    target: HeaderTarget,
}

pub(super) fn render_header(
    frame: &mut Frame,
    store: &TuiStore,
    update: Option<&crate::tui::app_update::UpdateBadgeView>,
    area: Rect,
) {
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
        frame.render_widget(header_line(store, update, left.width), left);
        frame.render_widget(header_status(store), right);
    } else {
        frame.render_widget(header_line(store, update, content_area.width), content_area);
    }
}

pub(crate) fn header_target_at(
    store: &TuiStore,
    update: Option<&crate::tui::app_update::UpdateBadgeView>,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<HeaderTarget> {
    if y != area.y || x < area.x || x >= area.x.saturating_add(area.width) {
        return None;
    }
    let content_width = area.width.saturating_sub(1);
    let line_width = if area.width >= 84 {
        let status_width = header_status_width(store);
        content_width.saturating_sub(status_width)
    } else {
        content_width
    };
    let content_end = area.x.saturating_add(content_width);
    let status_start = area.x.saturating_add(line_width);
    if area.width >= 84 && x >= status_start && x < content_end {
        return Some(HeaderTarget::SyncStatus);
    }
    if x >= status_start {
        return None;
    }
    let local_x = x.saturating_sub(area.x);
    header_hitboxes(store, update, line_width)
        .into_iter()
        .find(|hitbox| local_x >= hitbox.start && local_x < hitbox.end)
        .map(|hitbox| {
            hitbox
                .target
                .with_origin(area.x.saturating_add(hitbox.start))
        })
}

impl HeaderTarget {
    fn with_origin(self, column: u16) -> Self {
        match self {
            Self::Workspace { .. } => Self::Workspace { column },
            Self::Scope { .. } => Self::Scope { column },
            Self::Query { .. } => Self::Query { column },
            Self::Order { .. } => Self::Order { column },
            target => target,
        }
    }
}

fn header_line(
    store: &TuiStore,
    update: Option<&crate::tui::app_update::UpdateBadgeView>,
    width: u16,
) -> Paragraph<'static> {
    Paragraph::new(Line::from(header_spans(store, update, width))).style(Style::new().fg(FG).bg(BG))
}

fn header_spans(
    store: &TuiStore,
    update: Option<&crate::tui::app_update::UpdateBadgeView>,
    width: u16,
) -> Vec<Span<'static>> {
    header_layout(store, update, width).spans
}

fn header_hitboxes(
    store: &TuiStore,
    update: Option<&crate::tui::app_update::UpdateBadgeView>,
    width: u16,
) -> Vec<HeaderHitbox> {
    header_layout(store, update, width).hitboxes
}

struct HeaderLayout {
    spans: Vec<Span<'static>>,
    hitboxes: Vec<HeaderHitbox>,
    width: u16,
}

impl HeaderLayout {
    fn new() -> Self {
        Self {
            spans: Vec::new(),
            hitboxes: Vec::new(),
            width: 0,
        }
    }

    fn push(&mut self, spans: Vec<Span<'static>>, target: Option<HeaderTarget>) {
        let start = self.width;
        self.width = self.width.saturating_add(spans_width(spans.clone()));
        self.spans.extend(spans);
        if let Some(target) = target
            && start < self.width
        {
            self.hitboxes.push(HeaderHitbox {
                start,
                end: self.width,
                target,
            });
        }
    }

    fn push_text(&mut self, span: Span<'static>, target: Option<HeaderTarget>) {
        self.push(vec![span], target);
    }

    fn segment_fits(&self, segment: &[Span<'static>], available_width: u16) -> bool {
        header_segment_fits(self.width, spans_width(segment.to_vec()), available_width)
    }

    fn finish(mut self, available_width: u16) -> Self {
        self.hitboxes
            .retain(|hitbox| hitbox.start < available_width);
        for hitbox in &mut self.hitboxes {
            hitbox.end = hitbox.end.min(available_width);
        }
        self
    }
}

fn header_layout(
    store: &TuiStore,
    update: Option<&crate::tui::app_update::UpdateBadgeView>,
    width: u16,
) -> HeaderLayout {
    let compact = width < 120;
    let mut layout = HeaderLayout::new();
    layout.push_text(
        Span::styled(" aven", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Some(HeaderTarget::Home),
    );
    layout.push_text(
        Span::styled(
            format!(" v{}", crate::update::CURRENT_VERSION),
            Style::new().fg(FG_DIM),
        ),
        Some(HeaderTarget::Changelog),
    );
    if let Some(update) = update {
        layout.push_text(separator(), None);
        layout.push(update_badge(update), Some(HeaderTarget::Update));
    }
    layout.push_text(separator(), None);
    layout.push(
        vec![
            Span::styled(
                if compact { "ws " } else { "workspace " },
                Style::new().fg(FG_DIM),
            ),
            Span::styled(
                store.active_workspace.key.clone(),
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ],
        Some(HeaderTarget::Workspace { column: 0 }),
    );
    layout.push_text(separator(), None);
    let mut scope = vec![Span::styled("scope ", Style::new().fg(FG_DIM))];
    scope.extend(scope_badge(store));
    layout.push(scope, Some(HeaderTarget::Scope { column: 0 }));
    layout.push_text(separator(), None);
    let mut view = vec![Span::styled("query ", Style::new().fg(FG_DIM))];
    view.extend(view_badge(store));
    layout.push(view, Some(HeaderTarget::Query { column: 0 }));
    layout.push_text(separator(), None);
    layout.push(
        vec![Span::styled(
            match store.view_state.layout {
                TaskLayout::List => " ▦ list ",
                TaskLayout::Columns => " ▦ columns ",
            },
            Style::new()
                .fg(FG)
                .bg(BG_PANEL)
                .add_modifier(Modifier::BOLD),
        )],
        Some(HeaderTarget::Layout),
    );
    layout.push(active_filter_spans(store), None);

    let order = active_order_spans(store);
    if (!compact || width >= 84) && layout.segment_fits(&order, width) {
        layout.push(order, Some(HeaderTarget::Order { column: 0 }));
    }

    for (index, (label, count, color, active, target)) in header_metric_entries(store, compact)
        .into_iter()
        .enumerate()
    {
        let prefix = if index == 0 {
            vec![separator()]
        } else {
            vec![Span::raw(" ")]
        };
        let badge = metric(label, count, color, active);
        let mut segment = prefix.clone();
        segment.extend(badge.clone());
        if !layout.segment_fits(&segment, width) {
            break;
        }
        layout.push(prefix, None);
        layout.push(badge, Some(HeaderTarget::MetricView(target)));
    }

    layout.finish(width)
}

fn spans_width(spans: Vec<Span<'static>>) -> u16 {
    spans_text(spans).width() as u16
}

fn spans_text(spans: Vec<Span<'static>>) -> String {
    Line::from(spans).to_string()
}

fn header_segment_fits(base_width: u16, segment_width: u16, available_width: u16) -> bool {
    base_width
        .saturating_add(segment_width)
        .saturating_add(HEADER_STATUS_GAP)
        <= available_width
}

fn separator_text() -> &'static str {
    " │ "
}

#[cfg(test)]
fn header_metrics(store: &TuiStore, compact: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (label, count, color, active, _) in header_metric_entries(store, compact) {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.extend(metric(label, count, color, active));
    }
    spans
}

fn header_metric_entries(
    store: &TuiStore,
    compact: bool,
) -> Vec<(&'static str, i64, Color, bool, TaskQuery)> {
    let view = store.view_state.query;
    let metrics = [
        (
            "queue",
            store.counts.open,
            ACCENT,
            view == TaskQuery::Queue,
            TaskQuery::Queue,
        ),
        (
            "open",
            store.counts.open,
            GREEN,
            view == TaskQuery::Open,
            TaskQuery::Open,
        ),
        (
            "todo",
            store.counts.todo,
            BLUE,
            view == TaskQuery::Todo,
            TaskQuery::Todo,
        ),
        (
            "inbox",
            store.counts.inbox,
            FG_MUTED,
            view == TaskQuery::Inbox,
            TaskQuery::Inbox,
        ),
        (
            "conflicts",
            store.counts.conflicts,
            PINK,
            view == TaskQuery::Conflicts,
            TaskQuery::Conflicts,
        ),
    ];
    metrics
        .into_iter()
        .filter(|(_, count, _, active, _)| !compact || *count > 0 || *active)
        .collect()
}

fn separator() -> Span<'static> {
    Span::styled(separator_text(), Style::new().fg(BORDER))
}

fn update_badge(update: &crate::tui::app_update::UpdateBadgeView) -> Vec<Span<'static>> {
    let fill = if update.restart { ORANGE } else { ACCENT };
    let edge = Style::new().fg(fill).bg(BG);
    let label = Style::new()
        .fg(INVERSE_FG)
        .bg(fill)
        .add_modifier(Modifier::BOLD);
    vec![
        Span::styled("".to_string(), edge),
        Span::styled(update.label.clone(), label),
        Span::styled("".to_string(), edge),
    ]
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
    match store.view_state.query {
        TaskQuery::Queue => "queue",
        TaskQuery::All => "all",
        TaskQuery::Open => "open",
        TaskQuery::Inbox => "inbox",
        TaskQuery::Active => "active",
        TaskQuery::Backlog => "backlog",
        TaskQuery::Todo => "todo",
        TaskQuery::Done => "done",
        TaskQuery::Upcoming => "upcoming",
        TaskQuery::Conflicts => "conflicts",
        TaskQuery::Search => "search",
        TaskQuery::RecentActions => "recent",
        TaskQuery::Epics => "epics",
        TaskQuery::Recurring => "recurring",
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
    if store.view_state.query == TaskQuery::Recurring {
        return Vec::new();
    }
    let mut spans = vec![
        separator(),
        Span::styled("order ", Style::new().fg(FG_DIM)),
        Span::styled(
            store.sort_label(),
            Style::new().fg(FG_MUTED).add_modifier(Modifier::BOLD),
        ),
    ];
    if !matches!(
        store.view_state.query,
        TaskQuery::Queue | TaskQuery::Search | TaskQuery::RecentActions
    ) {
        spans.push(Span::styled(
            format!(" {}", store.sort_direction_label()),
            Style::new().fg(FG_DIM),
        ));
    }
    spans
}

fn active_filter_spans(store: &TuiStore) -> Vec<Span<'static>> {
    if store.view_state.query == TaskQuery::Recurring {
        let mut parts = vec![vec![filter_part(format!(
            "lifecycle={}",
            store.view_state.recurring.lifecycle.as_str()
        ))]];
        if let Some(search) = store.view_state.recurring.search.as_deref() {
            parts.push(vec![filter_part(format!("search={search}"))]);
        }
        let mut spans = vec![
            separator(),
            Span::styled("filter ", Style::new().fg(FG_DIM)),
        ];
        spans.extend(join_filter_parts(parts));
        return spans;
    }
    let modifiers = &store.view_state.filter_modifiers;
    let mut parts = Vec::new();
    if let Some(label) = &modifiers.label {
        parts.push(vec![filter_part(format!("label={label}"))]);
    }
    if let Some(priority) = &modifiers.priority {
        parts.push(vec![filter_part(format!("priority={priority}"))]);
    }
    match modifiers.closed {
        crate::tui::store::ClosedTaskVisibility::Default => {}
        crate::tui::store::ClosedTaskVisibility::Included => {
            parts.push(vec![filter_part("include_closed")]);
        }
        crate::tui::store::ClosedTaskVisibility::Only => {
            parts.push(vec![filter_part("closed_only")]);
        }
    }
    if modifiers.deleted_only {
        parts.push(vec![filter_part("deleted_only")]);
    } else if modifiers.include_deleted {
        parts.push(vec![filter_part("include_deleted")]);
    }
    if let Some(match_count) = store.view_state.projection_origin.match_count() {
        parts.push(vec![filter_part(format!("matches={match_count}"))]);
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
    label.width() as u16 + 3
}

fn header_status(store: &TuiStore) -> Paragraph<'static> {
    let (dot_color, label) = sync_status_label(store);
    let spans = vec![
        Span::raw(" "),
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    async fn test_store() -> (TuiStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::test_support::open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::ensure_default_workspace(&mut conn)
            .await
            .unwrap();
        drop(conn);
        let database = crate::db::Database::open(&db_path).await.unwrap();
        let mut store = TuiStore::new(database, crate::workspaces::Workspace::default())
            .await
            .unwrap();
        store.counts = crate::query::SidebarCounts {
            open: 3,
            inbox: 1,
            active: 0,
            backlog: 0,
            todo: 2,
            conflicts: 1,
            done: 4,
            upcoming: 0,
            epics: 0,
            recurring: 0,
        };
        (store, dir)
    }

    fn spans_text(spans: Vec<Span<'static>>) -> String {
        Line::from(spans).to_string()
    }

    #[tokio::test]
    async fn search_prompt_omits_match_count_filter() {
        let (mut store, _dir) = test_store().await;
        store.show_view(TaskQuery::Search).await.unwrap();

        assert!(!spans_text(active_filter_spans(&store)).contains("matches="));
    }

    #[tokio::test]
    async fn header_parts_render_scope_view_metrics_filters_and_order() {
        let (mut store, _dir) = test_store().await;
        store.view_state = TaskViewState {
            scope: TaskScope::Project("mobile-app".to_string()),
            query: TaskQuery::Open,
            layout: TaskLayout::List,
            projection_origin: crate::tui::store::TaskProjectionOrigin::ExactTasks(vec![
                crate::test_support::task_id("task-1"),
                crate::test_support::task_id("task-2"),
            ]),
            filter_modifiers: TaskFilterModifiers {
                label: Some("backend".to_string()),
                priority: Some("urgent".to_string()),
                closed: crate::tui::store::ClosedTaskVisibility::Included,
                include_deleted: true,
                deleted_only: false,
            },
            order: TaskOrder::Priority,
            direction: crate::query::SortDirection::Desc,
            recurring: Default::default(),
            expanded_epic_ids: std::collections::BTreeSet::new(),
            collapsed_epic_ids: std::collections::BTreeSet::new(),
        };

        assert_eq!(store.active_workspace.key, "default");
        assert_eq!(spans_text(scope_badge(&store)), " project mobile-app ");
        assert_eq!(spans_text(view_badge(&store)), " open ");
        assert!(spans_text(header_metrics(&store, false)).contains("open 3"));
        assert!(spans_text(header_metrics(&store, false)).contains("todo 2"));
        assert!(spans_text(header_metrics(&store, false)).contains("conflicts 1"));
        assert_eq!(
            spans_text(active_filter_spans(&store)),
            " │ filter label=backend priority=urgent include_closed include_deleted matches=2"
        );

        store.view_state.filter_modifiers.deleted_only = true;
        assert_eq!(
            spans_text(active_filter_spans(&store)),
            " │ filter label=backend priority=urgent include_closed deleted_only matches=2"
        );
        store.view_state.filter_modifiers.deleted_only = false;
        store.view_state.filter_modifiers.closed = crate::tui::store::ClosedTaskVisibility::Only;
        assert!(spans_text(active_filter_spans(&store)).contains("closed_only"));
        assert_eq!(
            spans_text(active_order_spans(&store)),
            " │ order priority desc"
        );
        assert!(!spans_text(active_filter_spans(&store)).contains("project="));
        assert_ne!(spans_text(view_badge(&store)), " project mobile-app ");
    }

    #[tokio::test]
    async fn compact_header_omits_order_when_it_does_not_fit() {
        let (mut store, _dir) = test_store().await;
        store.counts = crate::query::SidebarCounts::default();

        let rendered = spans_text(header_spans(&store, None, 75));

        assert!(!rendered.contains("order"));
    }

    #[tokio::test]
    async fn constrained_header_keeps_complete_filter_ahead_of_metrics() {
        let (mut store, _dir) = test_store().await;
        store.view_state.filter_modifiers.label = Some("capture".to_string());

        let rendered = spans_text(header_spans(&store, None, 78));

        assert!(rendered.contains("filter label=capture"), "{rendered:?}");
        assert!(!rendered.contains("queue 3"), "{rendered:?}");
        assert!(rendered.ends_with("label=capture"), "{rendered:?}");
    }

    #[tokio::test]
    async fn constrained_header_adds_only_complete_priority_metrics() {
        let (mut store, _dir) = test_store().await;
        store.view_state.filter_modifiers.label = Some("capture".to_string());

        let rendered = spans_text(header_spans(&store, None, 110));

        assert!(rendered.contains("filter label=capture"), "{rendered:?}");
        assert!(rendered.contains("queue 3"), "{rendered:?}");
        assert!(!rendered.contains("open 3"), "{rendered:?}");
        assert!(rendered.ends_with(''), "{rendered:?}");
    }

    #[test]
    fn header_segment_fit_requires_gap() {
        assert!(!header_segment_fits(10, 5, 15));
        assert!(!header_segment_fits(10, 5, 16));
        assert!(header_segment_fits(10, 5, 17));
    }

    #[tokio::test]
    async fn sync_indicator_keeps_space_after_complete_left_segment() {
        let (mut store, _dir) = test_store().await;
        store.view_state.query = TaskQuery::Todo;
        store.sync_status.enabled = true;
        let width = 150;
        let backend = TestBackend::new(width, 2);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_header(frame, &store, None, frame.area()))
            .unwrap();

        let first_row = terminal.backend().buffer().content[..width as usize]
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let (prefix, _) = first_row.split_once("● sync").unwrap();
        assert!(prefix.ends_with(' '), "{first_row:?}");
        assert!(prefix.trim_end().ends_with(''), "{first_row:?}");
    }

    #[tokio::test]
    async fn queue_header_shows_ranked_order_without_direction() {
        let (mut store, _dir) = test_store().await;
        store.view_state.query = TaskQuery::Queue;
        store.view_state.direction = crate::query::SortDirection::Desc;

        assert_eq!(spans_text(view_badge(&store)), " queue ");
        assert_eq!(spans_text(active_order_spans(&store)), " │ order ranked");
    }

    #[tokio::test]
    async fn update_badge_is_clear_clickable_and_visible_on_narrow_headers() {
        let (store, _dir) = test_store().await;
        let update = crate::tui::app_update::UpdateBadgeView {
            label: "update available v9.0.0".to_string(),
            restart: false,
        };
        let wide = Rect::new(0, 0, 150, 2);
        assert!((0..wide.width).any(|column| {
            header_target_at(&store, Some(&update), wide, column, 0) == Some(HeaderTarget::Update)
        }));
        let narrow = Rect::new(0, 0, 70, 2);
        assert!((0..narrow.width).any(|column| {
            header_target_at(&store, Some(&update), narrow, column, 0) == Some(HeaderTarget::Update)
        }));
        assert!(
            spans_text(header_spans(&store, Some(&update), narrow.width))
                .contains("update available v9.0.0")
        );
    }

    #[tokio::test]
    async fn header_click_targets_follow_rendered_segments() {
        let (mut store, _dir) = test_store().await;
        store.view_state.scope = TaskScope::Project("mobile-app".to_string());
        store.view_state.filter_modifiers.label = Some("capture".to_string());
        let area = Rect::new(7, 4, 157, 2);
        let content_width = area.width.saturating_sub(1);
        let line_width = content_width.saturating_sub(header_status_width(&store));
        let rendered = spans_text(header_spans(&store, None, line_width));
        let column_for = |text: &str| {
            let byte = rendered.find(text).unwrap();
            area.x
                .saturating_add(rendered[..byte].width().try_into().unwrap())
        };

        assert_eq!(
            header_target_at(&store, None, area, column_for("aven") + 1, area.y),
            Some(HeaderTarget::Home)
        );
        assert_eq!(
            header_target_at(
                &store,
                None,
                area,
                column_for(&format!("v{}", crate::update::CURRENT_VERSION)),
                area.y,
            ),
            Some(HeaderTarget::Changelog)
        );
        assert_eq!(
            header_target_at(&store, None, area, column_for("workspace default"), area.y),
            Some(HeaderTarget::Workspace {
                column: column_for("workspace default")
            })
        );
        assert_eq!(
            header_target_at(&store, None, area, column_for("scope "), area.y),
            Some(HeaderTarget::Scope {
                column: column_for("scope ")
            })
        );
        assert_eq!(
            header_target_at(&store, None, area, column_for("query "), area.y),
            Some(HeaderTarget::Query {
                column: column_for("query ")
            })
        );
        assert_eq!(
            header_target_at(&store, None, area, column_for("▦ list"), area.y),
            Some(HeaderTarget::Layout)
        );
        assert_eq!(
            header_target_at(&store, None, area, column_for("label=capture"), area.y),
            None
        );
        assert_eq!(
            header_target_at(&store, None, area, column_for("order "), area.y),
            Some(HeaderTarget::Order {
                column: column_for(" │ order ")
            })
        );
        assert_eq!(
            header_target_at(&store, None, area, column_for("queue 3"), area.y),
            Some(HeaderTarget::MetricView(TaskQuery::Queue))
        );
        assert_eq!(
            header_target_at(
                &store,
                None,
                area,
                area.x.saturating_add(line_width),
                area.y
            ),
            Some(HeaderTarget::SyncStatus)
        );
        assert_eq!(
            header_target_at(&store, None, area, column_for("queue 3"), area.y + 1),
            None
        );
    }
}
