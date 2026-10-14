use anyhow::{Context, Result, anyhow, bail};
use aven_core::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceRule, RecurrenceSchedule, TimeZoneId,
    WeekdaySet,
};
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, Utc, Weekday};

/// The instant and local time zone that supply defaults for omitted recurrence
/// fields.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecurrenceClock {
    pub(crate) now: DateTime<Utc>,
    pub(crate) local_time_zone: fn() -> Result<TimeZoneId>,
}

impl RecurrenceClock {
    pub(crate) fn system() -> Self {
        Self {
            now: Utc::now(),
            local_time_zone: system_time_zone,
        }
    }

    pub(crate) fn today_in(&self, time_zone: &TimeZoneId) -> NaiveDate {
        date_in(self.now, time_zone)
    }
}

fn date_in(now: DateTime<Utc>, time_zone: &TimeZoneId) -> NaiveDate {
    let zone = time_zone
        .as_str()
        .parse::<chrono_tz::Tz>()
        .expect("core-validated time zone parses with chrono-tz");
    now.with_timezone(&zone).date_naive()
}

fn system_time_zone() -> Result<TimeZoneId> {
    let value = iana_time_zone::get_timezone().context(
        "error local-time-zone-unavailable hint=\"pass --time-zone with an IANA zone such as Europe/Stockholm\"",
    )?;
    parse_time_zone(&value)
}

/// Builds a schedule from a canonical rule. An omitted time zone defaults to
/// the clock's local zone and an omitted start date to today in that zone.
pub(crate) fn recurrence_schedule(
    rule: &str,
    repeat_at: Option<&str>,
    repeat_due: Option<&str>,
    time_zone: Option<&str>,
    start_on: Option<&str>,
    clock: RecurrenceClock,
) -> Result<RecurrenceSchedule> {
    let time_zone = time_zone.map_or_else(clock.local_time_zone, parse_time_zone)?;
    let start_on = start_on.map_or_else(|| Ok(clock.today_in(&time_zone)), parse_date)?;
    let rule = parse_rule(rule, start_on)?;
    let available_local_time = repeat_at.map(parse_repeat_time).transpose()?.flatten();
    let due_policy = parse_due_policy(repeat_due.unwrap_or("same-day"))?;
    Ok(RecurrenceSchedule::new(
        rule,
        time_zone,
        start_on,
        available_local_time,
        due_policy,
    ))
}

/// Recurrence fields as entered in an authoring form. Blank fields take their
/// defaults, and a template fixes the rule, time zone, and start date so only
/// the local time and due policy apply.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecurrenceDraft<'a> {
    pub(crate) template: Option<&'a RecurrenceSchedule>,
    pub(crate) rule: &'a str,
    pub(crate) repeat_at: &'a str,
    pub(crate) repeat_due: &'a str,
    pub(crate) time_zone: &'a str,
    pub(crate) start_on: &'a str,
}

impl RecurrenceDraft<'_> {
    /// Returns `None` when the draft has no template and no repeat rule.
    pub(crate) fn schedule(&self, clock: RecurrenceClock) -> Result<Option<RecurrenceSchedule>> {
        let repeat_at = non_blank(self.repeat_at);
        if let Some(template) = self.template {
            let available_local_time = repeat_at.map(parse_repeat_time).transpose()?.flatten();
            let due_policy = parse_due_policy(self.repeat_due)?;
            return Ok(Some(RecurrenceSchedule::new(
                template.rule,
                template.timezone.clone(),
                template.start_on,
                available_local_time,
                due_policy,
            )));
        }
        let Some(rule) = canonical_rule_input(self.rule)? else {
            return Ok(None);
        };
        recurrence_schedule(
            &rule,
            repeat_at,
            Some(self.repeat_due),
            non_blank(self.time_zone),
            non_blank(self.start_on),
            clock,
        )
        .map(Some)
    }
}

fn non_blank(value: &str) -> Option<&str> {
    Some(value.trim()).filter(|value| !value.is_empty())
}

/// The next `count` slot dates, starting from the later of the schedule start
/// and today in the schedule's time zone.
pub(crate) fn upcoming_slots(
    schedule: &RecurrenceSchedule,
    now: DateTime<Utc>,
    count: usize,
) -> Vec<NaiveDate> {
    let from = schedule.start_on.max(date_in(now, &schedule.timezone));
    schedule.slots_on_or_after(from).take(count).collect()
}

