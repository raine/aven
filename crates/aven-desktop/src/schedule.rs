//! Setting when a task becomes available and when it is due: a menu of likely
//! dates that also reads typed ones, with a calendar for the rest. A date saves
//! as soon as it is chosen.
use std::rc::Rc;

use accesskit::Role;
use aven_core::ids::TaskId;
use aven_host::time_input::{parse_available_at_input, parse_due_on_input};
use chrono::{
    DateTime, Datelike, Days, Local, Month, Months, NaiveDate, NaiveDateTime, NaiveTime, Utc,
    Weekday,
};
use gpui::{AnyElement, Context, FontWeight, Window, div, prelude::*};

use crate::actions::*;
use crate::adapter::{Bulk, Command};
use crate::kit::*;
use crate::overlays::{Entry, MenuHeader, MenuOptions, Read};
use crate::shell::Root;
use crate::theme::Theme;

pub const CALENDAR_HEIGHT: f32 = 248.;
const CALENDAR_ROW: f32 = 30.;
const WEEK_DAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
const HIDDEN_PAST_DEADLINE: &str = "Available after its deadline, so it won't appear in Overdue.";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DateField {
    Available,
    Due,
}

impl DateField {
    fn other(self) -> Self {
        match self {
            Self::Available => Self::Due,
            Self::Due => Self::Available,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::Due => "Due date",
        }
    }

    fn anchor(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Due => "due",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Available => "Try: tomorrow at 9am, in 2 weeks",
            Self::Due => "Try: tomorrow, 3 days, Dec 24",
        }
    }

    fn clear_label(self) -> &'static str {
        match self {
            Self::Available => "Available now",
            Self::Due => "No deadline",
        }
    }

    /// The dates worth offering before anything is typed.
    fn presets(self, today: NaiveDate) -> Vec<(&'static str, NaiveDate)> {
        let tomorrow = after_days(today, 1);
        let monday = after_weekday(today, Weekday::Mon);
        let fortnight = after_days(today, 14);
        match self {
            Self::Available => vec![
                ("Tomorrow morning", tomorrow),
                ("Next Monday", monday),
                ("In two weeks", fortnight),
            ],
            Self::Due => vec![
                ("Today", today),
                ("Tomorrow", tomorrow),
                ("Friday", after_weekday(today, Weekday::Fri)),
                ("Next Monday", monday),
                ("In two weeks", fortnight),
            ],
        }
    }

    fn preset_value(self, date: NaiveDate) -> Option<String> {
        match self {
            Self::Available => parse_available_at_input(&format!("{date} at 09:00")).ok(),
            Self::Due => Some(date.to_string()),
        }
    }

    fn day_value(self, date: NaiveDate) -> Option<String> {
        match self {
            Self::Available => parse_available_at_input(&date.to_string()).ok(),
            Self::Due => Some(date.to_string()),
        }
    }

    /// The stored value typed text stands for, empty when it clears the field.
    fn read(self, text: &str, today: NaiveDate) -> Option<String> {
        let text = text.trim().to_lowercase();
        // A bare number would parse as epoch seconds.
        if text.is_empty() || text.chars().all(|letter| letter.is_ascii_digit()) {
            return None;
        }
        match self {
            Self::Available => read_available(&text, today),
            Self::Due => read_due(&text, today),
        }
    }

    /// How a stored value reads, and the day it falls on.
    fn describe(self, value: &str, today: NaiveDate) -> Option<(String, NaiveDate)> {
        match self {
            Self::Available => {
                let at = parse_local_wall_time(value)?;
                Some((moment_label(at, today), at.date()))
            }
            Self::Due => {
                let date = value.parse().ok()?;
                Some((day_label(date, today), date))
            }
        }
    }
}

fn after_days(date: NaiveDate, days: u64) -> NaiveDate {
    date.checked_add_days(Days::new(days)).unwrap_or(date)
}

/// The next `weekday` strictly after `date`.
fn after_weekday(date: NaiveDate, weekday: Weekday) -> NaiveDate {
    let ahead = (weekday.num_days_from_monday() + 6 - date.weekday().num_days_from_monday()) % 7;
    after_days(date, u64::from(ahead) + 1)
}

/// Local calendar date and clock time for schedule editing, without a UTC offset.
fn parse_local_wall_time(at: &str) -> Option<NaiveDateTime> {
    let at = at.parse::<DateTime<Utc>>().ok()?;
    Some(at.with_timezone(&Local).naive_local())
}

fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    let format = if date.year() == today.year() {
        "%a, %b %-d"
    } else {
        "%a, %b %-d, %Y"
    };
    date.format(format).to_string()
}

fn moment_label(at: NaiveDateTime, today: NaiveDate) -> String {
    let day = day_label(at.date(), today);
    if at.time() == NaiveTime::MIN {
        day
    } else {
        format!("{day} at {}", at.format("%H:%M"))
    }
}

fn relative_day(date: NaiveDate, today: NaiveDate) -> String {
    match (date - today).num_days() {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        -1 => "yesterday".to_string(),
        days if days < 0 => format!("{} days ago", -days),
        days => format!("in {days} days"),
    }
}

/// `Dec 24`, `24 Dec` or `December 24th, 2027`. Without a year it is the next
/// time that day comes round.
fn month_and_day(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let words: Vec<&str> = text
        .split(|letter: char| letter.is_whitespace() || letter == ',')
        .filter(|word| !word.is_empty())
        .collect();
    let (first, second, year) = match words.as_slice() {
        [first, second] => (*first, *second, None),
        [first, second, year] => (*first, *second, Some(year.parse::<i32>().ok()?)),
        _ => return None,
    };
    let month = |word: &str| word.parse::<Month>().ok();
    let day = |word: &str| {
        word.trim_end_matches(char::is_alphabetic)
            .parse::<u32>()
            .ok()
    };
    let (month, day) = month(first)
        .zip(day(second))
        .or_else(|| month(second).zip(day(first)))?;
    let on = |year: i32| NaiveDate::from_ymd_opt(year, month.number_from_month(), day);
    match year {
        Some(year) => on(year),
        None => on(today.year())
            .filter(|date| *date >= today)
            .or_else(|| on(today.year() + 1)),
    }
}

/// The ways a date may be meant, in the wording the shared parsers read.
fn spellings(date: &str, today: NaiveDate) -> Vec<String> {
    let mut spellings = vec![
        date.to_string(),
        format!("next {date}"),
        format!("in {date}"),
    ];
    spellings.extend(month_and_day(date, today).map(|date| date.to_string()));
    spellings
}

fn read_due(text: &str, today: NaiveDate) -> Option<String> {
    let text = text.strip_prefix("due ").unwrap_or(text);
    if text == "no deadline" {
        return Some(String::new());
    }
    let spellings = spellings(text, today);
    let mut values = spellings.iter().map(|text| parse_due_on_input(text));
    values.find_map(Result::ok)
}

/// Splits a time of day off the end of a date, with or without `at`.
fn date_and_time(text: &str) -> (&str, Option<&str>) {
    if let Some((date, time)) = text.rsplit_once(" at ") {
        return (date, Some(time));
    }
    let is_time = |word: &str| {
        word.ends_with("am")
            || word.ends_with("pm")
            || word.contains(':')
            || matches!(word, "noon" | "midnight")
    };
    match text.rsplit_once(' ') {
        Some((date, time)) if is_time(time) => (date, Some(time)),
        _ => (text, None),
    }
}

fn read_available(text: &str, today: NaiveDate) -> Option<String> {
    let text = text.strip_prefix("available ").unwrap_or(text);
    if matches!(text, "now" | "none" | "clear") {
        return Some(String::new());
    }
    let (date, time) = date_and_time(text);
    spellings(date, today).iter().find_map(|date| {
        let moment = match time {
            Some(time) => format!("{date} at {time}"),
            None => date.clone(),
        };
        parse_available_at_input(&moment).ok()
    })
}

/// Whether the task stays hidden until after it is due, so that it never reads
/// as overdue. An availability already past hides nothing.
fn hidden_past_deadline(available_at: &str, due_on: &str, today: NaiveDate) -> bool {
    let available = parse_local_wall_time(available_at).map(|at| at.date());
    let due = due_on.parse::<NaiveDate>().ok();
    available
        .zip(due)
        .is_some_and(|(available, due)| available > today && due < available)
}

/// One date field of the target, as a menu sets it.
#[derive(Clone)]
struct Setting {
    field: DateField,
    id: TaskId,
    /// The field's value, unless marked tasks share none.
    stored: Option<Option<String>>,
    /// The other field's value.
    other: Option<String>,
    today: NaiveDate,
}

impl Setting {
    fn note(&self, value: Option<&str>) -> Option<String> {
        let (value, other) = value.zip(self.other.as_deref())?;
        let (available_at, due_on) = match self.field {
            DateField::Available => (value, other),
            DateField::Due => (other, value),
        };
        hidden_past_deadline(available_at, due_on, self.today)
            .then(|| HIDDEN_PAST_DEADLINE.to_string())
    }

