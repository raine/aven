use aven_core::choices::{TaskPriority, TaskSource, TaskStatus};
use aven_core::query::{RecentActionItem, RecurrenceSeriesListItem, SortDirection, TaskListItem};
use aven_core::queue::{QueueDateKind, QueueReason, TriageReason};
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceSeriesState, next_slot_after};
use aven_host::recurrence_input::natural_rule_label;
use aven_host::schedule_input::format_schedule_input;
use aven_host::store::{
    ClosedTaskVisibility, TaskLayout, TaskListRenderMode, TaskListRow, TaskOrder,
    TaskProjectionOrigin, TaskQuery, TaskScope, TuiStore,
};
use chrono::{DateTime, Datelike as _, Local, NaiveDate, NaiveTime};
use std::collections::VecDeque;

use super::model::*;

/// A save this soon after creation is part of creating the task.
const BYLINE_UPDATE_GAP: chrono::TimeDelta = chrono::TimeDelta::minutes(5);
/// The name and hint for each store group.
const GROUP_NAMES: [(&str, &str, &str); 10] = [
    (
        "needs action",
        "Needs action",
        "Conflicts, urgent, stale, due today or overdue",
    ),
    (
        "available",
        "Available",
        "Deferred work that has just arrived",
    ),
    ("focus", "Focus", "Active work and high priority todos"),
    ("soon", "Medium priority", "Todos at medium priority"),
    ("triage", "Triage", "Inbox tasks"),
    ("blocked", "Blocked", "Waiting on open dependencies"),
    ("later", "Later", "Backlog and low priority todos"),
    (
        "epics",
        "Epics",
        "Containers, separate from actionable work",
    ),
    ("done", "Done", ""),
    ("canceled", "Canceled", ""),
];

pub fn due_text(due: &str, today: NaiveDate) -> String {
    let Ok(date) = NaiveDate::parse_from_str(due, "%Y-%m-%d") else {
        return due.to_string();
    };
    match (date - today).num_days() {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        _ => date.format("%b %-d").to_string(),
    }
}

/// The reason a queue row shows, with whether it reads as a problem. Urgency and
/// dates are left to the priority icon and the date column.
fn reason_text(item: &TaskListItem, today: NaiveDate) -> Option<(String, bool)> {
    match item.queue.reason? {
        QueueReason::Conflict => {
            let label = item
                .conflicts
                .first()
                .map_or("Field", |conflict| &conflict.field);
            Some((format!("{label} conflict"), true))
        }
        QueueReason::Stale { days } => Some((format!("No activity for {days}d"), false)),
        QueueReason::AvailableAfterDeferral => {
            let at = item
                .task
                .available_at
                .as_deref()
                .and_then(parse_local_timestamp);
            Some((availability_reason(at, today), false))
        }
        QueueReason::Urgent
        | QueueReason::DueToday
        | QueueReason::Overdue { .. }
        | QueueReason::Blocked { .. } => None,
    }
}

/// A deferral to a date lands on midnight, which is no time to show.
fn availability_reason(at: Option<DateTime<Local>>, today: NaiveDate) -> String {
    let Some(at) = at else {
        return "Available".to_string();
    };
    if at.date_naive() != today {
        return format!("Available since {}", at.format("%b %-d"));
    }
    if at.time() == NaiveTime::MIN {
        return "Available today".to_string();
    }
    format!("Available since {}", at.format("%H:%M"))
}

/// The month and day, with the year when it is not this year's.
pub fn short_date(day: NaiveDate, today: NaiveDate) -> String {
    if day.year() == today.year() {
        day.format("%b %-d").to_string()
    } else {
        day.format("%b %-d, %Y").to_string()
    }
}

fn row_date(stamp: &str, today: NaiveDate) -> Option<RowDate> {
    parse_local_timestamp(stamp).map(|at| RowDate {
        day: short_date(at.date_naive(), today),
        full: at.format("%b %-d, %Y, %H:%M").to_string(),
    })
}

