use anyhow::{Context, Result, bail};
use chrono::{
    DateTime, Datelike, Days, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday,
};

use crate::queue::unix_seconds;

pub(crate) fn parse_available_at_input(input: &str) -> Result<String> {
    parse_available_at_input_at(input, Local::now())
}

fn parse_available_at_input_at<Tz>(input: &str, now: DateTime<Tz>) -> Result<String>
where
    Tz: TimeZone + Clone,
{
    let input = input.trim();
    if input.is_empty() {
        bail!("error available-at-empty");
    }
    if input.eq_ignore_ascii_case("now") {
        return Ok(String::new());
    }
    if let Ok(seconds) = input.parse::<i64>() {
        return epoch_seconds_to_utc(seconds);
    }
    if is_iso_date(input) {
        let date = parse_iso_date(input)?;
        return local_datetime_to_utc(now.timezone(), date.and_time(NaiveTime::MIN));
    }
    if is_iso_timestamp(input) {
        let value = normalize_timestamp(input);
        validate_available_at_value(&value)?;
        return Ok(value);
    }

    let normalized = input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let (date_input, time) = if let Some((date_input, time_input)) = normalized.rsplit_once(" at ")
    {
        if date_input.contains(" at ") {
            bail!(
                "error ambiguous-available-at value={input:?} hint=\"use one 'at' followed by a time, such as next monday at 9am\""
            );
        }
        let time = parse_local_time(time_input).with_context(|| {
            format!(
                "error invalid-available-at-time value={time_input:?} hint=\"use HH:MM, 9am, 9:30pm, noon, or midnight\""
            )
        })?;
        (date_input, time)
    } else {
        if parse_local_time(&normalized).is_some() {
            bail!(
                "error ambiguous-available-at-time value={input:?} hint=\"include a date, such as today at 9am or next monday at 9am\""
            );
        }
        (normalized.as_str(), NaiveTime::MIN)
    };

    let timezone = now.timezone();
    let today = now.date_naive();
    let date = parse_local_date_expression(date_input, today)?.with_context(|| {
        if weekday(date_input).is_some() {
            format!(
                "error ambiguous-available-at-weekday value={input:?} hint=\"use next {date_input}\""
            )
        } else {
            format!(
                "error invalid-available-at value={input:?} hint=\"use today, tomorrow, in N days, in N weeks, next monday, an ISO date, or add 'at 9am'\""
            )
        }
    })?;
    local_datetime_to_utc(timezone, date.and_time(time))
}

pub(crate) fn available_at_error_message(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    message
        .rsplit_once("hint=\"")
        .and_then(|(_, hint)| hint.split_once('"'))
        .map(|(hint, _)| hint.to_string())
        .unwrap_or_else(|| {
            "try tomorrow, in 2 weeks, next monday at 9am, or YYYY-MM-DD".to_string()
        })
}

pub(crate) fn validate_available_at_value(value: &str) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    if !is_canonical_utc_timestamp(value)
        || timestamp_components(value).is_none_or(|(year, month, day, hour, minute, second)| {
            month == 0
                || month > 12
                || day == 0
                || day > days_in_month(year, month)
                || hour > 23
                || minute > 59
                || second > 59
        })
        || unix_seconds(value).is_none()
    {
        bail!(
            "error invalid-available-at value={value} hint=\"use YYYY-MM-DDTHH:MM:SSZ or empty\""
        );
    }
    Ok(())
}

