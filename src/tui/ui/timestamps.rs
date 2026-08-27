use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

pub(super) fn local_timestamp_display(value: &str) -> String {
    let Ok(datetime) = OffsetDateTime::parse(value, &Rfc3339) else {
        return value.to_string();
    };
    let Ok(offset) = UtcOffset::local_offset_at(datetime) else {
        return value.to_string();
    };
    timestamp_display_in_offset(datetime, offset).unwrap_or_else(|| value.to_string())
}

pub(super) fn optional_local_timestamp_display(value: Option<&str>, fallback: &str) -> String {
    value
        .map(local_timestamp_display)
        .unwrap_or_else(|| fallback.to_string())
}

pub(super) fn relative_timestamp_display(
    value: Option<&str>,
    now: OffsetDateTime,
    fallback: &str,
) -> String {
    let Some(value) = value else {
        return fallback.to_string();
    };
    let Ok(then) = OffsetDateTime::parse(value, &Rfc3339) else {
        return local_timestamp_display(value);
    };
    let seconds = (now - then).whole_seconds().max(0);
    match seconds {
        0..=9 => "just now".to_string(),
        10..=59 => format!("{seconds}s ago"),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn timestamp_display_in_offset(datetime: OffsetDateTime, offset: UtcOffset) -> Option<String> {
    datetime
        .to_offset(offset)
        .format(&time::macros::format_description!(
            "[year]-[month]-[day] [hour]:[minute]:[second]"
        ))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_display_uses_given_offset() {
        let datetime = OffsetDateTime::parse("2026-07-04T06:43:06Z", &Rfc3339).unwrap();
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap();

        assert_eq!(
            timestamp_display_in_offset(datetime, offset).unwrap(),
            "2026-07-04 08:43:06"
        );
    }

    #[test]
    fn local_timestamp_display_keeps_unparsed_values() {
        assert_eq!(
            local_timestamp_display("not-a-timestamp"),
            "not-a-timestamp"
        );
    }

    #[test]
    fn optional_local_timestamp_display_uses_fallback_for_absent_values() {
        assert_eq!(optional_local_timestamp_display(None, "never"), "never");
    }

    #[test]
    fn relative_timestamp_display_formats_recent_values() {
        let now = OffsetDateTime::parse("2026-08-28T12:00:00Z", &Rfc3339).unwrap();

        assert_eq!(
            relative_timestamp_display(Some("2026-08-28T11:59:48Z"), now, "never"),
            "12s ago"
        );
        assert_eq!(
            relative_timestamp_display(Some("2026-08-28T11:57:00Z"), now, "never"),
            "3m ago"
        );
    }
}
