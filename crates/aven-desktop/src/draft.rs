//! The composer's rules, free of GPUI so they can be tested on their own.
use anyhow::{Context as _, Result, bail};
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceSchedule};
use aven_host::recurrence_input::{RecurrenceClock, RecurrenceDraft, natural_rule_label};
use aven_host::schedule_input::{ParsedScheduleInput, format_schedule_input, parse_schedule_input};

/// The schedule a recurring text describes.
pub fn recurrence_schedule(text: &str) -> Result<RecurrenceSchedule> {
    let ParsedScheduleInput::Recurring {
        rule,
        available_time,
        due_policy,
        starts_on,
    } = parse_schedule_input(text)?
    else {
        bail!("that schedule does not repeat");
    };
    let draft = RecurrenceDraft {
        template: None,
        rule: &rule,
        repeat_at: &available_time,
        repeat_due: &due_policy,
        time_zone: "",
        start_on: &starts_on,
    };
    draft
        .schedule(RecurrenceClock::system())?
        .context("that schedule does not repeat")
}

/// What the schedule field shows for dates or a recurrence set elsewhere.
pub fn schedule_text(
    available_at: &str,
    due_on: &str,
    recurrence: Option<&RecurrenceSchedule>,
) -> String {
    let Some(schedule) = recurrence else {
        return format_schedule_input(available_at, due_on, "", "", "", "");
    };
    let time = schedule
        .available_local_time
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_default();
    let due = match schedule.due_policy {
        RecurrenceDuePolicy::None => "none",
        _ => "same-day",
    };
    format_schedule_input(
        "",
        "",
        &natural_rule_label(schedule.rule),
        &time,
        due,
        &schedule.start_on.to_string(),
    )
}
