use anyhow::{Result, bail};

use crate::queue::unix_seconds;

pub fn validate_due_on_value(value: &str) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    if !is_iso_date(value) || chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err() {
        bail!("error invalid-due value={value} hint=\"use YYYY-MM-DD or empty\"");
    }
    Ok(())
}

pub fn validate_available_at_value(value: &str) -> Result<()> {
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

fn is_canonical_utc_timestamp(value: &str) -> bool {
    let Some(value) = value.strip_suffix('Z') else {
        return false;
    };
    if value.len() != 19 {
        return false;
    }
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
