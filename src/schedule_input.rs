use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParsedScheduleInput {
    None,
    Once {
        available_at: String,
        due_on: String,
    },
    Recurring {
        rule: String,
        available_time: String,
        due_policy: String,
        starts_on: String,
    },
}

pub(crate) fn parse_schedule_input(input: &str) -> Result<ParsedScheduleInput> {
    let input = input.trim();
    if input.is_empty() || input.eq_ignore_ascii_case("none") {
        return Ok(ParsedScheduleInput::None);
    }

    let clauses = input.split(',').map(str::trim).collect::<Vec<_>>();
    if recurrence_intent(input) {
        let mut rule_clauses = clauses.as_slice();
        let mut due_policy = "same-day".to_string();
        let mut starts_on = String::new();
        while let Some((last, rest)) = rule_clauses.split_last() {
            if last.eq_ignore_ascii_case("due same day") {
                due_policy = "same-day".to_string();
            } else if last.eq_ignore_ascii_case("due none") || last.eq_ignore_ascii_case("no due") {
                due_policy = "none".to_string();
            } else if let Some(value) = last.strip_prefix("starting ") {
                starts_on = value.trim().to_string();
            } else {
                break;
            }
            rule_clauses = rest;
        }
        let rule_and_time = rule_clauses.join(", ");
        let (rule_input, available_time) = rule_and_time
            .rsplit_once(" at ")
            .map_or((rule_and_time.as_str(), ""), |(rule, time)| {
                (rule.trim(), time.trim())
            });
        let rule = crate::recurrence_input::canonical_rule_input(rule_input)?
            .ok_or_else(|| anyhow::anyhow!(crate::recurrence_input::rule_guidance()))?;
        crate::commands::recurrence_schedule(
            &rule,
            (!available_time.is_empty()).then_some(available_time),
            Some(&due_policy),
            None,
            (!starts_on.is_empty()).then_some(starts_on.as_str()),
        )?;
        return Ok(ParsedScheduleInput::Recurring {
            rule: rule_input.to_string(),
            available_time: available_time.to_string(),
            due_policy,
            starts_on,
        });
    }

    let mut available_at = String::new();
    let mut due_on = String::new();
    for (index, clause) in clauses.iter().enumerate() {
        if let Some(value) = clause.strip_prefix("available ") {
            available_at = value.trim().to_string();
        } else if let Some(value) = clause.strip_prefix("due ") {
            due_on = value.trim().to_string();
        } else if index == 0 {
            available_at = (*clause).to_string();
        } else {
            bail!(schedule_guidance());
        }
    }
    if !available_at.is_empty() {
        crate::time_input::parse_available_at_input(&available_at)?;
    }
    if !due_on.is_empty() {
        crate::time_input::parse_due_on_input(&due_on)?;
    }
    Ok(ParsedScheduleInput::Once {
        available_at,
        due_on,
    })
}

pub(crate) fn format_schedule_input(
    available_at: &str,
    due_on: &str,
    repeat_rule: &str,
    repeat_at: &str,
    repeat_due: &str,
    repeat_start_on: &str,
) -> String {
    if !matches!(repeat_rule.trim(), "" | "none") {
        let mut value = repeat_rule.trim().to_string();
        if !repeat_at.trim().is_empty() {
            value.push_str(" at ");
            value.push_str(repeat_at.trim());
        }
        value.push_str(if repeat_due == "none" {
            ", no due"
        } else {
            ", due same day"
        });
        if !repeat_start_on.trim().is_empty()
            && repeat_start_on.trim() != chrono::Local::now().date_naive().to_string()
        {
            value.push_str(", starting ");
            value.push_str(repeat_start_on.trim());
        }
        return value;
    }

    let mut parts = Vec::new();
    if !available_at.trim().is_empty() {
        parts.push(format!("available {}", available_at.trim()));
    }
    if !due_on.trim().is_empty() {
        parts.push(format!("due {}", due_on.trim()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(", ")
    }
}

pub(crate) fn recurrence_intent(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    let weekday = normalized
        .split_once(" at ")
        .map_or(normalized.as_str(), |(value, _)| value)
        .strip_suffix('s')
        .unwrap_or(&normalized);
    normalized.starts_with("every ")
        || normalized.starts_with("annual")
        || matches!(
            normalized.as_str(),
            "daily" | "weekdays" | "weekly" | "fortnightly" | "monthly" | "yearly" | "annually"
        )
        || matches!(
            weekday,
            "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday"
        )
}

pub(crate) const fn schedule_guidance() -> &'static str {
    "Try tomorrow, available tomorrow at 9am, due next Friday, every Friday at 09:00, or every 3 days at 09:00"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_off_and_recurring_schedules() {
        assert!(matches!(
            parse_schedule_input("available tomorrow, due next Friday").unwrap(),
            ParsedScheduleInput::Once { available_at, due_on }
                if available_at == "tomorrow" && due_on == "next Friday"
        ));
        assert!(matches!(
            parse_schedule_input("every Friday at 09:00, due same day").unwrap(),
            ParsedScheduleInput::Recurring { rule, available_time, due_policy, .. }
                if rule == "every Friday"
                    && available_time == "09:00"
                    && due_policy == "same-day"
        ));
        assert!(matches!(
            parse_schedule_input("every 3 days at 09:00, due same day").unwrap(),
            ParsedScheduleInput::Recurring { rule, available_time, .. }
                if rule == "every 3 days" && available_time == "09:00"
        ));
        assert!(matches!(
            parse_schedule_input("Every Monday, Wednesday, and Friday").unwrap(),
            ParsedScheduleInput::Recurring { rule, .. }
                if rule == "Every Monday, Wednesday, and Friday"
        ));
        assert!(matches!(
            parse_schedule_input("Fridays at 09:00").unwrap(),
            ParsedScheduleInput::Recurring { rule, available_time, .. }
                if rule == "Fridays" && available_time == "09:00"
        ));
        for input in [
            "every 3 day at 09:00",
            "every 0 days",
            "every 3 days on Monday",
            "every 3 months on Friday",
            "annually-ish",
        ] {
            let error = parse_schedule_input(input).unwrap_err().to_string();
            assert!(error.contains("Try daily"), "{input}: {error}");
        }
    }

    #[test]
    fn formats_structured_schedules_as_editable_text() {
        assert_eq!(format_schedule_input("", "", "", "", "same-day", ""), "");
        assert_eq!(
            format_schedule_input("tomorrow", "next Friday", "", "", "same-day", ""),
            "available tomorrow, due next Friday"
        );
        assert_eq!(
            format_schedule_input("", "", "daily", "09:00", "none", "2000-01-01"),
            "daily at 09:00, no due, starting 2000-01-01"
        );
    }
}
