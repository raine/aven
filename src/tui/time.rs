use chrono::{Datelike, Local, NaiveDate, TimeZone};

use crate::queue::unix_seconds;
use crate::tui::store::local_date;

pub(crate) fn due_state_at(due_on: &str, now_seconds: i64) -> crate::due::DueState {
    local_date(now_seconds)
        .map(|today| crate::due::due_state(due_on, today))
        .unwrap_or(crate::due::DueState::None)
}

pub(crate) fn due_label(due_on: &str, now_seconds: i64) -> Option<String> {
    let due = NaiveDate::parse_from_str(due_on, "%Y-%m-%d").ok()?;
    let today = local_date(now_seconds)?;
    Some(match crate::due::due_state(due_on, today) {
        crate::due::DueState::Overdue(days) => format!("{days}d late"),
        crate::due::DueState::Today => "due today".to_string(),
        crate::due::DueState::Future(1) => "due tomorrow".to_string(),
        crate::due::DueState::Future(days @ 2..=7) => format!("due in {days}d"),
        crate::due::DueState::Future(_) if due.year() == today.year() => {
            due.format("%b %-d").to_string()
        }
        crate::due::DueState::Future(_) => due.format("%b %-d, %Y").to_string(),
        crate::due::DueState::None => return None,
    })
}

pub(crate) fn compact_due_label(due_on: &str, now_seconds: i64) -> Option<String> {
    let due = NaiveDate::parse_from_str(due_on, "%Y-%m-%d").ok()?;
    let today = local_date(now_seconds)?;
    Some(match crate::due::due_state(due_on, today) {
        crate::due::DueState::Overdue(days @ 1..=999) => format!("{days}d!"),
        crate::due::DueState::Overdue(_) => "late!".to_string(),
        crate::due::DueState::Today => "today".to_string(),
        crate::due::DueState::Future(days @ 1..=7) => format!("+{days}d"),
        crate::due::DueState::Future(_) => due.format("%b%-d").to_string(),
        crate::due::DueState::None => return None,
    })
}

pub(crate) fn due_summary_lines(due_on: &str, now_seconds: i64) -> Option<[String; 2]> {
    let due = NaiveDate::parse_from_str(due_on, "%Y-%m-%d").ok()?;
    Some([
        due_label(due_on, now_seconds)?,
        due.format("%A, %b %-d, %Y").to_string(),
    ])
}

pub(crate) fn availability_summary_lines(
    available_at: &str,
    ready: bool,
    now_seconds: i64,
) -> Option<[String; 2]> {
    let available_seconds = unix_seconds(available_at)?;
    let local = local_datetime_label(available_seconds)?;
    let relative = if ready || available_seconds <= now_seconds {
        format!(
            "ready since {}",
            compact_duration(now_seconds.saturating_sub(available_seconds))
        )
    } else {
        format!(
            "available in {}",
            compact_duration(available_seconds.saturating_sub(now_seconds))
        )
    };
    Some([relative, local])
}

pub(crate) fn available_in_label(available_at: &str, now_seconds: i64) -> Option<String> {
    let available_seconds = unix_seconds(available_at)?;
    if available_seconds <= now_seconds {
        return Some("now".to_string());
    }
    Some(format!(
        "in{}",
        compact_duration(available_seconds - now_seconds)
    ))
}

pub(crate) fn local_datetime_label(seconds: i64) -> Option<String> {
    datetime_label_in(&Local, seconds)
}

fn datetime_label_in<Tz: TimeZone>(zone: &Tz, seconds: i64) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let local = zone.timestamp_opt(seconds, 0).single()?;
    Some(local.format("%a %b %-d %-I:%M %p %Z").to_string())
}

/// Formats an RFC 3339 availability instant in the local zone, keeping values
/// that do not parse.
pub(crate) fn available_at_display(available_at: &str) -> String {
    unix_seconds(available_at)
        .and_then(local_datetime_label)
        .unwrap_or_else(|| available_at.to_string())
}