    fn entry(
        &self,
        label: impl Into<String>,
        hint: impl Into<String>,
        value: Option<String>,
    ) -> Entry {
        let checked = self.stored.as_ref() == Some(&value);
        let note = self.note(value.as_deref());
        let setting = self.clone();
        Entry::new(label, move |root, _, _| {
            root.save_date(&setting, value.clone());
        })
        .hint(hint)
        .checked(checked)
        .note(note)
    }

    fn entries(&self) -> Vec<Entry> {
        let (field, today) = (self.field, self.today);
        let mut entries: Vec<Entry> = field
            .presets(today)
            .into_iter()
            .filter_map(|(label, date)| {
                let value = field.preset_value(date)?;
                let (hint, _) = field.describe(&value, today)?;
                Some(self.entry(label, hint, Some(value)))
            })
            .collect();
        entries.push(self.entry(field.clear_label(), "", None).sep());
        entries.push(Entry::new("Pick a date…", |_, _, _| {}).calendar());
        entries
    }

    /// What typed text sets, listed above the presets.
    fn typed(&self, text: &str) -> Option<Entry> {
        let (field, today) = (self.field, self.today);
        let value = field.read(text, today)?;
        if value.is_empty() {
            return Some(self.entry(field.clear_label(), "", None));
        }
        let (label, date) = field.describe(&value, today)?;
        Some(self.entry(label, relative_day(date, today), Some(value)))
    }
}

fn unread(text: &str) -> String {
    format!("Can't read \"{text}\". Try 3 days, next friday or Dec 24.")
}

type PickDay = Rc<dyn Fn(&mut Root, NaiveDate)>;

/// A month to pick a day from. The arrow keys move `day`, which decides the
/// month shown.
pub struct Calendar {
    pub day: NaiveDate,
    today: NaiveDate,
    selected: Option<NaiveDate>,
    pick: PickDay,
}

impl Calendar {
    pub fn move_days(&mut self, days: i64) {
        self.day += chrono::Duration::days(days);
    }

    fn move_months(&mut self, months: i32) {
        let step = Months::new(months.unsigned_abs());
        let moved = if months < 0 {
            self.day.checked_sub_months(step)
        } else {
            self.day.checked_add_months(step)
        };
        self.day = moved.unwrap_or(self.day);
    }

    /// The six weeks around the month, from a Monday.
    fn days(&self) -> Vec<NaiveDate> {
        let first = self.day.with_day(1).unwrap_or(self.day);
        let lead = i64::from(first.weekday().num_days_from_monday());
        let start = first - chrono::Duration::days(lead);
        (0..42).map(|ix| after_days(start, ix)).collect()
    }

    pub fn pick(&self) -> impl Fn(&mut Root, NaiveDate) + use<> {
        let pick = self.pick.clone();
        move |root, day| pick(root, day)
    }
}

impl Root {
    pub(crate) fn edit_availability(
        &mut self,
        _: &EditAvailability,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_date_menu(DateField::Available, window, cx);
    }

    pub(crate) fn edit_due(&mut self, _: &EditDue, window: &mut Window, cx: &mut Context<Self>) {
        self.open_date_menu(DateField::Due, window, cx);
    }

    pub(crate) fn open_date_menu(
        &mut self,
        field: DateField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.session.target_row().cloned() else {
            return;
        };
        let today = Local::now().date_naive();
        let bulk = self
            .session
            .bulk_targets()
            .filter(|_| self.session.bulk_panel());
        let detail = self.session.detail.as_ref();
        let detail = detail.filter(|detail| bulk.is_none() && detail.row.id == row.id);
        let available = detail.and_then(|detail| detail.available_at.clone());
        let due = detail.and_then(|detail| detail.due_on.clone());
        let (stored, other) = match field {
            DateField::Available => (available, due),
            DateField::Due => (due, available),
        };
        let selected = stored.as_deref();
        let selected = selected.and_then(|value| field.describe(value, today));
        let setting = Setting {
            field,
            id: row.id,
            stored: detail.is_some().then_some(stored),
            other,
            today,
        };
        let entries = setting.entries();
        let subject = bulk.map_or(row.reference, |ids| format!("{} tasks", ids.len()));
        let typed = setting.clone();
        let read: Read = Rc::new(move |text| typed.typed(text));
        let day = selected.as_ref().map_or(today, |(_, date)| *date);
        let pick: PickDay = Rc::new(move |root, day| {
            let value = setting.field.day_value(day);
            if value.is_some() {
                root.save_date(&setting, value);
            }
        });
        let options = MenuOptions {
            read: Some(read),
            unread: Some(unread),
            header: Some(MenuHeader {
                title: field.title(),
                subject,
                other: field.other().title(),
                swap: Rc::new(move |root, window, cx| {
                    root.replace_menu(
                        move |root, window, cx| root.open_date_menu(field.other(), window, cx),
                        window,
                        cx,
                    );
                }),
            }),
            calendar: Some(Calendar {
                day,
                today,
                selected: selected.map(|(_, date)| date),
                pick,
            }),
            ..MenuOptions::default()
        };
        let anchor = self.anchor(field.anchor());
        self.open_menu(entries, field.placeholder(), anchor, options, window, cx);
    }