/// An RFC3339 instant in the local time zone, with an offset for elapsed-time math.
pub(super) fn parse_local_timestamp(stamp: &str) -> Option<DateTime<Local>> {
    let parsed = DateTime::parse_from_rfc3339(stamp).ok()?;
    Some(parsed.with_timezone(&Local))
}

/// Open tasks warn from the day before they are due.
fn date_tone(item: &TaskListItem, today: NaiveDate) -> DateTone {
    let due = item.task.due_on.as_deref();
    let due = due.and_then(|due| NaiveDate::parse_from_str(due, "%Y-%m-%d").ok());
    let closed = item.task.status.is_terminal();
    match due.filter(|_| !closed).map(|due| (due - today).num_days()) {
        Some(days) if days < 0 => DateTone::Late,
        Some(days) if days <= 1 => DateTone::Soon,
        _ => DateTone::Plain,
    }
}

pub fn available_text(stamp: &str, today: NaiveDate) -> String {
    parse_local_timestamp(stamp).map_or_else(String::new, |time| {
        let day = time.format("%Y-%m-%d").to_string();
        format!("{} {}", due_text(&day, today), time.format("%H:%M"))
    })
}

pub(super) fn is_deferred(item: &TaskListItem, now: DateTime<Local>) -> bool {
    let at = item
        .task
        .available_at
        .as_deref()
        .and_then(parse_local_timestamp);
    at.is_some_and(|at| at > now)
}

fn available_text_for(item: &TaskListItem, now: DateTime<Local>) -> String {
    item.task.available_at.as_deref().map_or_else(
        || "Now".to_string(),
        |at| {
            let text = available_text(at, now.date_naive());
            if is_deferred(item, now) {
                format!("Hidden until {text}")
            } else {
                format!("Since {text}")
            }
        },
    )
}

fn task_row(item: &TaskListItem, now: DateTime<Local>) -> TaskRow {
    let today = now.date_naive();
    let task = &item.task;
    let prefix = item.display_ref.split('-').next().unwrap_or_default();
    TaskRow {
        id: task.id.clone(),
        reference: item.display_ref.clone(),
        project: prefix.to_string(),
        project_key: task.project_key.clone(),
        title: task.title.clone(),
        status: task.status,
        priority: task.priority,
        reason: reason_text(item, today),
        blocked: item.unresolved_blocker_count > 0,
        recurring: item.recurrence.is_some(),
        series: (item.recurrence.as_ref())
            .map(|series| (series.series_id.clone(), series.series_ref.clone())),
        progress: item
            .epic_rollup
            .as_ref()
            .map(|rollup| format!("{}/{}", rollup.done, rollup.total)),
        epic: task.is_epic.then(|| epic_progress(item)),
        labels: item.labels.clone(),
        due: task
            .due_on
            .as_deref()
            .map(|due| due_text(due, today))
            .unwrap_or_default(),
        available: available_text_for(item, now),
        date_tone: date_tone(item, today),
        deleted: task.deleted,
        is_epic: task.is_epic,
        created: row_date(&task.created_at, today),
        updated: row_date(&task.updated_at, today),
        byline: byline(item, now),
        activity: activity(item, now),
    }
}

fn group_text(label: &str) -> (String, String) {
    GROUP_NAMES
        .iter()
        .find(|(group, ..)| *group == label)
        .map_or_else(
            || (label.to_string(), String::new()),
            |(_, name, hint)| ((*name).to_string(), (*hint).to_string()),
        )
}

/// The group of tasks closed during this visit to a view.
pub fn done_group(rows: Vec<usize>) -> Group {
    let (name, hint) = group_text("done");
    Group {
        name,
        hint,
        rows,
        lane: None,
    }
}