/// Recent action summary with any availability instant shown in local time.
pub(crate) fn action_summary_display(action: &crate::query::RecentActionItem) -> String {
    match action_available_at(action) {
        Some(available_at) => {
            action
                .summary
                .replacen(available_at, &available_at_display(available_at), 1)
        }
        None => action.summary.clone(),
    }
}

/// Recent action detail with an availability instant shown in local time.
pub(crate) fn action_detail_display(action: &crate::query::RecentActionItem) -> Option<String> {
    match action_available_at(action) {
        Some(available_at) => Some(available_at_display(available_at)),
        None => action.detail.clone(),
    }
}

/// Task activity summary with the availability instant appended in local time.
pub(crate) fn task_activity_summary_display(
    action: &crate::query::RecentActionItem,
    task_title: &str,
) -> String {
    let summary = action.task_activity_summary(task_title);
    match action.task_activity_available_at() {
        Some(available_at) => format!("{summary} · {}", available_at_display(&available_at)),
        None => summary,
    }
}

fn action_available_at(action: &crate::query::RecentActionItem) -> Option<&str> {
    (action.field.as_deref() == Some("available_at"))
        .then_some(action.detail.as_deref())
        .flatten()
        .filter(|detail| !detail.is_empty())
}

/// Conflict value for display, formatting availability instants in local time.
pub(crate) fn conflict_value_display(field: &str, value: &str) -> String {
    if field == "available_at" && !value.is_empty() {
        available_at_display(value)
    } else {
        value.to_string()
    }
}

pub(crate) fn compact_duration(seconds: i64) -> String {
    let minutes = seconds.max(0) / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_label_formats_instant_in_given_zone() {
        let zone = chrono::FixedOffset::east_opt(3 * 3600).unwrap();
        let seconds = unix_seconds("2026-09-28T06:00:00Z").unwrap();
        assert_eq!(
            datetime_label_in(&zone, seconds).as_deref(),
            Some("Mon Sep 28 9:00 AM +03:00")
        );
    }

    #[test]
    fn available_at_display_uses_local_label_and_keeps_unparsed_values() {
        let seconds = unix_seconds("2026-09-28T06:00:00Z").unwrap();
        assert_eq!(
            available_at_display("2026-09-28T06:00:00Z"),
            local_datetime_label(seconds).unwrap()
        );
        assert_eq!(available_at_display("not-a-time"), "not-a-time");
        assert_eq!(conflict_value_display("status", "todo"), "todo");
        assert_eq!(conflict_value_display("available_at", ""), "");
    }

    #[test]
    fn compact_duration_formats_minutes_hours_days_weeks_and_months() {
        assert_eq!(compact_duration(-1), "0m");
        assert_eq!(compact_duration(0), "0m");
        assert_eq!(compact_duration(59), "0m");
        assert_eq!(compact_duration(60), "1m");
        assert_eq!(compact_duration(3_599), "59m");
        assert_eq!(compact_duration(3_600), "1h");
        assert_eq!(compact_duration(86_399), "23h");
        assert_eq!(compact_duration(13 * 86_400), "13d");
        assert_eq!(compact_duration(9 * 7 * 86_400), "9w");
        assert_eq!(compact_duration(122 * 86_400), "4mo");
    }

    #[test]
    fn compact_due_labels_fit_the_task_list_column() {
        let now = Local
            .with_ymd_and_hms(2026, 7, 16, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();

        assert_eq!(compact_due_label("2026-07-13", now).unwrap(), "3d!");
        assert_eq!(compact_due_label("2026-07-16", now).unwrap(), "today");
        assert_eq!(compact_due_label("2026-07-17", now).unwrap(), "+1d");
        assert_eq!(compact_due_label("2026-07-19", now).unwrap(), "+3d");
        assert_eq!(compact_due_label("2026-07-24", now).unwrap(), "Jul24");
        assert_eq!(compact_due_label("2027-01-01", now).unwrap(), "Jan1");
    }

    #[test]
    fn available_in_label_formats_future_values() {
        assert_eq!(
            available_in_label("1970-01-02T00:00:00Z", 0).unwrap(),
            "in1d"
        );
    }
}