fn parse_local_date_expression(value: &str, today: NaiveDate) -> Result<Option<NaiveDate>> {
    let offset_days = match value {
        "today" => Some(0),
        "tomorrow" => Some(1),
        _ => None,
    };
    if let Some(offset_days) = offset_days {
        return add_days(today, offset_days).map(Some);
    }

    let words = value.split_whitespace().collect::<Vec<_>>();
    if let ["in", amount, unit] = words.as_slice() {
        let amount = amount.parse::<u64>().with_context(|| {
            format!(
                "error invalid-available-at-amount value={amount:?} hint=\"use a whole number, such as in 2 weeks\""
            )
        })?;
        let days = match *unit {
            "day" | "days" => amount,
            "week" | "weeks" => amount
                .checked_mul(7)
                .context("relative week count is out of range")?,
            _ => return Ok(None),
        };
        return add_days(today, days).map(Some);
    }

    if let ["next", weekday_name] = words.as_slice()
        && let Some(target) = weekday(weekday_name)
    {
        let current = today.weekday().num_days_from_monday();
        let target = target.num_days_from_monday();
        let mut days = (target + 7 - current) % 7;
        if days == 0 {
            days = 7;
        }
        return add_days(today, u64::from(days)).map(Some);
    }

    if is_iso_date(value) {
        return parse_iso_date(value).map(Some);
    }
    Ok(None)
}

fn add_days(date: NaiveDate, days: u64) -> Result<NaiveDate> {
    date.checked_add_days(Days::new(days))
        .context("relative date is out of range")
}

fn weekday(value: &str) -> Option<Weekday> {
    match value {
        "monday" => Some(Weekday::Mon),
        "tuesday" => Some(Weekday::Tue),
        "wednesday" => Some(Weekday::Wed),
        "thursday" => Some(Weekday::Thu),
        "friday" => Some(Weekday::Fri),
        "saturday" => Some(Weekday::Sat),
        "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

fn parse_local_time(value: &str) -> Option<NaiveTime> {
    let value = value.trim();
    match value {
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0),
        "noon" => return NaiveTime::from_hms_opt(12, 0, 0),
        _ => {}
    }

    let (clock, meridiem) = if let Some(clock) = value.strip_suffix("am") {
        (clock.trim(), Some(false))
    } else if let Some(clock) = value.strip_suffix("pm") {
        (clock.trim(), Some(true))
    } else {
        (value, None)
    };
    let (hour, minute) = if let Some((hour, minute)) = clock.split_once(':') {
        if minute.contains(':') {
            return None;
        }
        (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?)
    } else {
        meridiem?;
        (clock.parse::<u32>().ok()?, 0)
    };

    let hour = match meridiem {
        Some(is_pm) if (1..=12).contains(&hour) => (hour % 12) + u32::from(is_pm) * 12,
        Some(_) => return None,
        None => hour,
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
}

fn parse_iso_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").with_context(|| {
        format!(
            "error invalid-local-date value={value:?} hint=\"use a real calendar date in YYYY-MM-DD form\""
        )
    })
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn is_iso_timestamp(value: &str) -> bool {
    let value = value.strip_suffix('Z').unwrap_or(value);
    let Some((date, time)) = value.split_once('T') else {
        return false;
    };
    is_iso_date(date) && is_hms(time)
}

fn is_canonical_utc_timestamp(value: &str) -> bool {
    value.ends_with('Z') && value.len() == 20 && is_iso_timestamp(value)
}

fn is_hms(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 8
        && bytes[2] == b':'
        && bytes[5] == b':'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 2 | 5) || byte.is_ascii_digit())
}

fn normalize_timestamp(value: &str) -> String {
    if value.ends_with('Z') {
        value.to_string()
    } else {
        format!("{value}Z")
    }
}

