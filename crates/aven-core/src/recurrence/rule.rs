use std::fmt;
use std::str::FromStr;

use chrono::{Datelike, Weekday};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl RecurrenceFrequency {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Yearly => "yearly",
        }
    }

    pub fn parse(value: &str) -> Result<Self, InvalidRecurrenceFrequency> {
        match value {
            "daily" => Ok(Self::Daily),
            "weekly" => Ok(Self::Weekly),
            "monthly" => Ok(Self::Monthly),
            "yearly" => Ok(Self::Yearly),
            _ => Err(InvalidRecurrenceFrequency(value.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidRecurrenceFrequency(String);

impl fmt::Display for InvalidRecurrenceFrequency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid recurrence frequency: {}", self.0)
    }
}

impl std::error::Error for InvalidRecurrenceFrequency {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WeekdaySet(u8);

impl WeekdaySet {
    const MONDAY: u8 = 1 << 0;
    const TUESDAY: u8 = 1 << 1;
    const WEDNESDAY: u8 = 1 << 2;
    const THURSDAY: u8 = 1 << 3;
    const FRIDAY: u8 = 1 << 4;
    const SATURDAY: u8 = 1 << 5;
    const SUNDAY: u8 = 1 << 6;

    pub fn from_weekdays(weekdays: impl IntoIterator<Item = Weekday>) -> Self {
        let mut bits = 0;
        for weekday in weekdays {
            bits |= Self::bit(weekday);
        }
        Self(bits)
    }

    pub fn weekdays() -> Self {
        Self(Self::MONDAY | Self::TUESDAY | Self::WEDNESDAY | Self::THURSDAY | Self::FRIDAY)
    }

    pub fn contains(self, weekday: Weekday) -> bool {
        self.0 & Self::bit(weekday) != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Weekday> {
        [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ]
        .into_iter()
        .filter(move |weekday| self.contains(*weekday))
    }

    const fn bit(weekday: Weekday) -> u8 {
        match weekday {
            Weekday::Mon => Self::MONDAY,
            Weekday::Tue => Self::TUESDAY,
            Weekday::Wed => Self::WEDNESDAY,
            Weekday::Thu => Self::THURSDAY,
            Weekday::Fri => Self::FRIDAY,
            Weekday::Sat => Self::SATURDAY,
            Weekday::Sun => Self::SUNDAY,
        }
    }
}

impl fmt::Display for WeekdaySet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut separator = "";
        for weekday in self.iter() {
            formatter.write_str(separator)?;
            formatter.write_str(weekday_name(weekday))?;
            separator = ",";
        }
        Ok(())
    }
}

impl FromStr for WeekdaySet {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_weekday_set(value)
    }
}

impl Serialize for WeekdaySet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for WeekdaySet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_weekday_set(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RecurrenceRule {
    frequency: RecurrenceFrequency,
    interval: u32,
    weekdays: WeekdaySet,
}

impl RecurrenceRule {
    pub fn new(
        frequency: RecurrenceFrequency,
        interval: u32,
        weekdays: WeekdaySet,
    ) -> Result<Self, InvalidRecurrenceRule> {
        if interval == 0 {
            return Err(InvalidRecurrenceRule::ZeroInterval);
        }
        match frequency {
            RecurrenceFrequency::Weekly if weekdays.is_empty() => {
                return Err(InvalidRecurrenceRule::EmptyWeekdays);
            }
            RecurrenceFrequency::Daily
            | RecurrenceFrequency::Monthly
            | RecurrenceFrequency::Yearly
                if !weekdays.is_empty() =>
            {
                return Err(InvalidRecurrenceRule::UnexpectedWeekdays);
            }
            _ => {}
        }
        Ok(Self {
            frequency,
            interval,
            weekdays,
        })
    }

    pub fn daily() -> Self {
        Self {
            frequency: RecurrenceFrequency::Daily,
            interval: 1,
            weekdays: WeekdaySet::default(),
        }
    }

    pub fn every_n_days(interval: u32) -> Result<Self, InvalidRecurrenceRule> {
        Self::new(RecurrenceFrequency::Daily, interval, WeekdaySet::default())
    }

    pub fn weekdays() -> Self {
        Self {
            frequency: RecurrenceFrequency::Weekly,
            interval: 1,
            weekdays: WeekdaySet::weekdays(),
        }
    }

    pub fn monthly() -> Self {
        Self {
            frequency: RecurrenceFrequency::Monthly,
            interval: 1,
            weekdays: WeekdaySet::default(),
        }
    }

    pub fn every_n_months(interval: u32) -> Result<Self, InvalidRecurrenceRule> {
        Self::new(
            RecurrenceFrequency::Monthly,
            interval,
            WeekdaySet::default(),
        )
    }

    pub fn yearly() -> Self {
        Self {
            frequency: RecurrenceFrequency::Yearly,
            interval: 1,
            weekdays: WeekdaySet::default(),
        }
    }

    pub fn every_n_years(interval: u32) -> Result<Self, InvalidRecurrenceRule> {
        Self::new(RecurrenceFrequency::Yearly, interval, WeekdaySet::default())
    }

    pub fn weekly(weekday: Weekday) -> Self {
        Self::weekly_on([weekday]).expect("one weekday is a valid weekly rule")
    }

    pub fn weekly_on(
        weekdays: impl IntoIterator<Item = Weekday>,
    ) -> Result<Self, InvalidRecurrenceRule> {
        Self::every_n_weeks_on(1, weekdays)
    }

    pub fn every_n_weeks_on(
        interval: u32,
        weekdays: impl IntoIterator<Item = Weekday>,
    ) -> Result<Self, InvalidRecurrenceRule> {
        Self::new(
            RecurrenceFrequency::Weekly,
            interval,
            WeekdaySet::from_weekdays(weekdays),
        )
    }

    pub fn frequency(self) -> RecurrenceFrequency {
        self.frequency
    }

    pub fn interval(self) -> u32 {
        self.interval
    }

    pub fn weekdays_set(self) -> WeekdaySet {
        self.weekdays
    }

    pub(crate) fn matches(self, start_on: chrono::NaiveDate, date: chrono::NaiveDate) -> bool {
        if date < start_on {
            return false;
        }
        match self.frequency {
            RecurrenceFrequency::Daily => {
                date.signed_duration_since(start_on).num_days() % i64::from(self.interval) == 0
            }
            RecurrenceFrequency::Weekly => {
                let days_from_anchor = i64::from(start_on.weekday().num_days_from_monday())
                    + date.signed_duration_since(start_on).num_days();
                let weeks = days_from_anchor / 7;
                weeks % i64::from(self.interval) == 0 && self.weekdays.contains(date.weekday())
            }
            RecurrenceFrequency::Monthly => {
                date.day() == start_on.day().min(last_day_of_month(date))
                    && months_between(start_on, date) % i64::from(self.interval) == 0
            }
            RecurrenceFrequency::Yearly => {
                date.month() == start_on.month()
                    && date.day() == start_on.day().min(last_day_of_month(date))
                    && i64::from(date.year() - start_on.year()) % i64::from(self.interval) == 0
            }
        }
    }
}

impl<'de> Deserialize<'de> for RecurrenceRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RuleWire {
            frequency: RecurrenceFrequency,
            interval: u32,
            weekdays: WeekdaySet,
        }

        let wire = RuleWire::deserialize(deserializer)?;
        Self::new(wire.frequency, wire.interval, wire.weekdays).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidRecurrenceRule {
    ZeroInterval,
    EmptyWeekdays,
    UnexpectedWeekdays,
}

impl fmt::Display for InvalidRecurrenceRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroInterval => "recurrence interval must be greater than zero",
            Self::EmptyWeekdays => "weekly recurrence must contain at least one weekday",
            Self::UnexpectedWeekdays => "only weekly recurrence can contain a weekday set",
        })
    }
}

