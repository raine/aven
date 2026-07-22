pub mod identity;
pub mod rule;
pub mod schedule;

use std::fmt;
use std::str::FromStr;

use chrono::{NaiveDate, NaiveTime};
use getrandom::fill as fill_random;
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::database::Database;
use sqlx::decode::Decode;
use sqlx::encode::{Encode, IsNull};
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteTypeInfo, SqliteValueRef};
use sqlx::types::Type;

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

pub(crate) fn recurrence_series_display_ref(
    series_id: &RecurrenceSeriesId,
    ids: &[RecurrenceSeriesId],
) -> String {
    let id = series_id.as_str();
    let shared = ids
        .iter()
        .filter(|candidate| candidate.as_str() != id)
        .map(|candidate| {
            id.bytes()
                .zip(candidate.as_str().bytes())
                .take_while(|(left, right)| left == right)
                .count()
        })
        .max()
        .unwrap_or(0);
    let length = 4.max(shared.saturating_add(1)).min(id.len());
    format!("RCR-{}", &id[..length])
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

impl Type<Sqlite> for RecurrenceSeriesId {
    fn type_info() -> SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }
}

impl Encode<'_, Sqlite> for RecurrenceSeriesId {
    fn encode_by_ref(
        &self,
        buffer: &mut <Sqlite as Database>::ArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <String as Encode<Sqlite>>::encode_by_ref(&self.0, buffer)
    }
}

impl<'row> Decode<'row, Sqlite> for RecurrenceSeriesId {
    fn decode(value: SqliteValueRef<'row>) -> Result<Self, BoxDynError> {
        String::decode(value)?.parse().map_err(Into::into)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceDuePolicy {
    SameDay,
    None,
}

impl RecurrenceDuePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SameDay => "same_day",
            Self::None => "none",
        }
    }

    pub fn parse(value: &str) -> Result<Self, InvalidRecurrenceValue> {
        match value {
            "same_day" => Ok(Self::SameDay),
            "none" => Ok(Self::None),
            _ => Err(InvalidRecurrenceValue::new("due policy", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceSeriesState {
    Active,
    Paused,
    Stopped,
}

impl RecurrenceSeriesState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }

    pub fn parse(value: &str) -> Result<Self, InvalidRecurrenceValue> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "stopped" => Ok(Self::Stopped),
            _ => Err(InvalidRecurrenceValue::new("series state", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceOutcome {
    Completed,
    Skipped,
}

impl RecurrenceOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(value: &str) -> Result<Self, InvalidRecurrenceValue> {
        match value {
            "completed" => Ok(Self::Completed),
            "skipped" => Ok(Self::Skipped),
            _ => Err(InvalidRecurrenceValue::new("outcome", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceProjectionState {
    Projected,
    Resolved,
    Archived,
    Corrected,
}

impl RecurrenceProjectionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Projected => "projected",
            Self::Resolved => "resolved",
            Self::Archived => "archived",
            Self::Corrected => "corrected",
        }
    }

    pub fn parse(value: &str) -> Result<Self, InvalidRecurrenceValue> {
        match value {
            "projected" => Ok(Self::Projected),
            "resolved" => Ok(Self::Resolved),
            "archived" => Ok(Self::Archived),
            "corrected" => Ok(Self::Corrected),
            _ => Err(InvalidRecurrenceValue::new("projection state", value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidRecurrenceValue {
    kind: &'static str,
    value: String,
}

impl InvalidRecurrenceValue {
    fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_string(),
        }
    }
}

impl fmt::Display for InvalidRecurrenceValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid recurrence {}: {}",
            self.kind, self.value
        )
    }
}

impl std::error::Error for InvalidRecurrenceValue {}

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
