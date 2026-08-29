use std::fmt;

use chrono::{
    DateTime, Datelike, Days, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
};
use chrono_tz::{GapInfo, Tz};

use super::rule::{RecurrenceFrequency, RecurrenceRule, last_day_of_month};
use super::{RecurrenceDuePolicy, RecurrenceSchedule, RecurrenceSlot};

pub fn is_slot(rule: &RecurrenceRule, start_on: NaiveDate, date: NaiveDate) -> bool {
    rule.matches(start_on, date)
}

pub fn next_slot_after(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<NaiveDate> {
    slot_on_or_after(rule, start_on, date.checked_add_days(Days::new(1))?)
}

pub fn live_slot_on(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    now: DateTime<Utc>,
    timezone: &super::TimeZoneId,
) -> Option<NaiveDate> {
    let zone = timezone.timezone();
    let local_today = now.with_timezone(&zone).date_naive();
    let mut slot = slot_on_or_before(rule, start_on, local_today)?;
    loop {
        if resolve_local(zone, slot.and_time(NaiveTime::MIN)).ok()? <= now {
            return Some(slot);
        }
        let before = slot.checked_sub_days(Days::new(1))?;
        slot = slot_on_or_before(rule, start_on, before)?;
    }
}

pub fn projection_slot_at(
    schedule: &RecurrenceSchedule,
    at: DateTime<Utc>,
) -> Result<NaiveDate, RecurrenceScheduleError> {
    if let Some(slot) = live_slot_on(&schedule.rule, schedule.start_on, at, &schedule.timezone) {
        return Ok(slot);
    }
    schedule
        .slots_on_or_after(schedule.start_on)
        .next()
        .ok_or(RecurrenceScheduleError::DateOverflow)
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
        let slot = slot_on_or_after(self.rule, self.start_on, self.candidate?)?;
        self.candidate = slot.checked_add_days(Days::new(1));
        Some(slot)
    }
}

pub(crate) fn slot_rank(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    slot_on: NaiveDate,
) -> Option<u64> {
    if !rule.matches(start_on, slot_on) {
        return None;
    }
    slot_count_before(rule, start_on, slot_on)
}