pub(super) fn epic_progress(item: &TaskListItem) -> EpicProgress {
    let count = |status: TaskStatus| {
        let matching = item.epic_children.iter();
        matching
            .filter(|child| child.status == status.as_str())
            .count()
    };
    let (done, canceled) = (count(TaskStatus::Done), count(TaskStatus::Canceled));
    EpicProgress {
        done,
        canceled,
        open: item.epic_children.len() - done - canceled,
        current: item
            .epic_children
            .iter()
            .find(|child| child.status == TaskStatus::Active.as_str())
            .map(|child| child.title.clone()),
    }
}

fn core_groups(store: &TuiStore) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for row in &store.task_list_view().rows {
        match row {
            TaskListRow::Group(group) => {
                let (name, hint) = group_text(&group.label);
                groups.push(Group {
                    name,
                    hint,
                    rows: Vec::with_capacity(group.count),
                    lane: None,
                });
            }
            TaskListRow::Task { task_index } | TaskListRow::EpicChild { task_index, .. } => {
                if groups.is_empty() {
                    groups.push(Group {
                        name: String::new(),
                        hint: String::new(),
                        rows: Vec::new(),
                        lane: None,
                    });
                }
                if let Some(group) = groups.last_mut() {
                    group.rows.push(*task_index);
                }
            }
        }
    }
    groups
}

fn named_groups(names: Vec<String>, rows: Vec<(usize, usize)>) -> Vec<Group> {
    let mut groups: Vec<Group> = names
        .into_iter()
        .map(|name| Group {
            name,
            hint: String::new(),
            rows: Vec::new(),
            lane: None,
        })
        .collect();
    for (group, row) in rows {
        groups[group].rows.push(row);
    }
    groups.retain(|group| !group.rows.is_empty());
    groups
}

/// The TUI's page: pressing work first, then taking turns between the reasons,
/// longest idle first within a project.
pub(super) fn review_page(
    mut candidates: Vec<(TaskListItem, TriageReason)>,
) -> Vec<(TaskListItem, TriageReason)> {
    const PAGE_SIZE: usize = 10;
    let idle = |item: &TaskListItem| item.queue.idle_days.unwrap_or(0);
    candidates.sort_by(|(left, _), (right, _)| {
        (left.task.project_key.cmp(&right.task.project_key)).then(idle(right).cmp(&idle(left)))
    });
    let (mut page, rest): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|(item, _)| {
        item.task.priority == TaskPriority::Urgent
            || matches!(
                item.queue.date_kind,
                Some(QueueDateKind::Overdue | QueueDateKind::DueToday)
            )
    });
    page.truncate(PAGE_SIZE);
    let mut pools: [VecDeque<_>; 3] = Default::default();
    for entry in rest {
        let pool = match entry.1 {
            TriageReason::StaleActive => 0,
            TriageReason::Inbox => 1,
            TriageReason::StaleTodo => 2,
        };
        pools[pool].push_back(entry);
    }
    while page.len() < PAGE_SIZE && pools.iter().any(|pool| !pool.is_empty()) {
        for pool in &mut pools {
            if page.len() < PAGE_SIZE
                && let Some(entry) = pool.pop_front()
            {
                page.push(entry);
            }
        }
    }
    page
}

/// Coarse buckets, sooner than core's day by day labels.
fn upcoming_groups(store: &TuiStore, now: DateTime<Local>) -> Vec<Group> {
    let names = ["Tomorrow or sooner", "Next 7 days", "Later"];
    let bucket = |item: &TaskListItem| {
        let at = item
            .task
            .available_at
            .as_deref()
            .and_then(parse_local_timestamp);
        let days = at.map_or(i64::MAX, |at| {
            (at.date_naive() - now.date_naive()).num_days()
        });
        match days {
            ..=1 => 0,
            2..=7 => 1,
            _ => 2,
        }
    };
    let rows = store.tasks.iter().enumerate();
    named_groups(
        names.map(String::from).to_vec(),
        rows.map(|(row, item)| (bucket(item), row)).collect(),
    )
}