fn parse_time_zone(value: &str) -> Result<TimeZoneId> {
    value.parse().map_err(Into::into)
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").with_context(|| {
        format!(
            "error invalid-recurrence-date value={value:?} hint=\"use a real calendar date in YYYY-MM-DD form\""
        )
    })
}

pub(crate) fn parse_repeat_time(value: &str) -> Result<Option<NaiveTime>> {
    if value == "none" {
        return Ok(None);
    }
    if value.len() != 5 || value.as_bytes().get(2) != Some(&b':') {
        bail!("error invalid-repeat-at value={value:?} hint=\"use HH:MM or none\"");
    }
    NaiveTime::parse_from_str(value, "%H:%M")
        .map(Some)
        .with_context(|| {
            format!("error invalid-repeat-at value={value:?} hint=\"use a valid 24-hour time such as 09:00\"")
        })
}

pub(crate) fn parse_due_policy(value: &str) -> Result<RecurrenceDuePolicy> {
    match value {
        "same-day" => Ok(RecurrenceDuePolicy::SameDay),
        "none" => Ok(RecurrenceDuePolicy::None),
        _ => bail!("error invalid-repeat-due value={value:?} hint=\"use same-day or none\""),
    }
}

fn parse_rule(value: &str, start_on: NaiveDate) -> Result<RecurrenceRule> {
    match value {
        "daily" => return Ok(RecurrenceRule::daily()),
        "weekdays" => return Ok(RecurrenceRule::weekdays()),
        "weekly" => return Ok(RecurrenceRule::weekly(start_on.weekday())),
        "fortnightly" => {
            return RecurrenceRule::every_n_weeks_on(2, [start_on.weekday()]).map_err(Into::into);
        }
        "monthly" => return Ok(RecurrenceRule::monthly()),
        "yearly" => return Ok(RecurrenceRule::yearly()),
        _ => {}
    }
    if let Some(days) = value.strip_prefix("weekly on ") {
        let weekdays = days.parse::<WeekdaySet>().map_err(anyhow::Error::msg)?;
        return RecurrenceRule::weekly_on(weekdays.iter()).map_err(Into::into);
    }
    let words = value.split(' ').collect::<Vec<_>>();
    if let ["every", interval, unit] = words.as_slice() {
        let interval = parse_rule_interval(interval, unit)?;
        return match *unit {
            "days" => RecurrenceRule::every_n_days(interval).map_err(Into::into),
            "weeks" => {
                RecurrenceRule::every_n_weeks_on(interval, [start_on.weekday()]).map_err(Into::into)
            }
            "months" => RecurrenceRule::every_n_months(interval).map_err(Into::into),
            "years" => RecurrenceRule::every_n_years(interval).map_err(Into::into),
            _ => invalid_rule(value),
        };
    }
    if let ["every", interval, "weeks", "on", days] = words.as_slice() {
        let interval = parse_rule_interval(interval, "weeks")?;
        let weekdays = days.parse::<WeekdaySet>().map_err(anyhow::Error::msg)?;
        return RecurrenceRule::every_n_weeks_on(interval, weekdays.iter()).map_err(Into::into);
    }
    invalid_rule(value)
}

fn invalid_rule<T>(value: &str) -> Result<T> {
    bail!(
        "error invalid-repeat-rule value={value:?} hint=\"use daily, every N days, weekdays, weekly, fortnightly, monthly, every N months, yearly, every N years, weekly on mon,wed,fri, every N weeks, or every N weeks on mon,thu\""
    )
}

