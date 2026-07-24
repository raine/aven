use anyhow::{Context, Result, bail};
use chrono::{
    DateTime, Datelike, Days, Local, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
    Weekday,
};

use aven_core::time_validation::{validate_available_at_value, validate_due_on_value};

pub fn parse_available_at_input(input: &str) -> Result<String> {
    parse_available_at_input_at(input, Local::now())
}

pub fn parse_due_on_input(input: &str) -> Result<String> {
    parse_due_on_input_at(input, Local::now().date_naive())
}

fn parse_due_on_input_at(input: &str, today: NaiveDate) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        bail!("error due-empty");
    }
    if input.eq_ignore_ascii_case("none") || input.eq_ignore_ascii_case("clear") {
        return Ok(String::new());
    }

    let normalized = input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let date = parse_local_date_expression(&normalized, today, "due")?.with_context(|| {
        if weekday(&normalized).is_some() {
            format!("error ambiguous-due-weekday value={input:?} hint=\"use next {normalized}\"")
        } else {
            format!(
                "error invalid-due value={input:?} hint=\"use today, tomorrow, 2d, 2w, in N months, next week, next monday, or an ISO date\""
            )
        }
    })?;
    let value = date.format("%Y-%m-%d").to_string();
    validate_due_on_value(&value)?;
    Ok(value)
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
    let date = parse_local_date_expression(date_input, today, "available-at")?.with_context(|| {
        if weekday(date_input).is_some() {
            format!(
                "error ambiguous-available-at-weekday value={input:?} hint=\"use next {date_input}\""
            )
        } else {
            format!(
                "error invalid-available-at value={input:?} hint=\"use today, tomorrow, 2d, 2w, in N months, next week, next monday, an ISO date, or add 'at 9am'\""
            )
        }
    })?;
    local_datetime_to_utc(timezone, date.and_time(time))
}

pub fn available_at_error_message(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    message
        .rsplit_once("hint=\"")
        .and_then(|(_, hint)| hint.split_once('"'))
        .map(|(hint, _)| hint.to_string())
        .unwrap_or_else(|| {
            "try tomorrow, 2d, 2w, in 2 months, next week, next monday at 9am, or YYYY-MM-DD"
                .to_string()
        })
}

pub fn due_on_error_message(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    message
        .rsplit_once("hint=\"")
        .and_then(|(_, hint)| hint.split_once('"'))
        .map(|(hint, _)| hint.to_string())
        .unwrap_or_else(|| {
            "try tomorrow, 2d, 2w, in 2 months, next week, next monday, or YYYY-MM-DD".to_string()
        })
}

fn parse_local_date_expression(
    value: &str,
    today: NaiveDate,
    field_slug: &str,
) -> Result<Option<NaiveDate>> {
    let offset_days = match value {
        "today" => Some(0),
        "tomorrow" => Some(1),
        _ => compact_day_offset(value)?,
    };
    if let Some(offset_days) = offset_days {
        return add_days(today, offset_days).map(Some);
    }

    if value == "next week" {
        return next_weekday(today, Weekday::Mon).map(Some);
    }

    let words = value.split_whitespace().collect::<Vec<_>>();
    if let ["in", amount, unit] = words.as_slice() {
        let amount = amount.parse::<u64>().with_context(|| {
            format!(
                "error invalid-{field_slug}-amount value={amount:?} hint=\"use a whole number, such as in 2 weeks\""
            )
        })?;
        return match *unit {
            "day" | "days" => add_days(today, amount).map(Some),
            "week" | "weeks" => amount
                .checked_mul(7)
                .context("relative week count is out of range")
                .and_then(|days| add_days(today, days))
                .map(Some),
            "month" | "months" => add_months(today, amount).map(Some),
            _ => Ok(None),
        };
    }

    if let ["next", weekday_name] = words.as_slice()
        && let Some(target) = weekday(weekday_name)
    {
        return next_weekday(today, target).map(Some);
    }

    if is_iso_date(value) {
        return parse_iso_date(value).map(Some);
    }
    Ok(None)
}

fn compact_day_offset(value: &str) -> Result<Option<u64>> {
    for (suffix, multiplier) in [("d", 1), ("w", 7)] {
        let Some(amount) = value.strip_suffix(suffix) else {
            continue;
        };
        if amount.is_empty() || !amount.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let amount = amount
            .parse::<u64>()
            .context("compact relative date is out of range")?;
        let days = amount
            .checked_mul(multiplier)
            .context("compact relative date is out of range")?;
        return Ok(Some(days));
    }
    Ok(None)
}

