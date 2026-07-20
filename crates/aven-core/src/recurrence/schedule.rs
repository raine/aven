use std::fmt;

use chrono::{DateTime, Days, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::{GapInfo, Tz};

use super::rule::RecurrenceRule;
use super::{RecurrenceDuePolicy, RecurrenceSchedule, RecurrenceSlot};

pub fn is_slot(rule: &RecurrenceRule, start_on: NaiveDate, date: NaiveDate) -> bool {
    rule.matches(start_on, date)
}

pub fn next_slot_after(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<NaiveDate> {
    let next = date.checked_add_days(Days::new(1))?;
    RecurrenceSlotIter::new(rule, start_on, next).next()
}

pub fn live_slot_on(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    now: DateTime<Utc>,
    timezone: &super::TimeZoneId,
) -> Option<NaiveDate> {
    let zone = timezone.timezone();
    let mut date = now.with_timezone(&zone).date_naive();
    while date >= start_on {
        if is_slot(rule, start_on, date)
            && resolve_local(zone, date.and_time(NaiveTime::MIN)).ok()? <= now
        {
            return Some(date);
        }
        date = date.checked_sub_days(Days::new(1))?;
    }
    None
}

pub fn slot_cutoff(
    schedule: &RecurrenceSchedule,
    slot_on: NaiveDate,
) -> Result<DateTime<Utc>, RecurrenceScheduleError> {
    require_slot(schedule, slot_on)?;
    let next = next_slot_after(&schedule.rule, schedule.start_on, slot_on)
        .ok_or(RecurrenceScheduleError::DateOverflow)?;
    resolve_local(schedule.timezone.timezone(), next.and_time(NaiveTime::MIN))
}

pub fn slot_values(
    schedule: &RecurrenceSchedule,
    slot_on: NaiveDate,
) -> Result<RecurrenceSlot, RecurrenceScheduleError> {
    require_slot(schedule, slot_on)?;
    let zone = schedule.timezone.timezone();
    let boundary = resolve_local(zone, slot_on.and_time(NaiveTime::MIN))?;
    let available_local_time = schedule.available_local_time.unwrap_or(NaiveTime::MIN);
    let available_at = resolve_local(zone, slot_on.and_time(available_local_time))?;
    let due_on = match schedule.due_policy {
        RecurrenceDuePolicy::SameDay => Some(slot_on.format("%Y-%m-%d").to_string()),
        RecurrenceDuePolicy::None => None,
    };
    Ok(RecurrenceSlot {
        scheduled_on: slot_on,
        boundary_at: format_utc(boundary),
        available_at: format_utc(available_at),
        due_on,
    })
}

#[derive(Debug, Clone)]
pub struct RecurrenceSlotIter<'a> {
    rule: &'a RecurrenceRule,
    start_on: NaiveDate,
    candidate: Option<NaiveDate>,
}

impl<'a> RecurrenceSlotIter<'a> {
    pub(crate) fn new(
        rule: &'a RecurrenceRule,
        start_on: NaiveDate,
        on_or_after: NaiveDate,
    ) -> Self {
        Self {
            rule,
            start_on,
            candidate: Some(on_or_after.max(start_on)),
        }
    }
}

impl Iterator for RecurrenceSlotIter<'_> {
    type Item = NaiveDate;

    fn next(&mut self) -> Option<Self::Item> {
        let mut candidate = self.candidate?;
        loop {
            let following = candidate.checked_add_days(Days::new(1));
            if is_slot(self.rule, self.start_on, candidate) {
                self.candidate = following;
                return Some(candidate);
            }
            candidate = following?;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceScheduleError {
    NotScheduledSlot,
    DateOverflow,
    UnresolvableLocalTime,
}

impl fmt::Display for RecurrenceScheduleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotScheduledSlot => "date is not a slot in the recurrence schedule",
            Self::DateOverflow => "recurrence schedule exceeds the supported date range",
            Self::UnresolvableLocalTime => {
                "recurrence local time is outside the time-zone rule range"
            }
        })
    }
}

impl std::error::Error for RecurrenceScheduleError {}

fn require_slot(
    schedule: &RecurrenceSchedule,
    slot_on: NaiveDate,
) -> Result<(), RecurrenceScheduleError> {
    if is_slot(&schedule.rule, schedule.start_on, slot_on) {
        Ok(())
    } else {
        Err(RecurrenceScheduleError::NotScheduledSlot)
    }
}

fn resolve_local(zone: Tz, local: NaiveDateTime) -> Result<DateTime<Utc>, RecurrenceScheduleError> {
    let zoned = match zone.from_local_datetime(&local) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => GapInfo::new(&local, &zone)
            .and_then(|gap| gap.end)
            .ok_or(RecurrenceScheduleError::UnresolvableLocalTime)?,
    };
    Ok(zoned.with_timezone(&Utc))
}

fn format_utc(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
