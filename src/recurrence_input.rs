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
    if matches!(normalized.as_str(), "monthly" | "every month") {
        return Ok(Some("monthly".to_string()));
    }
    if matches!(normalized.as_str(), "yearly" | "annually" | "every year") {
        return Ok(Some("yearly".to_string()));
    }
    if normalized == "fortnightly" {
        return Ok(Some("fortnightly".to_string()));
    }
    if matches!(normalized.as_str(), "weekdays" | "every weekday") {
        return Ok(Some("weekdays".to_string()));
    }

    if let Some(day) = normalized.strip_suffix('s').and_then(parse_weekday) {
        return Ok(Some(format!("weekly on {}", weekday_short(day))));
    }
    if let Some(value) = normalized.strip_prefix("every ") {
        if let Some((interval, unit)) = parse_interval_unit(value)? {
            return Ok(Some(format!("every {interval} {unit}")));
        }
        if let Some(day) = parse_weekday(value) {
            return Ok(Some(format!("weekly on {}", weekday_short(day))));
        }
        if let Some((interval, days)) = parse_week_interval(value)? {
            return Ok(Some(format!(
                "every {interval} weeks on {}",
                canonical_weekdays(days)?
            )));
        }
        return Ok(Some(format!("weekly on {}", canonical_weekdays(value)?)));
    }

    bail!(rule_guidance())
}

fn parse_interval_unit(value: &str) -> Result<Option<(u32, &'static str)>> {
    let words = value.split(' ').collect::<Vec<_>>();
    let [interval, unit] = words.as_slice() else {
        return Ok(None);
    };
    let canonical_unit = match *unit {
        "days" => "days",
        "weeks" => "weeks",
        "months" => "months",
        "years" => "years",
        _ => return Ok(None),
    };
    Ok(Some((parse_positive_interval(interval)?, canonical_unit)))
}

fn parse_week_interval(value: &str) -> Result<Option<(u32, &str)>> {
    let Some((interval, days)) = value.split_once(" weeks on ") else {
        return Ok(None);
    };
    Ok(Some((parse_positive_interval(interval)?, days)))
}

fn parse_positive_interval(value: &str) -> Result<u32> {
    let interval = value.parse::<u32>().map_err(|_| anyhow!(rule_guidance()))?;
    if interval == 0 {
        bail!(rule_guidance());
    }
    Ok(interval)
}

fn canonical_weekdays(value: &str) -> Result<String> {
    let normalized = value.replace(", and ", ",").replace(" and ", ",");
    let mut weekdays = Vec::new();
    for value in normalized.split(',').map(str::trim) {
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
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tuesday" => Some(Weekday::Tue),
        "wed" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
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
        RecurrenceFrequency::Daily if rule.interval() == 1 => "Every day".to_string(),
        RecurrenceFrequency::Daily => format!("Every {} days", rule.interval()),
        RecurrenceFrequency::Monthly if rule.interval() == 1 => "Every month".to_string(),
        RecurrenceFrequency::Monthly => format!("Every {} months", rule.interval()),
        RecurrenceFrequency::Yearly if rule.interval() == 1 => "Every year".to_string(),
        RecurrenceFrequency::Yearly => format!("Every {} years", rule.interval()),
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
    "Try daily, every 3 days, weekdays, monthly, every 3 months, yearly, every 2 years, every Friday, every 3 weeks, or every 4 weeks on Monday and Thursday"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_natural_rule_variants() {
        for (input, expected) in [
            ("daily", "daily"),
            ("every day", "daily"),
            ("monthly", "monthly"),
            ("every month", "monthly"),
            ("every 3 days", "every 3 days"),
            ("every 3 months", "every 3 months"),
            ("yearly", "yearly"),
            ("annually", "yearly"),
            ("every year", "yearly"),
            ("every 2 years", "every 2 years"),
            ("fortnightly", "fortnightly"),
            ("weekdays", "weekdays"),
            ("every weekday", "weekdays"),
            ("every Friday", "weekly on fri"),
            ("Fridays", "weekly on fri"),
            ("every Monday and Thursday", "weekly on mon,thu"),
            ("every 3 weeks", "every 3 weeks"),
            (
                "every 4 weeks on Monday and Thursday",
                "every 4 weeks on mon,thu",
            ),
            (
                "every 4294967295 weeks on tue",
                "every 4294967295 weeks on tue",
            ),
            (
                "Every Monday, Wednesday, and Friday",
                "weekly on mon,wed,fri",
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
        assert_eq!(natural_rule_label(RecurrenceRule::monthly()), "Every month");
        assert_eq!(
            natural_rule_label(RecurrenceRule::every_n_days(3).unwrap()),
            "Every 3 days"
        );
        assert_eq!(
            natural_rule_label(RecurrenceRule::every_n_months(3).unwrap()),
            "Every 3 months"
        );
        assert_eq!(natural_rule_label(RecurrenceRule::yearly()), "Every year");
        assert_eq!(
            natural_rule_label(RecurrenceRule::every_n_years(2).unwrap()),
            "Every 2 years"
        );
        let every_twelve_months = RecurrenceRule::every_n_months(12).unwrap();
        let yearly = RecurrenceRule::yearly();
        assert_ne!(every_twelve_months, yearly);
        assert_eq!(natural_rule_label(every_twelve_months), "Every 12 months");
        assert_eq!(natural_rule_label(yearly), "Every year");
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

    #[test]
    fn every_natural_label_parses_to_the_same_rule() {
        let rules = [
            RecurrenceRule::daily(),
            RecurrenceRule::every_n_days(3).unwrap(),
            RecurrenceRule::monthly(),
            RecurrenceRule::every_n_months(3).unwrap(),
            RecurrenceRule::yearly(),
            RecurrenceRule::every_n_years(2).unwrap(),
            RecurrenceRule::weekdays(),
            RecurrenceRule::weekly_on([Weekday::Mon, Weekday::Wed, Weekday::Fri]).unwrap(),
            RecurrenceRule::every_n_weeks_on(4, [Weekday::Mon, Weekday::Thu]).unwrap(),
        ];
        for rule in rules {
            let label = natural_rule_label(rule);
            let canonical = canonical_rule_input(&label).unwrap().unwrap();
            let schedule = crate::commands::recurrence_schedule(
                &canonical,
                None,
                None,
                Some("UTC"),
                Some("2028-02-29"),
            )
            .unwrap();
            assert_eq!(schedule.rule, rule, "label {label}");
        }
    }
}
