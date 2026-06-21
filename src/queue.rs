use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum QueueBand {
    NeedsAction,
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
}

impl QueueMeta {
    pub(crate) fn idle_seconds(self) -> Option<i64> {
        self.idle_days.map(|days| days.saturating_mul(86_400))
    }
}

impl QueueBand {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NeedsAction => "needs action",
            Self::Focus => "focus",
            Self::Triage => "triage",
            Self::Later => "later",
        }
    }

    pub(crate) fn order(self) -> u8 {
        match self {
            Self::NeedsAction => 0,
            Self::Focus => 1,
            Self::Triage => 2,
            Self::Later => 3,
        }
    }
}

pub(crate) fn queue_meta(task: &Task, has_conflict: bool, now_seconds: i64) -> QueueMeta {
    let idle_days = unix_seconds(&task.updated_at).map(|updated| {
        now_seconds
            .saturating_sub(updated)
            .max(0)
            .saturating_div(86_400)
    });
    let idle = idle_days.unwrap_or(0);
    let score = status_score(&task.status)
        + priority_score(&task.priority)
        + idle_score(&task.status, idle)
        + if has_conflict { 50 } else { 0 };
    QueueMeta {
        band: queue_band(task, has_conflict, idle),
        score,
        idle_days,
    }
}

pub(crate) fn queue_order(a: (&Task, QueueMeta), b: (&Task, QueueMeta)) -> Ordering {
    a.1.band
        .order()
        .cmp(&b.1.band.order())
        .then_with(|| b.1.score.cmp(&a.1.score))
        .then_with(|| priority_score(&b.0.priority).cmp(&priority_score(&a.0.priority)))
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

fn queue_band(task: &Task, has_conflict: bool, idle_days: i64) -> QueueBand {
    if has_conflict || task.priority == "urgent" || (task.status == "active" && idle_days >= 7) {
        QueueBand::NeedsAction
    } else if task.status == "active" || (task.status == "todo" && task.priority == "high") {
        QueueBand::Focus
    } else if task.status == "inbox" || (task.status == "todo" && task.priority == "medium") {
        QueueBand::Triage
    } else {
        QueueBand::Later
    }
}

fn priority_score(priority: &str) -> i32 {
    match priority {
        "urgent" => 40,
        "high" => 30,
        "medium" => 20,
        "low" => 10,
        _ => 0,
    }
}

fn status_score(status: &str) -> i32 {
    match status {
        "active" => 50,
        "todo" => 35,
        "inbox" => 25,
        "backlog" => 5,
        _ => 0,
    }
}

fn idle_score(status: &str, idle_days: i64) -> i32 {
    match status {
        "active" if idle_days >= 14 => 25,
        "active" if idle_days >= 7 => 15,
        "todo" if idle_days >= 30 => 15,
        "todo" if idle_days >= 14 => 10,
        "inbox" if idle_days >= 14 => 10,
        "inbox" if idle_days >= 7 => 5,
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

    fn task(status: &str, priority: &str, updated_at: &str) -> Task {
        Task {
            id: format!("{status}-{priority}"),
            workspace_id: "workspace".to_string(),
            title: "task".to_string(),
            description: String::new(),
            project_key: "app".to_string(),
            project_prefix: "APP".to_string(),
            status: status.to_string(),
            priority: priority.to_string(),
            created_at: updated_at.to_string(),
            updated_at: updated_at.to_string(),
            deleted: false,
        }
    }

    #[test]
    fn urgent_and_conflicted_tasks_need_action() {
        let urgent = task("todo", "urgent", "1000");
        let conflicted = task("todo", "none", "1000");

        assert_eq!(
            queue_meta(&urgent, false, 1000).band,
            QueueBand::NeedsAction
        );
        assert_eq!(
            queue_meta(&conflicted, true, 1000).band,
            QueueBand::NeedsAction
        );
    }

    #[test]
    fn active_and_high_todo_are_focus() {
        assert_eq!(
            queue_meta(&task("active", "none", "1000"), false, 1000).band,
            QueueBand::Focus
        );
        assert_eq!(
            queue_meta(&task("todo", "high", "1000"), false, 1000).band,
            QueueBand::Focus
        );
    }

    #[test]
    fn old_active_tasks_need_action() {
        assert_eq!(
            queue_meta(&task("active", "none", "0"), false, 8 * 86_400).band,
            QueueBand::NeedsAction
        );
    }

    #[test]
    fn old_inbox_tasks_gain_triage_weight() {
        let old = queue_meta(&task("inbox", "none", "0"), false, 14 * 86_400);
        let fresh = queue_meta(&task("inbox", "none", "0"), false, 0);

        assert_eq!(old.band, QueueBand::Triage);
        assert!(old.score > fresh.score);
    }

    #[test]
    fn unix_seconds_parses_utc_timestamp() {
        assert_eq!(unix_seconds("1970-01-02T01:02:03Z"), Some(90_123));
    }
}