fn timestamp_components(value: &str) -> Option<(i64, u32, u32, u32, u32, u32)> {
    let (date, time) = value.trim_end_matches('Z').split_once('T')?;
    let mut date = date.split('-');
    let year = date.next()?.parse().ok()?;
    let month = date.next()?.parse().ok()?;
    let day = date.next()?.parse().ok()?;
    let mut time = time.split(':');
    let hour = time.next()?.parse().ok()?;
    let minute = time.next()?.parse().ok()?;
    let second = time.next()?.parse().ok()?;
    Some((year, month, day, hour, minute, second))
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn local_datetime_to_utc<Tz>(timezone: Tz, value: NaiveDateTime) -> Result<String>
where
    Tz: TimeZone,
{
    let local = timezone.from_local_datetime(&value).single().with_context(|| {
        format!(
            "error ambiguous-or-unavailable-local-time value={} hint=\"choose another local time or use an explicit UTC timestamp\"",
            value.format("%Y-%m-%dT%H:%M:%S")
        )
    })?;
    let value = local
        .with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    validate_available_at_value(&value)?;
    Ok(value)
}

fn epoch_seconds_to_utc(seconds: i64) -> Result<String> {
    let value = DateTime::from_timestamp(seconds, 0)
        .context("epoch timestamp is out of range")?
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    validate_available_at_value(&value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn fixed_now() -> DateTime<FixedOffset> {
        FixedOffset::west_opt(5 * 60 * 60)
            .unwrap()
            .with_ymd_and_hms(2026, 7, 16, 15, 45, 0)
            .single()
            .unwrap()
    }

    fn parse_fixed(input: &str) -> Result<String> {
        parse_available_at_input_at(input, fixed_now())
    }

    #[test]
    fn parses_relative_calendar_dates_with_fixed_clock_and_timezone() {
        assert_eq!(parse_fixed("today").unwrap(), "2026-07-16T05:00:00Z");
        assert_eq!(parse_fixed("tomorrow").unwrap(), "2026-07-17T05:00:00Z");
        assert_eq!(parse_fixed("in 2 days").unwrap(), "2026-07-18T05:00:00Z");
        assert_eq!(parse_fixed("in 2 weeks").unwrap(), "2026-07-30T05:00:00Z");
    }

    #[test]
    fn next_weekday_is_strictly_after_today() {
        assert_eq!(parse_fixed("next monday").unwrap(), "2026-07-20T05:00:00Z");
        assert_eq!(
            parse_fixed("next thursday").unwrap(),
            "2026-07-23T05:00:00Z"
        );
    }

    #[test]
    fn parses_local_times_on_calendar_expressions() {
        assert_eq!(
            parse_fixed("next monday at 9am").unwrap(),
            "2026-07-20T14:00:00Z"
        );
        assert_eq!(
            parse_fixed("in 2 weeks at 14:30").unwrap(),
            "2026-07-30T19:30:00Z"
        );
        assert_eq!(
            parse_fixed("tomorrow at noon").unwrap(),
            "2026-07-17T17:00:00Z"
        );
    }

    #[test]
    fn parses_iso_date_as_start_of_supplied_local_day() {
        assert_eq!(parse_fixed("2026-06-25").unwrap(), "2026-06-25T05:00:00Z");
    }

    #[test]
    fn rejects_ambiguous_or_unsupported_expressions_with_guidance() {
        let weekday = parse_fixed("monday").unwrap_err().to_string();
        assert!(weekday.contains("use next monday"));

        let time = parse_fixed("9am").unwrap_err().to_string();
        assert!(time.contains("include a date"));

        let unsupported = parse_fixed("in two months").unwrap_err().to_string();
        assert!(unsupported.contains("use a whole number"));
    }

    #[test]
    fn presents_parser_guidance_without_internal_error_details() {
        let error = parse_fixed("monday").unwrap_err();
        assert_eq!(available_at_error_message(&error), "use next monday");
    }

    #[test]
    fn parses_timestamp_without_z_as_utc() {
        assert_eq!(
            parse_fixed("2026-06-25T10:11:12").unwrap(),
            "2026-06-25T10:11:12Z"
        );
    }

    #[test]
    fn parses_epoch_seconds_to_canonical_utc() {
        assert_eq!(parse_fixed("0").unwrap(), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn now_clears_availability() {
        assert_eq!(parse_fixed("now").unwrap(), "");
    }

    #[test]
    fn validates_only_canonical_values() {
        assert!(validate_available_at_value("").is_ok());
        assert!(validate_available_at_value("2026-06-25T10:11:12Z").is_ok());
        assert!(validate_available_at_value("2026-06-25").is_err());
        assert!(validate_available_at_value("0").is_err());
        assert!(validate_available_at_value("2026-99-99T99:99:99Z").is_err());
    }
}
