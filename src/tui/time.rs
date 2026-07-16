use chrono::{Datelike, Local, TimeZone};

use crate::queue::unix_seconds;

pub(crate) fn available_day_label(available_at: &str, now_seconds: i64) -> String {
    let Some(available_seconds) = unix_seconds(available_at) else {
        return "later".to_string();
    };
    let Some(available_date) = local_date(available_seconds) else {
        return "later".to_string();
    };
    let Some(today) = local_date(now_seconds) else {
        return "later".to_string();
    };
    let days = (available_date - today).num_days();
    match days {
        i64::MIN..=-1 => "ready".to_string(),
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        2..=6 => available_date.format("%A").to_string().to_lowercase(),
        _ if available_date.year() == today.year() => available_date.format("%b %-d").to_string(),
        _ => available_date.format("%b %-d, %Y").to_string(),
    }
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
    let local = Local.timestamp_opt(seconds, 0).single()?;
    Some(local.format("%a %b %-d %-I:%M %p %Z").to_string())
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

fn local_date(seconds: i64) -> Option<chrono::NaiveDate> {
    Some(Local.timestamp_opt(seconds, 0).single()?.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn available_in_label_formats_future_values() {
        assert_eq!(
            available_in_label("1970-01-02T00:00:00Z", 0).unwrap(),
            "in1d"
        );
    }
}
