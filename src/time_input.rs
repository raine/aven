use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};

use crate::queue::unix_seconds;

pub(crate) fn parse_available_at_input(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        bail!("error available-at-empty");
    }
    if input.eq_ignore_ascii_case("now") {
        return Ok(String::new());
    }
    if input.eq_ignore_ascii_case("today") {
        return relative_day(0);
    }
    if input.eq_ignore_ascii_case("tomorrow") {
        return relative_day(1);
    }
    if let Ok(seconds) = input.parse::<i64>() {
        return epoch_seconds_to_utc(seconds);
    }
    if is_iso_date(input) {
        return local_date_start(input);
    }
    if is_iso_timestamp(input) {
        let value = normalize_timestamp(input);
        validate_available_at_value(&value)?;
        return Ok(value);
    }
    bail!(
        "error invalid-available-at value={input} hint=\"use YYYY-MM-DD, YYYY-MM-DDTHH:MM:SSZ, epoch seconds, today, tomorrow, or now\""
    )
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

fn relative_day(offset_days: i64) -> Result<String> {
    let offset_days = u64::try_from(offset_days).context("invalid relative day")?;
    let date = Local::now()
        .date_naive()
        .checked_add_days(Days::new(offset_days))
        .context("relative date is out of range")?;
    local_midnight_to_utc(date)
}

fn local_date_start(value: &str) -> Result<String> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("invalid local date: {value}"))?;
    local_midnight_to_utc(date)
}

fn local_midnight_to_utc(date: NaiveDate) -> Result<String> {
    let local = Local
        .from_local_datetime(
            &date
                .and_hms_opt(0, 0, 0)
                .context("local midnight is out of range")?,
        )
        .single()
        .context("local midnight is ambiguous or unavailable")?;
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
    use super::*;

    #[test]
    fn parses_date_as_start_of_local_day() {
        let value = parse_available_at_input("2026-06-25").unwrap();
        let seconds = unix_seconds(&value).unwrap();
        let local = Local.timestamp_opt(seconds, 0).single().unwrap();

        assert_eq!(local.date_naive().to_string(), "2026-06-25");
        assert_eq!(local.time().to_string(), "00:00:00");
    }

    #[test]
    fn parses_timestamp_without_z_as_utc() {
        assert_eq!(
            parse_available_at_input("2026-06-25T10:11:12").unwrap(),
            "2026-06-25T10:11:12Z"
        );
    }

    #[test]
    fn parses_epoch_seconds_to_canonical_utc() {
        assert_eq!(
            parse_available_at_input("0").unwrap(),
            "1970-01-01T00:00:00Z"
        );
    }

    #[test]
    fn now_clears_availability() {
        assert_eq!(parse_available_at_input("now").unwrap(), "");
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