/// A flat view's rows under the chosen grouping, in `rows`' order.
fn flat_groups(store: &TuiStore, rows: Vec<usize>, group: GroupBy) -> Vec<Group> {
    const STATUSES: [TaskStatus; 6] = [
        TaskStatus::Active,
        TaskStatus::Todo,
        TaskStatus::Inbox,
        TaskStatus::Backlog,
        TaskStatus::Done,
        TaskStatus::Canceled,
    ];
    const PRIORITIES: [TaskPriority; 5] = [
        TaskPriority::Urgent,
        TaskPriority::High,
        TaskPriority::Medium,
        TaskPriority::Low,
        TaskPriority::None,
    ];
    type Slot<'a> = Box<dyn Fn(&TaskListItem) -> usize + 'a>;
    let (names, slot): (Vec<String>, Slot<'_>) = match group {
        GroupBy::None => (vec![String::new()], Box::new(|_| 0)),
        GroupBy::Status => (
            STATUSES
                .map(|status| capitalised(&status.to_string()))
                .to_vec(),
            Box::new(|item| {
                STATUSES
                    .iter()
                    .position(|status| *status == item.task.status)
                    .unwrap_or_default()
            }),
        ),
        GroupBy::Priority => (
            PRIORITIES
                .map(|priority| capitalised(&priority.to_string()))
                .to_vec(),
            Box::new(|item| {
                PRIORITIES
                    .iter()
                    .position(|priority| *priority == item.task.priority)
                    .unwrap_or_default()
            }),
        ),
        GroupBy::Project => {
            let projects = &store.projects;
            (
                projects
                    .iter()
                    .map(|project| project.name.clone())
                    .collect(),
                Box::new(|item| {
                    projects
                        .iter()
                        .position(|project| project.key == item.task.project_key)
                        .unwrap_or_default()
                }),
            )
        }
    };
    if names.is_empty() {
        return Vec::new();
    }
    let rows = rows
        .into_iter()
        .map(|row| (slot(&store.tasks[row]), row))
        .collect();
    named_groups(names, rows)
}

fn list_groups(store: &TuiStore, now: DateTime<Local>, arrange: &Arrange) -> Vec<Group> {
    let filters = &store.view_state.filter_modifiers;
    if store.view_state.layout == TaskLayout::Columns {
        let board = store.column_board();
        return board
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| Group {
                name: column.config.name.clone(),
                hint: String::new(),
                rows: column.task_indices.clone(),
                lane: board.lane_entry_status(index),
            })
            .collect();
    }
    match store.view_state.render_mode() {
        TaskListRenderMode::Queue | TaskListRenderMode::Epics if !filters.deleted_only => {
            core_groups(store)
        }
        TaskListRenderMode::Upcoming => upcoming_groups(store, now),
        _ => {
            let mut rows: Vec<usize> = (0..store.tasks.len()).collect();
            if arrange.order == ViewOrder::Available {
                let descending = store.view_state.sort_direction() == SortDirection::Desc;
                rows.sort_by(|a, b| {
                    let at = |row: usize| store.tasks[row].task.available_at.as_ref();
                    match (at(*a), at(*b)) {
                        (None, None) => std::cmp::Ordering::Equal,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (Some(a), Some(b)) if descending => b.cmp(a),
                        (Some(a), Some(b)) => a.cmp(b),
                    }
                });
            }
            flat_groups(store, rows, arrange.group)
        }
    }
}

