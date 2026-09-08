use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDate;

use crate::choices::{TaskPriority, TaskStatus};
use crate::types::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueBand {
    NeedsAction,
    Available,
    Focus,
    Soon,
    Triage,
    Blocked,
    #[default]
    Later,
    Epics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueReason {
    Conflict,
    Urgent,
    DueToday,
    Overdue { days: u32 },
    Stale { days: u32 },
    Blocked { count: u32 },
    AvailableAfterDeferral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueDateKind {
    DueToday,
    Overdue,
    Due,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueMeta {
    pub band: QueueBand,
    pub score: i32,
    pub idle_days: Option<i64>,
    pub idle_seconds: Option<i64>,
    pub reason: Option<QueueReason>,
    pub date_kind: Option<QueueDateKind>,
    pub has_deferred_date: bool,
}

impl QueueBand {
    pub fn label(self) -> &'static str {
        match self {
            Self::NeedsAction => "needs action",
            Self::Available => "available",
            Self::Blocked => "blocked",
            Self::Focus => "focus",
            Self::Soon => "soon",
            Self::Triage => "triage",
            Self::Later => "later",
            Self::Epics => "epics",
        }
    }

    pub fn order(self) -> u8 {
        match self {
            Self::NeedsAction => 0,
            Self::Available => 1,
            Self::Focus => 2,
            Self::Soon => 3,
            Self::Triage => 4,
            Self::Blocked => 5,
            Self::Later => 6,
            Self::Epics => 7,
        }
    }
}

pub fn queue_meta_on(
    task: &Task,
    has_conflict: bool,
    unresolved_blocker_count: i64,
    dependent_count: i64,
    now_seconds: i64,
    local_today: NaiveDate,
) -> QueueMeta {
    let available = available_since_defer(task, now_seconds);
    let visible = work_visible(task, now_seconds);
    let due_days = task
        .due_on
        .as_deref()
        .and_then(|due| NaiveDate::parse_from_str(due, "%Y-%m-%d").ok())
        .map(|due| due.signed_duration_since(local_today).num_days());
    let activity_at = if available {
        task.available_at.as_deref().unwrap_or("")
    } else {
        &task.queue_activity_at
    };
    let idle_seconds =
        unix_seconds(activity_at).map(|activity| now_seconds.saturating_sub(activity).max(0));
    let idle_days = idle_seconds.map(|seconds| seconds.saturating_div(86_400));
    let idle = idle_days.unwrap_or(0);
    let score = status_score(task.status)
        + priority_score(task.priority)
        + idle_score(task.status, idle)
        + dependent_score(dependent_count)
        + if visible && !task.is_epic {
            due_score(due_days)
        } else {
            0
        }
        + if available { 100 } else { 0 }
        + if has_conflict { 50 } else { 0 };
    let (band, reason) = classify_queue(
        task,
        has_conflict,
        unresolved_blocker_count,
        idle,
        available,
        visible.then_some(due_days).flatten(),
    );
    let (date_kind, has_deferred_date) =
        queue_date_presentation(task, reason, due_days, now_seconds);
    QueueMeta {
        band,
        score,
        idle_days,
        idle_seconds,
        reason,
        date_kind,
        has_deferred_date,
    }
}

#[cfg(test)]
fn queue_meta(
    task: &Task,
    has_conflict: bool,
    has_unresolved_blockers: bool,
    dependent_count: i64,
    now_seconds: i64,
) -> QueueMeta {
    queue_meta_on(
        task,
        has_conflict,
        i64::from(has_unresolved_blockers),
        dependent_count,
        now_seconds,
        NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
    )
}

pub fn queue_order(a: (&Task, QueueMeta), b: (&Task, QueueMeta)) -> Ordering {
    a.1.band
        .order()
        .cmp(&b.1.band.order())
        .then_with(|| b.1.score.cmp(&a.1.score))
        .then_with(|| priority_score(b.0.priority).cmp(&priority_score(a.0.priority)))
        .then_with(|| a.0.created_at.cmp(&b.0.created_at))
        .then_with(|| a.0.id.cmp(&b.0.id))
}

fn due_score(days: Option<i64>) -> i32 {
    match days {
        Some(days) if days <= 0 => 40,
        Some(days) if days <= 7 => (5 * (8 - days)) as i32,
        _ => 0,
    }
}

pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn unix_seconds(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(seconds);
    }
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

pub fn work_visible(task: &Task, now_seconds: i64) -> bool {
    !task.deleted
        && task.status.is_open()
        && task.available_at.as_deref().is_none_or(|available_at| {
            unix_seconds(available_at).is_some_and(|available_at| available_at <= now_seconds)
        })
}

pub fn available_since_defer(task: &Task, now_seconds: i64) -> bool {
    if task.deleted || !task.status.is_open() {
        return false;
    }
    let Some(available_at) = task.available_at.as_deref().and_then(unix_seconds) else {
        return false;
    };
    if available_at > now_seconds {
        return false;
    }
    let activity = unix_seconds(&task.queue_activity_at).unwrap_or(0);
    activity < available_at || creation_seeded_queue_activity(task)
}

fn creation_seeded_queue_activity(task: &Task) -> bool {
    task.queue_activity_at == task.created_at && task.updated_at == task.created_at
}

fn classify_queue(
    task: &Task,
    has_conflict: bool,
    unresolved_blocker_count: i64,
    idle_days: i64,
    available: bool,
    due_days: Option<i64>,
) -> (QueueBand, Option<QueueReason>) {
    if task.is_epic {
        (QueueBand::Epics, None)
    } else if has_conflict {
        (QueueBand::NeedsAction, Some(QueueReason::Conflict))
    } else if task.priority == TaskPriority::Urgent {
        (QueueBand::NeedsAction, Some(QueueReason::Urgent))
    } else if task.status == TaskStatus::Active && idle_days >= 7 {
        (
            QueueBand::NeedsAction,
            Some(QueueReason::Stale {
                days: bounded_u32(idle_days),
            }),
        )
    } else if unresolved_blocker_count > 0 {
        (
            QueueBand::Blocked,
            Some(QueueReason::Blocked {
                count: bounded_u32(unresolved_blocker_count),
            }),
        )
    } else if let Some(days) = due_days.filter(|days| *days <= 0) {
        let reason = if days == 0 {
            QueueReason::DueToday
        } else {
            QueueReason::Overdue {
                days: bounded_u32(days.saturating_neg()),
            }
        };
        (QueueBand::NeedsAction, Some(reason))
    } else if available {
        (
            QueueBand::Available,
            Some(QueueReason::AvailableAfterDeferral),
        )
    } else if task.status == TaskStatus::Active
        || (task.status == TaskStatus::Todo && task.priority == TaskPriority::High)
    {
        (QueueBand::Focus, None)
    } else if task.status == TaskStatus::Todo && task.priority == TaskPriority::Medium {
        (QueueBand::Soon, None)
    } else if task.status == TaskStatus::Inbox {
        (QueueBand::Triage, None)
    } else {
        (QueueBand::Later, None)
    }
}

fn queue_date_presentation(
    task: &Task,
    reason: Option<QueueReason>,
    due_days: Option<i64>,
    now_seconds: i64,
) -> (Option<QueueDateKind>, bool) {
    if matches!(reason, Some(QueueReason::AvailableAfterDeferral))
        || task
            .available_at
            .as_deref()
            .and_then(unix_seconds)
            .is_some_and(|at| at > now_seconds)
    {
        return (None, true);
    }
    (
        due_days.map(|days| match days {
            0 => QueueDateKind::DueToday,
            ..=-1 => QueueDateKind::Overdue,
            _ => QueueDateKind::Due,
        }),
        false,
    )
}

fn bounded_u32(value: i64) -> u32 {
    value.clamp(0, i64::from(u32::MAX)) as u32
}

fn priority_score(priority: TaskPriority) -> i32 {
    match priority {
        TaskPriority::Urgent => 40,
        TaskPriority::High => 30,
        TaskPriority::Medium => 20,
        TaskPriority::Low => 10,
        TaskPriority::None => 0,
    }
}

fn dependent_score(dependent_count: i64) -> i32 {
    dependent_count.clamp(0, 5) as i32 * 6
}

fn status_score(status: TaskStatus) -> i32 {
    match status {
        TaskStatus::Active => 50,
        TaskStatus::Todo => 35,
        TaskStatus::Inbox => 25,
        TaskStatus::Backlog => 5,
        TaskStatus::Done | TaskStatus::Canceled => 0,
    }
}

fn idle_score(status: TaskStatus, idle_days: i64) -> i32 {
    match status {
        TaskStatus::Active if idle_days >= 14 => 25,
        TaskStatus::Active if idle_days >= 7 => 15,
        TaskStatus::Todo if idle_days >= 30 => 15,
        TaskStatus::Todo if idle_days >= 14 => 10,
        TaskStatus::Inbox if idle_days >= 14 => 10,
        TaskStatus::Inbox if idle_days >= 7 => 5,
        _ => 0,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: &str, priority: &str, queue_activity_at: &str) -> Task {
        Task {
            id: crate::test_support::task_id(&format!("{status}-{priority}")),
            workspace_id: "0000000000000001".parse().unwrap(),
            title: "task".to_string(),
            description: String::new(),
            project_id: "0000000000000001".parse().unwrap(),
            project_key: "app".to_string(),
            project_prefix: "APP".to_string(),
            status: TaskStatus::parse(status).expect("valid status"),
            priority: TaskPriority::parse(priority).expect("valid priority"),
            source: crate::choices::TaskSource::Unknown,
            created_at: queue_activity_at.to_string(),
            updated_at: queue_activity_at.to_string(),
            queue_activity_at: queue_activity_at.to_string(),
            available_at: None,
            due_on: None,
            deleted: false,
            is_epic: false,
        }
    }

    fn epic(status: &str, priority: &str, queue_activity_at: &str) -> Task {
        Task {
            is_epic: true,
            ..task(status, priority, queue_activity_at)
        }
    }

    #[test]
    fn epic_tasks_have_epic_band() {
        assert_eq!(
            queue_meta(&epic("active", "urgent", "0"), true, true, 0, 8 * 86_400).band,
            QueueBand::Epics
        );
    }

    #[test]
    fn active_and_high_todo_are_focus() {
        assert_eq!(
            queue_meta(&task("active", "none", "1000"), false, false, 0, 1000).band,
            QueueBand::Focus
        );
        assert_eq!(
            queue_meta(&task("todo", "high", "1000"), false, false, 0, 1000).band,
            QueueBand::Focus
        );
    }

    #[test]
    fn medium_todo_tasks_are_soon() {
        assert_eq!(
            queue_meta(&task("todo", "medium", "1000"), false, false, 0, 1000).band,
            QueueBand::Soon
        );
    }

    #[test]
    fn blocked_tasks_sort_below_actionable_groups() {
        assert!(QueueBand::Triage.order() < QueueBand::Blocked.order());
        assert!(QueueBand::Blocked.order() < QueueBand::Later.order());
    }

    #[test]
    fn open_dependents_add_queue_weight() {
        let plain = queue_meta(&task("todo", "medium", "1000"), false, false, 0, 1000);
        let blocker = queue_meta(&task("todo", "medium", "1000"), false, false, 3, 1000);

        assert_eq!(plain.band, QueueBand::Soon);
        assert_eq!(blocker.band, QueueBand::Soon);
        assert!(blocker.score > plain.score);
    }

    #[test]
    fn old_inbox_tasks_gain_triage_weight() {
        let old = queue_meta(&task("inbox", "none", "0"), false, false, 0, 14 * 86_400);
        let fresh = queue_meta(&task("inbox", "none", "0"), false, false, 0, 0);

        assert_eq!(old.band, QueueBand::Triage);
        assert!(old.score > fresh.score);
    }

    #[test]
    fn activity_after_availability_acknowledges_resurfacing() {
        let mut deferred = task("inbox", "none", "1000");
        deferred.available_at = Some("2000".to_string());
        deferred.updated_at = "2001".to_string();
        deferred.queue_activity_at = "2001".to_string();

        assert_eq!(
            queue_meta(&deferred, false, false, 0, 2001).band,
            QueueBand::Triage
        );
    }

    #[test]
    fn blocked_deferred_task_remains_blocked_when_available() {
        let mut deferred = task("todo", "none", "1000");
        deferred.available_at = Some("2000".to_string());

        assert_eq!(
            queue_meta(&deferred, false, true, 0, 2000).band,
            QueueBand::Blocked
        );
    }

    #[test]
    fn due_does_not_override_blockers_epics_or_future_availability() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let mut due = task("todo", "none", "1000");
        due.due_on = Some("2026-07-15".to_string());
        assert_eq!(
            queue_meta_on(&due, false, 1, 0, 2000, today).band,
            QueueBand::Blocked
        );

        due.is_epic = true;
        let epic_with_due = queue_meta_on(&due, false, 0, 0, 2000, today);
        assert_eq!(epic_with_due.band, QueueBand::Epics);
        due.due_on = None;
        assert_eq!(
            epic_with_due.score,
            queue_meta_on(&due, false, 0, 0, 2000, today).score
        );

        due.is_epic = false;
        due.due_on = Some("2026-07-15".to_string());
        due.available_at = Some("3000".to_string());
        assert_eq!(
            queue_meta_on(&due, false, 0, 0, 2000, today).band,
            QueueBand::Later
        );
    }

    #[test]
    fn due_week_adds_bounded_queue_weight() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let mut near = task("todo", "medium", "1000");
        near.due_on = Some("2026-07-17".to_string());
        let mut far = near.clone();
        far.due_on = Some("2026-07-23".to_string());

        let near_score = queue_meta_on(&near, false, 0, 0, 2000, today).score;
        let far_score = queue_meta_on(&far, false, 0, 0, 2000, today).score;
        assert!(near_score > far_score);
    }

    #[test]
    fn presentation_reasons_follow_classifier_precedence() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let mut item = task("active", "urgent", "0");
        item.due_on = Some("2026-07-14".to_string());

        let conflicted = queue_meta_on(&item, true, 3, 0, 8 * 86_400, today);
        assert_eq!(conflicted.band, QueueBand::NeedsAction);
        assert_eq!(conflicted.reason, Some(QueueReason::Conflict));

        let urgent = queue_meta_on(&item, false, 3, 0, 8 * 86_400, today);
        assert_eq!(urgent.band, QueueBand::NeedsAction);
        assert_eq!(urgent.reason, Some(QueueReason::Urgent));

        item.priority = TaskPriority::None;
        let stale = queue_meta_on(&item, false, 3, 0, 8 * 86_400, today);
        assert_eq!(stale.band, QueueBand::NeedsAction);
        assert_eq!(stale.reason, Some(QueueReason::Stale { days: 8 }));

        item.status = TaskStatus::Todo;
        let blocked = queue_meta_on(&item, false, 3, 0, 8 * 86_400, today);
        assert_eq!(blocked.band, QueueBand::Blocked);
        assert_eq!(blocked.reason, Some(QueueReason::Blocked { count: 3 }));

        let overdue = queue_meta_on(&item, false, 0, 0, 8 * 86_400, today);
        assert_eq!(overdue.reason, Some(QueueReason::Overdue { days: 2 }));
        assert_eq!(overdue.date_kind, Some(QueueDateKind::Overdue));
    }

    #[test]
    fn presentation_reasons_cover_date_boundaries_and_unexplained_bands() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let mut item = task("todo", "none", "1000");
        item.due_on = Some("2026-07-16".to_string());
        let due_today = queue_meta_on(&item, false, 0, 0, 2000, today);
        assert_eq!(due_today.band, QueueBand::NeedsAction);
        assert_eq!(due_today.reason, Some(QueueReason::DueToday));
        assert_eq!(due_today.date_kind, Some(QueueDateKind::DueToday));

        item.status = TaskStatus::Inbox;
        item.due_on = Some("2026-07-15".to_string());
        let overdue_inbox = queue_meta_on(&item, false, 0, 0, 2000, today);
        assert_eq!(overdue_inbox.band, QueueBand::NeedsAction);
        assert_eq!(overdue_inbox.reason, Some(QueueReason::Overdue { days: 1 }));

        item.status = TaskStatus::Todo;
        item.due_on = Some("2026-07-17".to_string());
        let future_due = queue_meta_on(&item, false, 0, 0, 2000, today);
        assert_eq!(future_due.reason, None);
        assert_eq!(future_due.date_kind, Some(QueueDateKind::Due));

        item.due_on = None;
        item.available_at = Some("2000".to_string());
        let available = queue_meta_on(&item, false, 0, 0, 2000, today);
        assert_eq!(available.band, QueueBand::Available);
        assert_eq!(available.reason, Some(QueueReason::AvailableAfterDeferral));
        assert_eq!(available.idle_seconds, Some(0));
        assert_eq!(available.date_kind, None);
        assert!(available.has_deferred_date);

        item.available_at = Some("3000".to_string());
        let deferred = queue_meta_on(&item, false, 0, 0, 2000, today);
        assert_eq!(deferred.band, QueueBand::Later);
        assert_eq!(deferred.reason, None);
        assert_eq!(deferred.date_kind, None);
        assert!(deferred.has_deferred_date);

        item.available_at = None;
        let plain = queue_meta_on(&item, false, 0, 0, 2000, today);
        assert_eq!(plain.band, QueueBand::Later);
        assert_eq!(plain.reason, None);
        assert_eq!(plain.date_kind, None);
        assert!(!plain.has_deferred_date);
    }

    #[test]
    fn presentation_numeric_payloads_are_bounded() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let blocked = queue_meta_on(&task("todo", "none", "0"), false, i64::MAX, 0, 0, today);
        assert_eq!(
            blocked.reason,
            Some(QueueReason::Blocked { count: u32::MAX })
        );
    }

    #[test]
    fn unix_seconds_parses_utc_timestamp() {
        assert_eq!(unix_seconds("1970-01-02T01:02:03Z"), Some(90_123));
    }
}