impl std::error::Error for InvalidRecurrenceRule {}

fn months_between(start: chrono::NaiveDate, date: chrono::NaiveDate) -> i64 {
    (i64::from(date.year()) * 12 + i64::from(date.month0()))
        - (i64::from(start.year()) * 12 + i64::from(start.month0()))
}

fn last_day_of_month(date: chrono::NaiveDate) -> u32 {
    (28..=31)
        .rev()
        .find(|day| date.with_day(*day).is_some())
        .expect("every calendar month has at least 28 days")
}

fn parse_weekday_set(value: &str) -> Result<WeekdaySet, &'static str> {
    if value.is_empty() {
        return Ok(WeekdaySet::default());
    }
    let mut weekdays = Vec::new();
    for part in value.split(',') {
        let weekday = match part {
            "mon" => Weekday::Mon,
            "tue" => Weekday::Tue,
            "wed" => Weekday::Wed,
            "thu" => Weekday::Thu,
            "fri" => Weekday::Fri,
            "sat" => Weekday::Sat,
            "sun" => Weekday::Sun,
            _ => return Err("weekday set must use canonical comma-separated weekday names"),
        };
        if weekdays.contains(&weekday) {
            return Err("weekday set cannot contain duplicate weekdays");
        }
        weekdays.push(weekday);
    }
    let set = WeekdaySet::from_weekdays(weekdays);
    if set.to_string() != value {
        return Err("weekday set must be ordered from Monday through Sunday");
    }
    Ok(set)
}

const fn weekday_name(weekday: Weekday) -> &'static str {
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