pub(super) fn view_info(store: &TuiStore, arrange: &Arrange) -> ViewInfo {
    let state = &store.view_state;
    let filters = &state.filter_modifiers;
    ViewInfo {
        query: state.query,
        project: match &state.scope {
            TaskScope::Workspace => None,
            TaskScope::Project(project) => Some(project.clone()),
        },
        columns: state.layout == TaskLayout::Columns,
        order: match (arrange.order, state.order) {
            (ViewOrder::Available, _) => ViewOrder::Available,
            (_, TaskOrder::Priority) => ViewOrder::Priority,
            (_, TaskOrder::DueOn) => ViewOrder::Due,
            (_, TaskOrder::Created) => ViewOrder::Created,
            (_, TaskOrder::Updated) => ViewOrder::Updated,
            (_, TaskOrder::Project) => ViewOrder::Project,
            (_, TaskOrder::Title) => ViewOrder::Title,
        },
        descending: state.sort_direction() == SortDirection::Desc,
        group: arrange.group,
        label: filters.label.clone(),
        priority: filters
            .priority
            .as_deref()
            .and_then(|text| TaskPriority::parse(text).ok()),
        deleted: match (filters.deleted_only, filters.include_deleted) {
            (true, _) => DeletedFilter::Only,
            (false, true) => DeletedFilter::Show,
            (false, false) => DeletedFilter::Hide,
        },
        metadata: arrange.metadata.clone(),
        search: match &state.projection_origin {
            TaskProjectionOrigin::Search { query, .. } => Some(query.clone()),
            _ => None,
        },
        lifecycle: state.recurring.lifecycle,
        closed: filters.closed == ClosedTaskVisibility::Included,
        due_today: state.query == TaskQuery::Overdue && filters.due_today,
    }
}

/// Reads the store's projection. `now` formats dates.
pub fn snapshot(store: &TuiStore, now: DateTime<Local>) -> Snapshot {
    arranged_snapshot(store, now, &Arrange::default())
}

fn sync_view(store: &TuiStore) -> SyncView {
    let status = &store.sync_status;
    SyncView {
        phase: status.phase.into(),
        server: status.server.clone(),
        pending: status.pending_changes.try_into().unwrap_or(usize::MAX),
        conflicts: status.conflicts.try_into().unwrap_or(usize::MAX),
        daemon_synced_at: status.daemon_sync.last_success_at,
        access_refused: status.access_refused_at.is_some(),
        vault_deleted: status.vault_deleted_at.is_some(),
    }
}

pub fn arranged_snapshot(store: &TuiStore, now: DateTime<Local>, arrange: &Arrange) -> Snapshot {
    let today = now.date_naive();
    let upcoming = store.view_state.query == TaskQuery::Upcoming;
    let tasks: Vec<TaskRow> = store
        .tasks
        .iter()
        .map(|item| {
            let mut row = task_row(item, now);
            if upcoming {
                let at = item.task.available_at.as_deref();
                row.due = at.map(|at| available_text(at, today)).unwrap_or_default();
            }
            row
        })
        .collect();
    let mut groups = if store.view_state.query == TaskQuery::Recurring {
        Vec::new()
    } else {
        list_groups(store, now, arrange)
    };
    if let Some(allowed) = &arrange.allowed {
        for group in &mut groups {
            group.rows.retain(|row| allowed.contains(&tasks[*row].id));
        }
        groups.retain(|group| !group.rows.is_empty());
    }
    let counts = &store.counts;
    Snapshot {
        workspace: store.active_workspace.name.clone(),
        tasks,
        groups,
        series: store.recurrence_series.iter().map(series_row).collect(),
        workspaces: store
            .workspaces
            .iter()
            .map(|workspace| NavEntry {
                name: workspace.name.clone(),
                key: workspace.key.clone(),
                prefix: String::new(),
                count: store
                    .workspace_open_counts
                    .get(&workspace.id)
                    .copied()
                    .unwrap_or_default(),
            })
            .collect(),
        projects: store
            .projects
            .iter()
            .map(|project| NavEntry {
                name: project.name.clone(),
                key: project.key.clone(),
                prefix: project.prefix.clone(),
                count: project.open_count,
            })
            .collect(),
        views: vec![
            ("queue", counts.open),
            ("ready", counts.ready),
            ("inbox", counts.inbox),
            ("upcoming", counts.upcoming),
            ("overdue", counts.overdue),
            ("blocked", counts.blocked),
            ("epics", counts.epics),
            ("recurring", counts.recurring),
            ("done", counts.done),
            ("active", counts.active),
            ("backlog", counts.backlog),
            ("todo", counts.todo),
            ("conflicts", counts.conflicts),
        ],
        view: view_info(store, arrange),
        labels: store.labels.clone(),
        fields: Vec::new(),
        recent: super::ledger::ledger(&store.recent_actions, now),
        sync: sync_view(store),
        welcome: false,
    }
}

