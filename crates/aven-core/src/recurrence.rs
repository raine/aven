pub mod identity;
pub mod rule;
pub mod schedule;

use std::fmt;
use std::str::FromStr;

use chrono::{NaiveDate, NaiveTime};
use getrandom::fill as fill_random;
use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::{BASE32, encode_crockford};

pub use identity::{
    RecurrenceFieldVersionSeeds, RecurrenceOccurrenceIdentity, RecurrenceOccurrenceLink,
    derive_occurrence_identity,
};
pub use rule::{RecurrenceFrequency, RecurrenceRule, WeekdaySet};
pub use schedule::{
    RecurrenceScheduleError, RecurrenceSlotIter, is_slot, live_slot_on, next_slot_after,
    slot_cutoff, slot_values,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RecurrenceSeriesId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidRecurrenceSeriesId;

impl RecurrenceSeriesId {
    pub fn new() -> Self {
        let mut bytes = [0u8; 10];
        fill_random(&mut bytes).expect("fill random bytes");
        Self(encode_crockford(&bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RecurrenceSeriesId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RecurrenceSeriesId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for InvalidRecurrenceSeriesId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("recurrence series ID must be 16 Crockford Base32 characters")
    }
}

impl std::error::Error for InvalidRecurrenceSeriesId {}

impl FromStr for RecurrenceSeriesId {
    type Err = InvalidRecurrenceSeriesId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == 16 && value.bytes().all(|byte| BASE32.contains(&byte)) {
            Ok(Self(value.to_string()))
        } else {
            Err(InvalidRecurrenceSeriesId)
        }
    }
}

impl TryFrom<String> for RecurrenceSeriesId {
    type Error = InvalidRecurrenceSeriesId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl<'de> Deserialize<'de> for RecurrenceSeriesId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceDuePolicy {
    SameDay,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceSeriesState {
    Active,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceOutcome {
    Completed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct TimeZoneId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidTimeZoneId(String);

impl TimeZoneId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn timezone(&self) -> chrono_tz::Tz {
        self.0
            .parse()
            .expect("TimeZoneId always contains a chrono-tz zone")
    }
}

impl fmt::Display for TimeZoneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for InvalidTimeZoneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid IANA time zone: {}", self.0)
    }
}

impl std::error::Error for InvalidTimeZoneId {}

impl FromStr for TimeZoneId {
    type Err = InvalidTimeZoneId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value
            .parse::<chrono_tz::Tz>()
            .map(|_| Self(value.to_string()))
            .map_err(|_| InvalidTimeZoneId(value.to_string()))
    }
}

impl TryFrom<String> for TimeZoneId {
    type Error = InvalidTimeZoneId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl<'de> Deserialize<'de> for TimeZoneId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceSchedule {
    pub rule: RecurrenceRule,
    pub timezone: TimeZoneId,
    pub start_on: NaiveDate,
    pub available_local_time: Option<NaiveTime>,
    pub due_policy: RecurrenceDuePolicy,
}

impl RecurrenceSchedule {
    pub fn new(
        rule: RecurrenceRule,
        timezone: TimeZoneId,
        start_on: NaiveDate,
        available_local_time: Option<NaiveTime>,
        due_policy: RecurrenceDuePolicy,
    ) -> Self {
        Self {
            rule,
            timezone,
            start_on,
            available_local_time,
            due_policy,
        }
    }

    pub fn slots_on_or_after(&self, date: NaiveDate) -> RecurrenceSlotIter<'_> {
        RecurrenceSlotIter::new(&self.rule, self.start_on, date)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceSlot {
    pub scheduled_on: NaiveDate,
    pub boundary_at: String,
    pub available_at: String,
    pub due_on: Option<String>,
}