fn next_weekday(date: NaiveDate, target: Weekday) -> Result<NaiveDate> {
    let current = date.weekday().num_days_from_monday();
    let target = target.num_days_from_monday();
    let mut days = (target + 7 - current) % 7;
    if days == 0 {
        days = 7;
    }
    add_days(date, u64::from(days))
}

fn add_days(date: NaiveDate, days: u64) -> Result<NaiveDate> {
    date.checked_add_days(Days::new(days))
        .context("relative date is out of range")
}

fn add_months(date: NaiveDate, months: u64) -> Result<NaiveDate> {
    let months = u32::try_from(months).context("relative month count is out of range")?;
    date.checked_add_months(Months::new(months))
        .context("relative month count is out of range")
}

fn weekday(value: &str) -> Option<Weekday> {
    match value.trim_end_matches('.') {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Some(Weekday::Tue),
        "wed" | "weds" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
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

    fn parse_due_fixed(input: &str) -> Result<String> {
        parse_due_on_input_at(input, fixed_now().date_naive())
    }

    #[test]
    fn parses_due_calendar_expressions_without_timezones() {
        assert_eq!(parse_due_fixed("today").unwrap(), "2026-07-16");
        assert_eq!(parse_due_fixed("tomorrow").unwrap(), "2026-07-17");
        assert_eq!(parse_due_fixed("in 2 weeks").unwrap(), "2026-07-30");
        assert_eq!(parse_due_fixed("next monday").unwrap(), "2026-07-20");
        assert_eq!(parse_due_fixed("2028-02-29").unwrap(), "2028-02-29");
    }

    #[test]
    fn due_input_clears_and_rejects_timed_or_invalid_dates() {
        assert_eq!(parse_due_fixed("none").unwrap(), "");
        assert_eq!(parse_due_fixed("clear").unwrap(), "");
        assert!(parse_due_fixed("tomorrow at 9am").is_err());
        assert!(parse_due_fixed("2026-07-17T09:00:00Z").is_err());
        assert!(
            parse_due_fixed("monday")
                .unwrap_err()
                .to_string()
                .contains("use next monday")
        );
        assert!(validate_due_on_value("2026-02-29").is_err());
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
    fn parses_compact_offsets_next_week_and_calendar_months() {
        assert_eq!(parse_due_fixed("2d").unwrap(), "2026-07-18");
        assert_eq!(parse_due_fixed("2w").unwrap(), "2026-07-30");
        assert_eq!(parse_due_fixed("in 2 months").unwrap(), "2026-09-16");
        assert_eq!(
            parse_fixed("next week at 9am").unwrap(),
            "2026-07-20T14:00:00Z"
        );

        let january_end = NaiveDate::from_ymd_opt(2025, 1, 31).unwrap();
        assert_eq!(
            parse_due_on_input_at("in 1 month", january_end).unwrap(),
            "2025-02-28"
        );
        let leap_january_end = NaiveDate::from_ymd_opt(2024, 1, 31).unwrap();
        assert_eq!(
            parse_due_on_input_at("in 1 month", leap_january_end).unwrap(),
            "2024-02-29"
        );
    }

    #[test]
    fn accepts_common_weekday_abbreviations() {
        for (input, expected_date, expected_available_at) in [
            ("mon", "2026-07-20", "2026-07-20T14:00:00Z"),
            ("tue", "2026-07-21", "2026-07-21T14:00:00Z"),
            ("tues", "2026-07-21", "2026-07-21T14:00:00Z"),
            ("wed", "2026-07-22", "2026-07-22T14:00:00Z"),
            ("weds", "2026-07-22", "2026-07-22T14:00:00Z"),
            ("thu", "2026-07-23", "2026-07-23T14:00:00Z"),
            ("thur", "2026-07-23", "2026-07-23T14:00:00Z"),
            ("thurs", "2026-07-23", "2026-07-23T14:00:00Z"),
            ("fri", "2026-07-17", "2026-07-17T14:00:00Z"),
            ("sat", "2026-07-18", "2026-07-18T14:00:00Z"),
            ("sun", "2026-07-19", "2026-07-19T14:00:00Z"),
        ] {
            assert_eq!(
                parse_due_fixed(&format!("next {input}")).unwrap(),
                expected_date
            );
            assert_eq!(
                parse_fixed(&format!("next {input} at 9am")).unwrap(),
                expected_available_at
            );
        }
        assert_eq!(
            parse_fixed("next Mon. at 9am").unwrap(),
            "2026-07-20T14:00:00Z"
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
