use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::choices::{TaskPriority, TaskStatus};
use crate::types::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum QueueBand {
    NeedsAction,
    Blocked,
    Focus,
    Triage,
    #[default]
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct QueueMeta {
    pub(crate) band: QueueBand,
    pub(crate) score: i32,
    pub(crate) idle_days: Option<i64>,
    pub(crate) idle_seconds: Option<i64>,
}

impl QueueMeta {
    pub(crate) fn idle_seconds(self) -> Option<i64> {
        self.idle_seconds
    }
}

impl QueueBand {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NeedsAction => "needs action",
            Self::Blocked => "blocked",
            Self::Focus => "focus",
            Self::Triage => "triage",
            Self::Later => "later",
        }
    }

    pub(crate) fn order(self) -> u8 {
        match self {
            Self::NeedsAction => 0,
            Self::Blocked => 1,
            Self::Focus => 2,
            Self::Triage => 3,
            Self::Later => 4,
        }
    }
}

pub(crate) fn queue_meta(
    task: &Task,
    has_conflict: bool,
    has_unresolved_blockers: bool,
    now_seconds: i64,
) -> QueueMeta {
    let idle_seconds = unix_seconds(&task.queue_activity_at)
        .map(|activity| now_seconds.saturating_sub(activity).max(0));
    let idle_days = idle_seconds.map(|seconds| seconds.saturating_div(86_400));
    let idle = idle_days.unwrap_or(0);
    let score = status_score(task.status)
        + priority_score(task.priority)
        + idle_score(task.status, idle)
        + if has_conflict { 50 } else { 0 };
    QueueMeta {
        band: queue_band(task, has_conflict, has_unresolved_blockers, idle),
        score,
        idle_days,
        idle_seconds,
    }
}

pub(crate) fn queue_order(a: (&Task, QueueMeta), b: (&Task, QueueMeta)) -> Ordering {
    a.1.band
        .order()
        .cmp(&b.1.band.order())
        .then_with(|| b.1.score.cmp(&a.1.score))
        .then_with(|| priority_score(b.0.priority).cmp(&priority_score(a.0.priority)))
        .then_with(|| a.0.created_at.cmp(&b.0.created_at))
        .then_with(|| a.0.id.cmp(&b.0.id))
}

pub(crate) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn unix_seconds(value: &str) -> Option<i64> {
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

fn queue_band(
    task: &Task,
    has_conflict: bool,
    has_unresolved_blockers: bool,
    idle_days: i64,
) -> QueueBand {
    if has_conflict
        || task.priority == TaskPriority::Urgent
        || (task.status == TaskStatus::Active && idle_days >= 7)
    {
        QueueBand::NeedsAction
    } else if has_unresolved_blockers {
        QueueBand::Blocked
    } else if task.status == TaskStatus::Active
        || (task.status == TaskStatus::Todo && task.priority == TaskPriority::High)
    {
        QueueBand::Focus
    } else if task.status == TaskStatus::Inbox
        || (task.status == TaskStatus::Todo && task.priority == TaskPriority::Medium)
    {
        QueueBand::Triage
    } else {
        QueueBand::Later
    }
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
            id: format!("{status}-{priority}"),
            workspace_id: "workspace".to_string(),
            title: "task".to_string(),
            description: String::new(),
            project_id: "project-id".to_string(),
            project_key: "app".to_string(),
            project_prefix: "APP".to_string(),
            status: TaskStatus::parse(status).expect("valid status"),
            priority: TaskPriority::parse(priority).expect("valid priority"),
            created_at: queue_activity_at.to_string(),
            updated_at: queue_activity_at.to_string(),
            queue_activity_at: queue_activity_at.to_string(),
            deleted: false,
        }
    }

    #[test]
    fn urgent_and_conflicted_tasks_need_action() {
        let urgent = task("todo", "urgent", "1000");
        let conflicted = task("todo", "none", "1000");

        assert_eq!(
            queue_meta(&urgent, false, false, 1000).band,
            QueueBand::NeedsAction
        );
        assert_eq!(
            queue_meta(&conflicted, true, false, 1000).band,
            QueueBand::NeedsAction
        );
    }

    #[test]
    fn active_and_high_todo_are_focus() {
        assert_eq!(
            queue_meta(&task("active", "none", "1000"), false, false, 1000).band,
            QueueBand::Focus
        );
        assert_eq!(
            queue_meta(&task("todo", "high", "1000"), false, false, 1000).band,
            QueueBand::Focus
        );
    }

    #[test]
    fn old_active_tasks_need_action() {
        assert_eq!(
            queue_meta(&task("active", "none", "0"), false, false, 8 * 86_400).band,
            QueueBand::NeedsAction
        );
    }

    #[test]
    fn old_inbox_tasks_gain_triage_weight() {
        let old = queue_meta(&task("inbox", "none", "0"), false, false, 14 * 86_400);
        let fresh = queue_meta(&task("inbox", "none", "0"), false, false, 0);

        assert_eq!(old.band, QueueBand::Triage);
        assert!(old.score > fresh.score);
    }

    #[test]
    fn unix_seconds_parses_utc_timestamp() {
        assert_eq!(unix_seconds("1970-01-02T01:02:03Z"), Some(90_123));
    }
}