fn parse_rule_interval(value: &str, unit: &str) -> Result<u32> {
    let interval = value.parse::<u32>().with_context(|| {
        format!(
            "error invalid-repeat-interval value={value:?} hint=\"use a positive whole number of {unit}\""
        )
    })?;
    if interval == 0 {
        bail!(
            "error invalid-repeat-interval value={value:?} hint=\"use a positive whole number of {unit}\""
        );
    }
    Ok(interval)
}

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

    fn clock(now: &str, local_time_zone: fn() -> Result<TimeZoneId>) -> RecurrenceClock {
        RecurrenceClock {
            now: now.parse().unwrap(),
            local_time_zone,
        }
    }

    fn auckland() -> Result<TimeZoneId> {
        parse_time_zone("Pacific/Auckland")
    }

    fn unavailable() -> Result<TimeZoneId> {
        bail!("error local-time-zone-unavailable")
    }

    fn date(value: &str) -> NaiveDate {
        value.parse().unwrap()
    }

    fn slots(rule: &str, start_on: &str, now: &str, count: usize) -> Vec<NaiveDate> {
        let schedule = recurrence_schedule(
            rule,
            None,
            None,
            Some("UTC"),
            Some(start_on),
            clock(now, unavailable),
        )
        .unwrap();
        upcoming_slots(&schedule, now.parse().unwrap(), count)
    }

    #[test]
    fn parses_the_fixed_rule_grammar() {
        let monday = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        use Weekday::{Fri, Mon, Thu, Tue, Wed};
        let weeks = |n, days: &[Weekday]| {
            RecurrenceRule::every_n_weeks_on(n, days.iter().copied()).unwrap()
        };
        for (input, expected) in [
            ("daily", RecurrenceRule::daily()),
            ("every 1 days", RecurrenceRule::daily()),
            ("every 3 days", RecurrenceRule::every_n_days(3).unwrap()),
            ("weekdays", RecurrenceRule::weekdays()),
            ("weekly", RecurrenceRule::weekly(Mon)),
            ("fortnightly", weeks(2, &[Mon])),
            ("monthly", RecurrenceRule::monthly()),
            ("every 1 months", RecurrenceRule::monthly()),
            ("every 3 months", RecurrenceRule::every_n_months(3).unwrap()),
            ("yearly", RecurrenceRule::yearly()),
            ("every 1 years", RecurrenceRule::yearly()),
            ("every 2 years", RecurrenceRule::every_n_years(2).unwrap()),
            ("weekly on mon,wed,fri", weeks(1, &[Mon, Wed, Fri])),
            ("every 2 weeks", weeks(2, &[Mon])),
            ("every 3 weeks", weeks(3, &[Mon])),
            ("every 2 weeks on tue", weeks(2, &[Tue])),
            ("every 3 weeks on mon,thu", weeks(3, &[Mon, Thu])),
        ] {
            assert_eq!(parse_rule(input, monday).unwrap(), expected, "{input}");
        }
        for rule in [
            "every 0 days",
            "every 0 weeks",
            "every 0 months",
            "every 0 years",
            "every -1 days",
            "every -1 months",
            "every nope years",
            "every 4294967296 days",
            "every 3 bananas",
            "every nope months",
            "every 4294967296 years",
            "every 3 days on mon",
            "weekly on monday",
            "weekly on fri,mon",
            "every two weeks on tue",
            "daily ",
        ] {
            assert!(parse_rule(rule, monday).is_err(), "{rule}");
        }
    }

    #[test]
    fn omitted_zone_and_start_come_from_the_clock() {
        // 23:30 UTC on March 1 is already March 2 in Auckland.
        let clock = clock("2026-03-01T23:30:00Z", auckland);
        let local = recurrence_schedule("weekly", None, None, None, None, clock).unwrap();
        assert_eq!(local.timezone.as_str(), "Pacific/Auckland");
        assert_eq!(local.start_on, date("2026-03-02"));
        assert_eq!(local.rule, RecurrenceRule::weekly(Weekday::Mon));

        let utc = recurrence_schedule("weekly", None, None, Some("UTC"), None, clock).unwrap();
        assert_eq!(utc.start_on, date("2026-03-01"));
        assert_eq!(utc.rule, RecurrenceRule::weekly(Weekday::Sun));
    }

    #[test]
    fn local_zone_is_consulted_only_when_the_zone_is_omitted() {
        let clock = clock("2026-03-01T12:00:00Z", unavailable);
        assert!(recurrence_schedule("daily", None, None, Some("UTC"), None, clock).is_ok());
        let error = recurrence_schedule("daily", None, None, None, None, clock).unwrap_err();
        assert!(error.to_string().contains("local-time-zone-unavailable"));
    }

    #[test]
    fn schedule_input_errors_keep_their_diagnostics() {
        let clock = clock("2026-03-01T12:00:00Z", unavailable);
        for (args, code) in [
            ((Some("9am"), None, None), "invalid-repeat-at"),
            ((None, Some("later"), None), "invalid-repeat-due"),
            ((None, None, Some("2027-02-29")), "invalid-recurrence-date"),
        ] {
            let (repeat_at, repeat_due, start_on) = args;
            let error =
                recurrence_schedule("daily", repeat_at, repeat_due, Some("UTC"), start_on, clock)
                    .unwrap_err();
            assert!(format!("{error:#}").contains(code), "{error:#}");
        }
    }

    #[test]
    fn draft_blank_fields_take_defaults() {
        let clock = clock("2026-03-01T23:30:00Z", auckland);
        let draft = RecurrenceDraft {
            template: None,
            rule: "every Friday",
            repeat_at: " ",
            repeat_due: "same-day",
            time_zone: "",
            start_on: "",
        };
        let schedule = draft.schedule(clock).unwrap().unwrap();
        assert_eq!(schedule.timezone.as_str(), "Pacific/Auckland");
        assert_eq!(schedule.start_on, date("2026-03-02"));
        assert_eq!(schedule.available_local_time, None);

        for rule in ["", "  ", "none"] {
            let draft = RecurrenceDraft { rule, ..draft };
            assert_eq!(draft.schedule(clock).unwrap(), None);
        }
    }

    #[test]
    fn template_draft_keeps_rule_zone_and_start() {
        let template = RecurrenceSchedule::new(
            RecurrenceRule::monthly(),
            parse_time_zone("Europe/Helsinki").unwrap(),
            date("2028-01-31"),
            None,
            RecurrenceDuePolicy::SameDay,
        );
        let draft = RecurrenceDraft {
            template: Some(&template),
            rule: "daily",
            repeat_at: "07:15",
            repeat_due: "none",
            time_zone: "UTC",
            start_on: "2030-01-01",
        };
        let schedule = draft
            .schedule(clock("2026-03-01T12:00:00Z", unavailable))
            .unwrap()
            .unwrap();
        assert_eq!(schedule.rule, template.rule);
        assert_eq!(schedule.timezone, template.timezone);
        assert_eq!(schedule.start_on, template.start_on);
        assert_eq!(
            schedule.available_local_time,
            NaiveTime::from_hms_opt(7, 15, 0)
        );
        assert_eq!(schedule.due_policy, RecurrenceDuePolicy::None);

        let invalid = RecurrenceDraft {
            repeat_at: "25:00",
            ..draft
        };
        let error = invalid
            .schedule(clock("2026-03-01T12:00:00Z", unavailable))
            .unwrap_err();
        assert!(format!("{error:#}").contains("invalid-repeat-at"));
    }

    #[test]
    fn upcoming_slots_start_from_the_later_of_start_and_today() {
        assert_eq!(
            slots("daily", "2026-05-10", "2026-05-01T12:00:00Z", 2),
            [date("2026-05-10"), date("2026-05-11")]
        );
        assert_eq!(
            slots("daily", "2026-04-01", "2026-05-01T12:00:00Z", 2),
            [date("2026-05-01"), date("2026-05-02")]
        );
    }

    #[test]
    fn upcoming_slots_use_the_schedule_zone_across_dst_changes() {
        // Helsinki moves from UTC+2 to UTC+3 at 01:00 UTC on 2026-03-29.
        let schedule = recurrence_schedule(
            "daily",
            Some("03:30"),
            None,
            Some("Europe/Helsinki"),
            Some("2026-03-01"),
            clock("2026-03-01T00:00:00Z", unavailable),
        )
        .unwrap();
        assert_eq!(
            upcoming_slots(&schedule, "2026-03-28T21:59:00Z".parse().unwrap(), 2),
            [date("2026-03-28"), date("2026-03-29")]
        );
        assert_eq!(
            upcoming_slots(&schedule, "2026-03-28T22:00:00Z".parse().unwrap(), 2),
            [date("2026-03-29"), date("2026-03-30")]
        );
        assert_eq!(
            upcoming_slots(&schedule, "2026-10-24T21:00:00Z".parse().unwrap(), 1),
            [date("2026-10-25")]
        );
    }

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
            let schedule = recurrence_schedule(
                &canonical,
                None,
                None,
                Some("UTC"),
                Some("2028-02-29"),
                RecurrenceClock::system(),
            )
            .unwrap();
            assert_eq!(schedule.rule, rule, "label {label}");
        }
    }
}