/// Coarse age of an RFC3339 task or activity timestamp for list and detail text.
/// Under a minute reads as "just now"; hours and days round to the nearest unit.
/// Sync status uses `crate::session::ago` for elapsed whole units, including minutes.
pub fn relative_time(stamp: &str, now: DateTime<Local>) -> String {
    let minutes = parse_local_timestamp(stamp).map_or(0, |at| (now - at).num_minutes());
    let hours = minutes as f64 / 60.;
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if hours < 24. {
        format!("{}h ago", hours.round())
    } else {
        format!("{}d ago", (hours / 24.).round())
    }
}

/// Core has no desktop source, so a task made here names none.
fn source_name(source: TaskSource) -> Option<&'static str> {
    match source {
        TaskSource::Cli => Some("CLI"),
        TaskSource::Tui => Some("TUI"),
        TaskSource::Api => Some("API"),
        TaskSource::Ios => Some("iOS"),
        TaskSource::Android => Some("Android"),
        TaskSource::Unknown => None,
    }
}

pub(super) fn byline(item: &TaskListItem, now: DateTime<Local>) -> Byline {
    let task = &item.task;
    let created = parse_local_timestamp(&task.created_at);
    let updated = parse_local_timestamp(&task.updated_at);
    let changed = created
        .zip(updated)
        .is_some_and(|(created, updated)| updated - created >= BYLINE_UPDATE_GAP);
    Byline {
        created_ago: relative_time(&task.created_at, now),
        created_on: created
            .map_or_else(String::new, |at| at.format("%b %-d, %Y, %H:%M").to_string()),
        source: source_name(task.source).map(str::to_string),
        updated_ago: changed.then(|| relative_time(&task.updated_at, now)),
    }
}

pub fn detail(item: &TaskListItem, now: DateTime<Local>) -> Detail {
    detail_with(item, now, Vec::new())
}

pub(super) fn status_of(text: &str) -> TaskStatus {
    TaskStatus::parse(text).unwrap_or(TaskStatus::Todo)
}

pub(super) fn dependency(link: &aven_core::query::TaskDependencyLink) -> Link {
    Link {
        id: link.task_id.clone(),
        reference: link.display_ref.clone(),
        project: link
            .display_ref
            .split('-')
            .next()
            .unwrap_or_default()
            .to_string(),
        project_key: link.project_key.clone(),
        title: link.title.clone(),
        status: status_of(&link.status),
    }
}