pub(crate) fn slot_count_before(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<u64> {
    if date <= start_on {
        return Some(0);
    }
    match rule.frequency() {
        RecurrenceFrequency::Daily => {
            let elapsed = date.signed_duration_since(start_on).num_days();
            let interval = i64::from(rule.interval());
            u64::try_from(elapsed.checked_add(interval - 1)?.checked_div(interval)?).ok()
        }
        RecurrenceFrequency::Weekly => weekly_slot_count_before(rule, start_on, date),
        RecurrenceFrequency::Monthly => {
            monthly_slot_count_before(i64::from(rule.interval()), start_on, date)
        }
        RecurrenceFrequency::Yearly => {
            monthly_slot_count_before(i64::from(rule.interval()) * 12, start_on, date)
        }
    }
}

pub(crate) fn slot_at_rank(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    rank: u64,
) -> Option<NaiveDate> {
    match rule.frequency() {
        RecurrenceFrequency::Daily => {
            let days = rank.checked_mul(u64::from(rule.interval()))?;
            start_on.checked_add_days(Days::new(days))
        }
        RecurrenceFrequency::Weekly => weekly_slot_at_rank(rule, start_on, rank),
        RecurrenceFrequency::Monthly => {
            monthly_slot_at_rank(i64::from(rule.interval()), start_on, rank)
        }
        RecurrenceFrequency::Yearly => {
            monthly_slot_at_rank(i64::from(rule.interval()) * 12, start_on, rank)
        }
    }
}

pub(crate) fn slot_on_or_after(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<NaiveDate> {
    let target = date.max(start_on);
    match rule.frequency() {
        RecurrenceFrequency::Daily => daily_slot_on_or_after(rule.interval(), start_on, target),
        RecurrenceFrequency::Weekly => weekly_slot_on_or_after(rule, start_on, target),
        RecurrenceFrequency::Monthly => {
            month_slot_on_or_after(i64::from(rule.interval()), start_on, target)
        }
        RecurrenceFrequency::Yearly => {
            month_slot_on_or_after(i64::from(rule.interval()) * 12, start_on, target)
        }
    }
}

pub(crate) fn slot_on_or_before(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<NaiveDate> {
    if date < start_on {
        return None;
    }
    match rule.frequency() {
        RecurrenceFrequency::Daily => daily_slot_on_or_before(rule.interval(), start_on, date),
        RecurrenceFrequency::Weekly => weekly_slot_on_or_before(rule, start_on, date),
        RecurrenceFrequency::Monthly => {
            month_slot_on_or_before(i64::from(rule.interval()), start_on, date)
        }
        RecurrenceFrequency::Yearly => {
            month_slot_on_or_before(i64::from(rule.interval()) * 12, start_on, date)
        }
    }
}

fn daily_slot_on_or_after(
    interval: u32,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<NaiveDate> {
    let elapsed = date.signed_duration_since(start_on).num_days();
    let interval = i64::from(interval);
    let periods = elapsed.checked_add(interval - 1)?.checked_div(interval)?;
    add_days(start_on, periods.checked_mul(interval)?)
}

fn daily_slot_on_or_before(
    interval: u32,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<NaiveDate> {
    let elapsed = date.signed_duration_since(start_on).num_days();
    let interval = i64::from(interval);
    add_days(
        start_on,
        elapsed.checked_div(interval)?.checked_mul(interval)?,
    )
}

fn month_slot_on_or_after(period: i64, start_on: NaiveDate, date: NaiveDate) -> Option<NaiveDate> {
    let difference = month_ordinal(date).checked_sub(month_ordinal(start_on))?;
    let mut index = difference.checked_div(period)?;
    let mut slot = month_slot(start_on, period, index)?;
    if slot < date {
        index = index.checked_add(1)?;
        slot = month_slot(start_on, period, index)?;
    }
    Some(slot)
}

fn monthly_slot_count_before(period: i64, start_on: NaiveDate, date: NaiveDate) -> Option<u64> {
    let difference = month_ordinal(date).checked_sub(month_ordinal(start_on))?;
    let index = difference.checked_div(period)?;
    let slot = month_slot(start_on, period, index)?;
    let count = if slot < date {
        index.checked_add(1)?
    } else {
        index
    };
    u64::try_from(count).ok()
}

fn monthly_slot_at_rank(period: i64, start_on: NaiveDate, rank: u64) -> Option<NaiveDate> {
    month_slot(start_on, period, i64::try_from(rank).ok()?)
}

fn month_slot_on_or_before(period: i64, start_on: NaiveDate, date: NaiveDate) -> Option<NaiveDate> {
    let difference = month_ordinal(date).checked_sub(month_ordinal(start_on))?;
    let mut index = difference.checked_div(period)?;
    let mut slot = month_slot(start_on, period, index)?;
    if slot > date {
        index = index.checked_sub(1)?;
        if index < 0 {
            return None;
        }
        slot = month_slot(start_on, period, index)?;
    }
    Some(slot)
}

fn month_slot(start_on: NaiveDate, period: i64, index: i64) -> Option<NaiveDate> {
    let ordinal = month_ordinal(start_on).checked_add(period.checked_mul(index)?)?;
    let year = i32::try_from(ordinal.div_euclid(12)).ok()?;
    let month = u32::try_from(ordinal.rem_euclid(12) + 1).ok()?;
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    NaiveDate::from_ymd_opt(year, month, start_on.day().min(last_day_of_month(first)))
}

fn weekly_slot_on_or_after(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<NaiveDate> {
    let anchor_monday = monday_of(start_on)?;
    let target_monday = monday_of(date)?;
    let target_week = target_monday
        .signed_duration_since(anchor_monday)
        .num_days()
        .checked_div(7)?;
    let interval = i64::from(rule.interval());
    let mut block = target_week.checked_div(interval)?;
    loop {
        let monday = add_weeks(anchor_monday, block.checked_mul(interval)?)?;
        for weekday in rule.weekdays_set().iter() {
            let candidate = add_days(monday, i64::from(weekday.num_days_from_monday()))?;
            if candidate >= start_on && candidate >= date {
                return Some(candidate);
            }
        }
        block = block.checked_add(1)?;
    }
}

fn weekly_slot_count_before(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<u64> {
    let anchor_monday = monday_of(start_on)?;
    let target_monday = monday_of(date)?;
    let target_week = target_monday
        .signed_duration_since(anchor_monday)
        .num_days()
        .checked_div(7)?;
    let weekdays = rule.weekdays_set();
    let weekday_count = i64::try_from(weekdays.iter().count()).ok()?;
    let first_block_excluded = i64::try_from(
        weekdays
            .iter()
            .filter(|weekday| {
                add_days(anchor_monday, i64::from(weekday.num_days_from_monday()))
                    .is_some_and(|candidate| candidate < start_on)
            })
            .count(),
    )
    .ok()?;
    let interval = i64::from(rule.interval());
    let block = target_week.checked_div(interval)?;
    let complete_blocks = if target_week % interval == 0 {
        block
    } else {
        block.checked_add(1)?
    };
    let complete =
        complete_blocks
            .checked_mul(weekday_count)?
            .checked_sub(if complete_blocks == 0 {
                0
            } else {
                first_block_excluded
            })?;
    let current = if target_week % interval == 0 {
        weekdays
            .iter()
            .filter(|weekday| {
                add_days(target_monday, i64::from(weekday.num_days_from_monday()))
                    .is_some_and(|candidate| candidate >= start_on && candidate < date)
            })
            .count() as i64
    } else {
        0
    };
    u64::try_from(complete.checked_add(current)?).ok()
}

fn weekly_slot_at_rank(rule: &RecurrenceRule, start_on: NaiveDate, rank: u64) -> Option<NaiveDate> {
    let anchor_monday = monday_of(start_on)?;
    let weekdays = rule.weekdays_set().iter().collect::<Vec<_>>();
    let first_block = weekdays
        .iter()
        .filter_map(|weekday| {
            add_days(anchor_monday, i64::from(weekday.num_days_from_monday()))
                .filter(|candidate| *candidate >= start_on)
        })
        .collect::<Vec<_>>();
    let first_count = u64::try_from(first_block.len()).ok()?;
    let (block, weekday_index) = if rank < first_count {
        return first_block.get(usize::try_from(rank).ok()?).copied();
    } else {
        let remaining = rank.checked_sub(first_count)?;
        let weekday_count = u64::try_from(weekdays.len()).ok()?;
        let block = 1u64.checked_add(remaining.checked_div(weekday_count)?)?;
        (block, remaining % weekday_count)
    };
    let monday = add_weeks(
        anchor_monday,
        i64::try_from(block.checked_mul(u64::from(rule.interval()))?).ok()?,
    )?;
    add_days(
        monday,
        i64::from(weekdays[usize::try_from(weekday_index).ok()?].num_days_from_monday()),
    )
}

fn weekly_slot_on_or_before(
    rule: &RecurrenceRule,
    start_on: NaiveDate,
    date: NaiveDate,
) -> Option<NaiveDate> {
    let anchor_monday = monday_of(start_on)?;
    let target_monday = monday_of(date)?;
    let target_week = target_monday
        .signed_duration_since(anchor_monday)
        .num_days()
        .checked_div(7)?;
    let interval = i64::from(rule.interval());
    let mut block = target_week.checked_div(interval)?;
    loop {
        let monday = add_weeks(anchor_monday, block.checked_mul(interval)?)?;
        for weekday in rule.weekdays_set().iter().rev() {
            if let Some(candidate) = add_days(monday, i64::from(weekday.num_days_from_monday()))
                && candidate >= start_on
                && candidate <= date
            {
                return Some(candidate);
            }
        }
        block = block.checked_sub(1)?;
        if block < 0 {
            return None;
        }
    }
}

fn month_ordinal(date: NaiveDate) -> i64 {
    i64::from(date.year()) * 12 + i64::from(date.month0())
}

fn monday_of(date: NaiveDate) -> Option<NaiveDate> {
    date.checked_sub_days(Days::new(u64::from(date.weekday().num_days_from_monday())))
}

fn add_weeks(date: NaiveDate, weeks: i64) -> Option<NaiveDate> {
    add_days(date, weeks.checked_mul(7)?)
}

fn add_days(date: NaiveDate, days: i64) -> Option<NaiveDate> {
    date.checked_add_days(Days::new(u64::try_from(days).ok()?))
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