    fn save_date(&mut self, setting: &Setting, value: Option<String>) {
        if setting.stored.as_ref() == Some(&value) {
            return;
        }
        let (available_at, due_on) = match setting.field {
            DateField::Available => (Some(value), None),
            DateField::Due => (None, Some(value)),
        };
        let bulk = Bulk::Schedule {
            available_at: available_at.clone(),
            due_on: due_on.clone(),
        };
        if !self.session.bulk(bulk) {
            self.session.write(Command::SetSchedule {
                id: setting.id.clone(),
                available_at,
                due_on,
            });
        }
        self.session.review_deferred();
    }

    pub(crate) fn render_calendar(
        calendar: &Calendar,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> AnyElement {
        let step = |id: &'static str, label: &'static str, text: &'static str, months: i32| {
            let raised = t.raised;
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .flex()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded(px(RADIUS_CONTROL))
                .text_color(t.muted)
                .hover(move |style| style.bg(raised))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(calendar) = this.menu_calendar() {
                        calendar.move_months(months);
                    }
                    cx.notify();
                }))
                .child(text)
        };
        let head = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(CALENDAR_ROW))
            .child(step("calendar-back", "Previous month", "‹", -1))
            .child(
                div()
                    .text_color(t.ink)
                    .font_weight(FontWeight::MEDIUM)
                    .child(calendar.day.format("%B %Y").to_string()),
            )
            .child(step("calendar-next", "Next month", "›", 1));
        let week_days = div().flex().h(px(22.)).children(WEEK_DAYS.map(|name| {
            small(Text::Caption)
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.faint)
                .child(name)
        }));
        let days = calendar.days();
        let weeks = days.chunks(7).enumerate().map(|(week, days)| {
            let days = days.iter().enumerate().map(|(ix, date)| {
                let date = *date;
                let chosen = calendar.selected == Some(date);
                let today = date == calendar.today;
                let dim = date < calendar.today || date.month() != calendar.day.month();
                let color = if chosen {
                    t.on_fill
                } else if today {
                    t.accent
                } else if dim {
                    t.faint
                } else {
                    t.body
                };
                let pick = t.pick();
                div()
                    .id(("calendar-day", week * 7 + ix))
                    .role(Role::Button)
                    .aria_label(day_label(date, calendar.today))
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_CONTROL))
                    .text_color(color)
                    .when(today, |day| day.font_weight(FontWeight::SEMIBOLD))
                    .when(date == calendar.day, |day| day.bg(pick))
                    .when(chosen, |day| day.bg(t.fill))
                    .when(!chosen, |day| day.hover(move |style| style.bg(pick)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_menu_day(date, window, cx);
                    }))
                    .child(date.day().to_string())
            });
            div()
                .flex()
                .h(px(CALENDAR_ROW))
                .children(days.collect::<Vec<_>>())
        });
        div()
            .flex_none()
            .h(px(CALENDAR_HEIGHT))
            .p(px(8.))
            .child(head)
            .child(week_days)
            .children(weeks.collect::<Vec<_>>())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    #[test]
    fn typing_reads_the_shortcuts_the_parser_refuses() {
        let today = today();
        let due = |text: &str| DateField::Due.read(text, today);
        assert_eq!(due("3 days"), Some(after_days(today, 3).to_string()));
        assert_eq!(due("2 weeks"), Some(after_days(today, 14).to_string()));
        let friday = after_weekday(today, Weekday::Fri).to_string();
        for typed in ["friday", "Fri", "tues", "next friday", "due friday"] {
            assert!(due(typed).is_some(), "{typed}");
        }
        assert_eq!(due("friday").as_ref(), Some(&friday));
        assert_eq!(due("due tomorrow"), Some(after_days(today, 1).to_string()));
    }

    #[test]
    fn typing_never_saves_the_words_themselves() {
        let today = today();
        let due = DateField::Due.read("tomorrow", today).unwrap();
        assert!(aven_core::time_validation::validate_due_on_value(&due).is_ok());
        let at = DateField::Available.read("tomorrow", today).unwrap();
        assert!(aven_core::time_validation::validate_available_at_value(&at).is_ok());
    }

    #[test]
    fn text_that_cannot_be_read_saves_nothing() {
        let today = today();
        for typed in ["whenever", "12", "9am", ""] {
            assert_eq!(DateField::Due.read(typed, today), None, "{typed}");
            assert_eq!(DateField::Available.read(typed, today), None, "{typed}");
        }
        assert_eq!(DateField::Due.read("every friday", today), None);
        assert_eq!(DateField::Due.read("tomorrow at 9am", today), None);
    }

    #[test]
    fn clearing_words_clear_their_own_field() {
        let today = today();
        for typed in ["none", "clear", "no deadline"] {
            assert_eq!(DateField::Due.read(typed, today).as_deref(), Some(""));
        }
        for typed in ["now", "none", "clear", "available now"] {
            assert_eq!(DateField::Available.read(typed, today).as_deref(), Some(""));
        }
    }

    #[test]
    fn availability_takes_a_time_with_or_without_at() {
        let today = today();
        let tomorrow = after_days(today, 1);
        let two = tomorrow.and_hms_opt(14, 0, 0).unwrap();
        for typed in ["tomorrow at 2pm", "tomorrow 2pm", "tomorrow 14:00"] {
            let at = DateField::Available.read(typed, today).unwrap();
            assert_eq!(parse_local_wall_time(&at), Some(two), "{typed}");
        }
        let at = DateField::Available.read("dec 24 9am", today).unwrap();
        assert_eq!(
            parse_local_wall_time(&at)
                .unwrap()
                .format("%m-%d %H")
                .to_string(),
            "12-24 09"
        );
        let at = DateField::Available.read("3 days noon", today).unwrap();
        assert_eq!(
            parse_local_wall_time(&at).unwrap().date(),
            after_days(today, 3)
        );
    }

    #[test]
    fn a_month_and_day_is_the_next_time_it_comes_round() {
        let today = date("2026-11-02");
        assert_eq!(month_and_day("dec 24", today), Some(date("2026-12-24")));
        assert_eq!(
            month_and_day("24 december", today),
            Some(date("2026-12-24"))
        );
        assert_eq!(month_and_day("dec 24th", today), Some(date("2026-12-24")));
        assert_eq!(month_and_day("jan 3", today), Some(date("2027-01-03")));
        assert_eq!(month_and_day("nov 2", today), Some(today));
        assert_eq!(
            month_and_day("dec 24, 2028", today),
            Some(date("2028-12-24"))
        );
        assert_eq!(month_and_day("feb 30", today), None);
        assert_eq!(month_and_day("next week", today), None);
    }

    #[test]
    fn weekday_presets_are_always_ahead() {
        let monday = date("2026-11-02");
        assert_eq!(after_weekday(monday, Weekday::Fri), date("2026-11-06"));
        assert_eq!(after_weekday(monday, Weekday::Mon), date("2026-11-09"));
        let friday = date("2026-11-06");
        assert_eq!(after_weekday(friday, Weekday::Fri), date("2026-11-13"));
        let sunday = date("2026-11-08");
        assert_eq!(after_weekday(sunday, Weekday::Mon), date("2026-11-09"));
    }

    #[test]
    fn presets_describe_their_date() {
        let today = today();
        let tomorrow = after_days(today, 1);
        let value = DateField::Due.preset_value(tomorrow).unwrap();
        assert_eq!(value, tomorrow.to_string());
        let value = DateField::Available.preset_value(tomorrow).unwrap();
        let (hint, day) = DateField::Available.describe(&value, today).unwrap();
        assert_eq!(day, tomorrow);
        assert!(hint.ends_with(" at 09:00"), "{hint}");
    }

    #[test]
    fn labels_name_the_year_only_when_it_differs() {
        let today = date("2026-11-02");
        assert_eq!(day_label(date("2026-12-24"), today), "Thu, Dec 24");
        assert_eq!(day_label(date("2027-01-03"), today), "Sun, Jan 3, 2027");
        let midnight = date("2026-12-24").and_time(NaiveTime::MIN);
        assert_eq!(moment_label(midnight, today), "Thu, Dec 24");
    }

    #[test]
    fn relative_hints_count_days() {
        let today = date("2026-11-02");
        assert_eq!(relative_day(today, today), "today");
        assert_eq!(relative_day(date("2026-11-03"), today), "tomorrow");
        assert_eq!(relative_day(date("2026-11-05"), today), "in 3 days");
        assert_eq!(relative_day(date("2026-10-30"), today), "3 days ago");
    }

    #[test]
    fn a_deadline_before_a_future_availability_is_noted() {
        let today = today();
        let at = |days| {
            let date = after_days(today, days);
            parse_available_at_input(&format!("{date} at 09:00")).unwrap()
        };
        let due = |days| after_days(today, days).to_string();
        assert!(hidden_past_deadline(&at(5), &due(3), today));
        assert!(!hidden_past_deadline(&at(3), &due(3), today));
        assert!(!hidden_past_deadline(&at(3), &due(5), today));
    }

    fn setting(field: DateField, stored: Option<Option<String>>, other: Option<&str>) -> Setting {
        Setting {
            field,
            id: TaskId::new(),
            stored,
            other: other.map(str::to_string),
            today: today(),
        }
    }

    #[test]
    fn typed_dates_show_the_date_and_how_far_off_it_is() {
        let today = today();
        let setting = setting(DateField::Due, Some(None), None);
        let entry = setting.typed("3 days").unwrap();
        assert_eq!(entry.label, day_label(after_days(today, 3), today));
        assert_eq!(entry.hint, "in 3 days");
        assert!(setting.typed("whenever").is_none());
        assert_eq!(setting.typed("none").unwrap().label, "No deadline");
    }

    #[test]
    fn a_typed_date_matches_the_preset_that_says_the_same() {
        let setting = setting(DateField::Due, Some(None), None);
        let typed = setting.typed("tomorrow").unwrap();
        let entries = setting.entries();
        let same: Vec<&Entry> = entries
            .iter()
            .filter(|one| one.hint == typed.label)
            .collect();
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].label, "Tomorrow");
    }

    #[test]
    fn the_stored_date_is_checked_unless_marked_tasks_share_none() {
        let tomorrow = after_days(today(), 1).to_string();
        let one = setting(DateField::Due, Some(Some(tomorrow)), None);
        let checked = |setting: &Setting| {
            let entries = setting.entries();
            let checked = entries.into_iter().filter(|entry| entry.checked);
            checked.map(|entry| entry.label).collect::<Vec<_>>()
        };
        assert_eq!(checked(&one), ["Tomorrow"]);
        let unset = setting(DateField::Due, Some(None), None);
        assert_eq!(checked(&unset), ["No deadline"]);
        let marked = setting(DateField::Due, None, None);
        assert!(checked(&marked).is_empty());
    }

    #[test]
    fn dates_that_hide_a_task_past_its_deadline_carry_a_note() {
        let today = today();
        let later = DateField::Available
            .preset_value(after_days(today, 14))
            .unwrap();
        let due = setting(DateField::Due, Some(None), Some(&later));
        let entries = due.entries();
        let today_entry = entries.iter().find(|one| one.label == "Today").unwrap();
        assert!(today_entry.note.is_some());
        let clear = entries
            .iter()
            .find(|one| one.label == "No deadline")
            .unwrap();
        assert!(clear.note.is_none());
        let on_time = due.typed("in 3 weeks").unwrap();
        assert!(on_time.note.is_none());
    }

    #[test]
    fn calendars_show_six_weeks_from_a_monday() {
        let calendar = Calendar {
            day: date("2026-11-02"),
            today: date("2026-11-02"),
            selected: None,
            pick: Rc::new(|_, _| {}),
        };
        let days = calendar.days();
        assert_eq!(days.len(), 42);
        assert_eq!(days[0], date("2026-10-26"));
        assert_eq!(days[41], date("2026-12-06"));
    }

    #[test]
    fn calendars_step_by_month_and_day() {
        let mut calendar = Calendar {
            day: date("2026-01-31"),
            today: date("2026-01-31"),
            selected: None,
            pick: Rc::new(|_, _| {}),
        };
        calendar.move_months(1);
        assert_eq!(calendar.day, date("2026-02-28"));
        calendar.move_months(-1);
        assert_eq!(calendar.day, date("2026-01-28"));
        calendar.move_days(7);
        assert_eq!(calendar.day, date("2026-02-04"));
    }
}