/// An error toast: what failed, with the reason as its detail.
pub(crate) fn capitalised(text: &str) -> String {
    let mut letters = text.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// One history line, which leaves out the task's title and spells out any
/// availability instant in local time.
fn activity_text(action: &RecentActionItem, title: &str, today: NaiveDate) -> String {
    let summary = action.task_activity_summary(title);
    match action.task_activity_available_at() {
        Some(at) => format!("{summary} · {}", available_text(&at, today)),
        None => summary,
    }
}

/// What the task's history reads as, newest first, ending at its creation.
pub(super) fn activity(item: &TaskListItem, now: DateTime<Local>) -> Vec<(String, String)> {
    let mut lines: Vec<(String, String)> = item
        .activity
        .iter()
        .map(|action| {
            (
                capitalised(&activity_text(action, &item.task.title, now.date_naive())),
                relative_time(&action.created_at, now),
            )
        })
        .collect();
    lines.truncate(4);
    let created = source_name(item.task.source).map_or_else(
        || "Created".to_string(),
        |source| format!("Created from {source}"),
    );
    lines.push((created, relative_time(&item.task.created_at, now)));
    lines
}

pub(super) fn detail_with(item: &TaskListItem, now: DateTime<Local>, images: Vec<Image>) -> Detail {
    let live_related = item.related.iter().filter(|link| !link.deleted);
    Detail {
        task_id: item.task.id.clone(),
        row: task_row(item, now),
        description: item.task.description.clone(),
        available_at: item.task.available_at.clone(),
        due_on: item.task.due_on.clone(),
        project: item.task.project_key.clone(),
        notes: item
            .notes
            .iter()
            .rev()
            .map(|note| Note {
                id: note.id.clone(),
                at: relative_time(&note.created_at, now),
                body: note.body.clone(),
            })
            .collect(),
        images,
        epic: item.epic_parent.as_ref().map(dependency),
        blocked_by: item.depends_on.iter().map(dependency).collect(),
        blocks: item.blocks.iter().map(dependency).collect(),
        related: live_related
            .map(|link| Link {
                id: link.task_id.clone(),
                reference: link.display_ref.clone(),
                project: link
                    .display_ref
                    .split('-')
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                project_key: link.project_key.clone(),
                title: link.title.clone(),
                status: link.status,
            })
            .collect(),
        metadata: item
            .metadata
            .iter()
            .map(|field| (field.key.clone(), field.value.clone()))
            .collect(),
        children: Vec::new(),
        conflicts: Vec::new(),
    }
}

pub(super) fn status_label(status: TaskStatus) -> String {
    let mut text = status.to_string();
    text[..1].make_ascii_uppercase();
    text
}

pub(super) fn priority_label(priority: TaskPriority) -> String {
    let mut text = priority.to_string();
    text[..1].make_ascii_uppercase();
    text
}

/// The slot after a series' current one, while it still has any.
pub(super) fn series_schedule(series: &aven_core::types::RecurrenceSeries) -> String {
    let time = series
        .available_local_time
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_default();
    let due = match series.due_policy {
        RecurrenceDuePolicy::SameDay => "same-day",
        RecurrenceDuePolicy::None => "none",
    };
    let rule = natural_rule_label(series.rule);
    format_schedule_input("", "", &rule, &time, due, "")
}

pub(super) fn slot_after(
    series: &aven_core::types::RecurrenceSeries,
    slot: &str,
) -> Option<String> {
    let slot = NaiveDate::parse_from_str(slot, "%Y-%m-%d").ok()?;
    (series.state != RecurrenceSeriesState::Stopped)
        .then(|| next_slot_after(&series.rule, series.start_on, slot))
        .flatten()
        .map(|date| date.to_string())
}

fn series_row(item: &RecurrenceSeriesListItem) -> SeriesRow {
    let current = item.current_occurrence.as_ref();
    SeriesRow {
        id: item.series.id.clone(),
        reference: item.series_ref.clone(),
        project: item.project_key.clone(),
        title: item.series.title.clone(),
        state: item.series.state,
        rule: item.rule_label.clone(),
        after: current.and_then(|link| slot_after(&item.series, &link.slot_on)),
        current: current.map(|link| Occurrence {
            id: link.task_id.clone(),
            reference: link.task_ref.clone(),
            slot: link.slot_on.clone(),
        }),
    }
}

/// Which section of its epic a child reads under, as `epicFacts` sorts them.
pub(super) fn child_group(status: TaskStatus, waiting: bool) -> ChildGroup {
    match status {
        TaskStatus::Done | TaskStatus::Canceled => ChildGroup::Finished,
        TaskStatus::Active => ChildGroup::Progress,
        _ if waiting => ChildGroup::Waiting,
        _ => ChildGroup::Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ages_show_minutes_then_rounded_hours_and_days() {
        let now = parse_local_timestamp("2026-11-02T12:00:00+00:00").unwrap();
        for (minutes, expected) in [
            (-1, "just now"),
            (0, "just now"),
            (1, "1m ago"),
            (30, "30m ago"),
            (59, "59m ago"),
            (60, "1h ago"),
            (89, "1h ago"),
            (90, "2h ago"),
            (1439, "24h ago"),
            (1440, "1d ago"),
            (2159, "1d ago"),
            (2160, "2d ago"),
        ] {
            let at = now - chrono::TimeDelta::minutes(minutes);
            assert_eq!(relative_time(&at.to_rfc3339(), now), expected, "{minutes}");
        }
        assert_eq!(relative_time("invalid", now), "just now");
    }

    #[test]
    fn availability_shows_a_clock_only_for_a_time_today() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let at = |day: u32, hour: u32| {
            let naive = NaiveDate::from_ymd_opt(2026, 11, day)
                .and_then(|date| date.and_hms_opt(hour, 30, 0));
            naive.and_then(|naive| naive.and_local_timezone(Local).single())
        };
        let midnight = NaiveDate::from_ymd_opt(2026, 11, 2)
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .and_then(|naive| naive.and_local_timezone(Local).single());
        assert_eq!(availability_reason(midnight, today), "Available today");
        assert_eq!(
            availability_reason(at(2, 14), today),
            "Available since 14:30"
        );
        assert_eq!(
            availability_reason(at(1, 14), today),
            "Available since Nov 1"
        );
        assert_eq!(availability_reason(None, today), "Available");
    }

    #[test]
    fn activity_leaves_out_the_title_and_shows_the_time() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let change = |field: &str, summary: &str, detail: Option<&str>| RecentActionItem {
            change_id: String::new(),
            entity_type: "task".to_string(),
            entity_id: String::new(),
            op_type: aven_core::change_log::op_type::SET_FIELD.to_string(),
            field: Some(field.to_string()),
            created_at: String::new(),
            synced: true,
            target: aven_core::query::RecentActionTarget {
                display_ref: None,
                title: None,
                project_key: None,
                status: None,
                deleted: false,
            },
            verb: String::new(),
            summary: summary.to_string(),
            detail: detail.map(str::to_string),
            accent: String::new(),
            grouped_change_count: 1,
            initial_status: None,
        };

        let status = change("status", "changed status to todo: Task", None);
        assert_eq!(
            activity_text(&status, "Task", today),
            "changed status to todo"
        );

        let cleared = change("available_at", "cleared availability: Task", None);
        assert_eq!(
            activity_text(&cleared, "Task", today),
            "cleared availability"
        );

        let at = "2026-11-05T10:30:00+00:00";
        let moved = change("available_at", "changed availability: Task", Some(at));
        assert_eq!(
            activity_text(&moved, "Task", today),
            format!("changed availability · {}", available_text(at, today))
        );
    }

    #[test]
    fn short_dates_name_the_year_only_outside_this_one() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        for (day, expected) in [
            ((2026, 11, 3), "Nov 3"),
            ((2026, 1, 12), "Jan 12"),
            ((2025, 10, 12), "Oct 12, 2025"),
            ((2027, 2, 1), "Feb 1, 2027"),
        ] {
            let day = NaiveDate::from_ymd_opt(day.0, day.1, day.2).unwrap();
            assert_eq!(short_date(day, today), expected);
        }
    }

    #[test]
    fn row_dates_read_in_local_time() {
        use chrono::TimeZone as _;
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let at = Local.with_ymd_and_hms(2026, 11, 3, 9, 14, 0).unwrap();
        let date = row_date(&at.to_rfc3339(), today).unwrap();
        assert_eq!(date.day, "Nov 3");
        assert_eq!(date.full, "Nov 3, 2026, 09:14");
        assert_eq!(row_date("", today), None);
    }

    #[test]
    fn due_text_names_the_days_around_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        for (due, expected) in [
            ("2026-10-14", "Today"),
            ("2026-10-15", "Tomorrow"),
            ("2026-10-13", "Yesterday"),
            ("2026-10-02", "Oct 2"),
            ("2026-11-20", "Nov 20"),
        ] {
            assert_eq!(due_text(due, today), expected);
        }
    }
}
