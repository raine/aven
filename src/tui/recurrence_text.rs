use anyhow::{Result, anyhow, bail};
use aven_core::recurrence::{RecurrenceFrequency, RecurrenceRule};
use chrono::Weekday;

pub(crate) fn canonical_rule_input(input: &str) -> Result<Option<String>> {
    let normalized = input
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() || normalized == "none" {
        return Ok(None);
    }
    if matches!(normalized.as_str(), "daily" | "every day") {
        return Ok(Some("daily".to_string()));
    }
    if matches!(normalized.as_str(), "weekdays" | "every weekday") {
        return Ok(Some("weekdays".to_string()));
    }

    if let Some(day) = normalized.strip_suffix('s').and_then(parse_weekday) {
        return Ok(Some(format!("weekly on {}", weekday_short(day))));
    }
    if let Some(days) = normalized.strip_prefix("every ") {
        if let Some(day) = parse_weekday(days) {
            return Ok(Some(format!("weekly on {}", weekday_short(day))));
        }
        if let Some((interval, days)) = parse_week_interval(days)? {
            return Ok(Some(format!(
                "every {interval} weeks on {}",
                canonical_weekdays(days)?
            )));
        }
        return Ok(Some(format!("weekly on {}", canonical_weekdays(days)?)));
    }

    bail!(rule_guidance())
}

fn parse_week_interval(value: &str) -> Result<Option<(u32, &str)>> {
    let Some((interval, days)) = value.split_once(" weeks on ") else {
        return Ok(None);
    };
    let interval = interval
        .parse::<u32>()
        .map_err(|_| anyhow!(rule_guidance()))?;
    if interval == 0 {
        bail!(rule_guidance());
    }
    Ok(Some((interval, days)))
}

fn canonical_weekdays(value: &str) -> Result<String> {
    let normalized = value.replace(',', " and ");
    let mut weekdays = Vec::new();
    for value in normalized.split(" and ").map(str::trim) {
        let Some(weekday) = parse_weekday(value) else {
            bail!(rule_guidance());
        };
        if !weekdays.contains(&weekday) {
            weekdays.push(weekday);
        }
    }
    if weekdays.is_empty() {
        bail!(rule_guidance());
    }
    weekdays.sort_by_key(|weekday| weekday.num_days_from_monday());
    Ok(weekdays
        .into_iter()
        .map(weekday_short)
        .collect::<Vec<_>>()
        .join(","))
}

fn parse_weekday(value: &str) -> Option<Weekday> {
    match value.trim() {
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

fn weekday_short(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

pub(crate) fn natural_rule_label(rule: RecurrenceRule) -> String {
    match rule.frequency() {
        RecurrenceFrequency::Daily => "Every day".to_string(),
        RecurrenceFrequency::Weekly if rule == RecurrenceRule::weekdays() => {
            "Every weekday".to_string()
        }
        RecurrenceFrequency::Weekly => {
            let days = rule
                .weekdays_set()
                .iter()
                .map(weekday_name)
                .collect::<Vec<_>>();
            let days = match days.as_slice() {
                [] => String::new(),
                [day] => (*day).to_string(),
                [first, second] => format!("{first} and {second}"),
                _ => {
                    let (last, rest) = days.split_last().expect("weekday list is nonempty");
                    format!("{}, and {last}", rest.join(", "))
                }
            };
            if rule.interval() == 1 {
                format!("Every {days}")
            } else {
                format!("Every {} weeks on {days}", rule.interval())
            }
        }
    }
}

pub(crate) const fn rule_guidance() -> &'static str {
    "Try daily, weekdays, every Friday, every Monday and Thursday, or every 4 weeks on Monday and Thursday"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_natural_rule_variants() {
        for (input, expected) in [
            ("daily", "daily"),
            ("every day", "daily"),
            ("weekdays", "weekdays"),
            ("every weekday", "weekdays"),
            ("every Friday", "weekly on fri"),
            ("Fridays", "weekly on fri"),
            ("every Monday and Thursday", "weekly on mon,thu"),
            (
                "every 4 weeks on Monday and Thursday",
                "every 4 weeks on mon,thu",
            ),
        ] {
            assert_eq!(
                canonical_rule_input(input).unwrap().as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn invalid_natural_rule_has_accessible_guidance() {
        let error = canonical_rule_input("sometimes").unwrap_err().to_string();
        assert!(error.contains("Try daily"));
        assert!(error.contains("every 4 weeks"));
    }

    #[test]
    fn formats_rules_in_natural_language() {
        assert_eq!(natural_rule_label(RecurrenceRule::daily()), "Every day");
        assert_eq!(
            natural_rule_label(RecurrenceRule::weekdays()),
            "Every weekday"
        );
        assert_eq!(
            natural_rule_label(RecurrenceRule::weekly(Weekday::Wed)),
            "Every Wednesday"
        );
        assert_eq!(
            natural_rule_label(
                RecurrenceRule::every_n_weeks_on(4, [Weekday::Mon, Weekday::Thu]).unwrap()
            ),
            "Every 4 weeks on Monday and Thursday"
        );
    }
}
