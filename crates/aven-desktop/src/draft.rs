//! The composer's rules, free of GPUI so they can be tested on their own.
use std::ops::Range;

use anyhow::{Context as _, Result, bail};
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::labels::normalize_label;
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceSchedule, RecurrenceSeriesId};
use aven_host::recurrence_input::{
    RecurrenceClock, RecurrenceDraft, natural_rule_label, upcoming_slots,
};
use aven_host::schedule_input::{ParsedScheduleInput, format_schedule_input, parse_schedule_input};
use aven_host::time_input::{parse_available_at_input, parse_due_on_input};
use chrono::{Local, NaiveDate, Utc};

use crate::adapter::{NavEntry, NewTask, PendingImage, available_text, due_text};
use crate::kit::PRIORITIES;
use crate::toast::Toast;

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

/// What a title means once its tokens are read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleParts {
    pub title: String,
    pub labels: Vec<String>,
    pub priority: Option<TaskPriority>,
    /// The key of the project an `@` token names.
    pub project: Option<String>,
}

/// What a title token sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Label(String),
    Priority(TaskPriority),
    Project(String),
}

/// The project `name` stands for: its key, name or ref prefix, in any case.
fn project_named(name: &str, projects: &[NavEntry]) -> Option<String> {
    let name = name.to_lowercase();
    let by = |field: fn(&NavEntry) -> &str| {
        projects
            .iter()
            .find(|project| field(project).to_lowercase() == name)
    };
    let found = by(|project| &project.key)
        .or_else(|| by(|project| &project.name))
        .or_else(|| by(|project| &project.prefix));
    found.map(|project| project.key.clone())
}

/// What one word of a title sets, if it is a token.
fn read_token(word: &str, projects: &[NavEntry]) -> Option<Token> {
    if let Some(label) = word.strip_prefix('#') {
        return label
            .starts_with(|c: char| c.is_ascii_alphabetic())
            .then(|| Token::Label(normalize_label(label)));
    }
    if let Some(name) = word.strip_prefix('!') {
        let priority = TaskPriority::parse(&name.to_lowercase()).ok()?;
        return (priority != TaskPriority::None).then_some(Token::Priority(priority));
    }
    let name = word.strip_prefix('@').filter(|name| !name.is_empty())?;
    project_named(name, projects).map(Token::Project)
}

/// The words of `text` with where each starts.
fn words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split_whitespace()
        .map(move |word| (word.as_ptr() as usize - text.as_ptr() as usize, word))
}

/// Where the tokens of a title are and what each sets. `\#word` is text.
pub fn title_tokens(text: &str, projects: &[NavEntry]) -> Vec<(Range<usize>, Token)> {
    words(text)
        .filter_map(|(at, word)| Some((at..at + word.len(), read_token(word, projects)?)))
        .collect()
}

/// Reads `#label`, `!priority` and `@project` out of a title. `\#word` stays
/// as text without its backslash.
pub fn parse_title(text: &str, projects: &[NavEntry]) -> TitleParts {
    let mut parts = TitleParts::default();
    let mut kept = Vec::new();
    for (_, word) in words(text) {
        if let Some(rest) = word.strip_prefix('\\')
            && rest.starts_with(['#', '!', '@'])
        {
            kept.push(rest);
            continue;
        }
        match read_token(word, projects) {
            Some(Token::Label(label)) => parts.labels.push(label),
            Some(Token::Priority(priority)) => parts.priority = parts.priority.or(Some(priority)),
            Some(Token::Project(key)) => parts.project = parts.project.or(Some(key)),
            None => kept.push(word),
        }
    }
    parts.title = kept.join(" ");
    parts
}

/// What takes the place of `range` when it leaves `text`: the spaces around
/// it go too, and one stays between the words either side.
pub fn cut(text: &str, range: Range<usize>) -> (Range<usize>, &'static str) {
    let start = text[..range.start].trim_end().len();
    let end = text.len() - text[range.end..].trim_start().len();
    (start..end, if start > 0 { " " } else { "" })
}

/// The token a typed space just finished, when `now` is `prev` with one
/// whitespace character added before `caret`. The range covers the space.
pub fn finished_token(
    prev: &str,
    now: &str,
    caret: usize,
    projects: &[NavEntry],
) -> Option<(Range<usize>, Token)> {
    let space = now.get(..caret)?.chars().next_back()?;
    let typed_at = caret - space.len_utf8();
    let added = space.is_whitespace()
        && now.len() == prev.len() + space.len_utf8()
        && prev.get(..typed_at) == now.get(..typed_at)
        && prev.get(typed_at..) == now.get(caret..);
    if !added {
        return None;
    }
    let before = &now[..typed_at];
    let start = word_start(before);
    let token = read_token(&before[start..], projects)?;
    Some((start..caret, token))
}

/// Where the last word of `before` starts.
fn word_start(before: &str) -> usize {
    before.len()
        - before
            .rsplit(char::is_whitespace)
            .next()
            .unwrap_or("")
            .len()
}

/// A `#`, `!` or `@` word the caret ends, being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaretWord {
    pub start: usize,
    pub word: String,
}

/// The token word that ends at `caret`, if one is being typed there.
pub fn caret_word(text: &str, caret: usize) -> Option<CaretWord> {
    let before = text.get(..caret)?;
    let start = word_start(before);
    let word = &before[start..];
    let query = word.strip_prefix(['#', '!', '@'])?;
    query
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        .then(|| CaretWord {
            start,
            word: word.to_string(),
        })
}

/// The most labels or projects a word offers; typing more narrows them.
const SUGGESTION_LIMIT: usize = 8;

/// One way to finish the word being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    pub hint: String,
    pub token: Token,
}

/// What the word being typed could become: labels not yet on the draft, with
/// a new one for an unknown name; priorities; or projects by name, key or
/// ref prefix. Nothing when the word already reads as the only choice.
pub fn suggestions(
    word: &CaretWord,
    held: &[String],
    labels: &[String],
    projects: &[NavEntry],
) -> Vec<Suggestion> {
    let query = word.word[1..].to_lowercase();
    let mut found: Vec<Suggestion> = match word.word.as_bytes()[0] {
        b'#' => {
            let mut found: Vec<Suggestion> = labels
                .iter()
                .filter(|label| label.starts_with(&query) && !held.contains(label))
                .take(SUGGESTION_LIMIT)
                .map(|label| Suggestion {
                    label: label.clone(),
                    hint: String::new(),
                    token: Token::Label(label.clone()),
                })
                .collect();
            if let Some(Token::Label(new)) = read_token(&word.word, projects)
                && !labels.contains(&new)
                && !held.contains(&new)
            {
                found.push(Suggestion {
                    label: format!("New label \u{201c}{new}\u{201d}"),
                    hint: String::new(),
                    token: Token::Label(new),
                });
            }
            found
        }
        b'!' => PRIORITIES
            .iter()
            .filter(|(priority, name)| {
                *priority != TaskPriority::None && name.to_lowercase().starts_with(&query)
            })
            .map(|(priority, name)| Suggestion {
                label: name.to_string(),
                hint: String::new(),
                token: Token::Priority(*priority),
            })
            .collect(),
        _ => projects
            .iter()
            .filter(|project| {
                [&project.name, &project.key, &project.prefix]
                    .iter()
                    .any(|field| field.to_lowercase().starts_with(&query))
            })
            .take(SUGGESTION_LIMIT)
            .map(|project| Suggestion {
                label: project.name.clone(),
                hint: project.prefix.clone(),
                token: Token::Project(project.key.clone()),
            })
            .collect(),
    };
    let settled = read_token(&word.word, projects);
    if found.len() == 1 && settled.as_ref() == Some(&found[0].token) {
        found.clear();
    }
    found
}

/// A one-off schedule's typed dates as stored: an RFC 3339 availability and a
/// `YYYY-MM-DD` deadline, empty when unset.
fn resolved_dates(available_at: &str, due_on: &str) -> (String, String) {
    let resolve = |text: &str, parse: fn(&str) -> Result<String>| {
        if text.is_empty() {
            String::new()
        } else {
            parse(text).unwrap_or_default()
        }
    };
    (
        resolve(available_at, parse_available_at_input),
        resolve(due_on, parse_due_on_input),
    )
}

/// What the schedule chip says about a schedule's text, if it has one.
pub fn schedule_chip(text: &str) -> Option<String> {
    let today = Local::now().date_naive();
    match parse_schedule_input(text).ok()? {
        ParsedScheduleInput::None => None,
        ParsedScheduleInput::Once {
            available_at,
            due_on,
        } => {
            let (available_at, due_on) = resolved_dates(&available_at, &due_on);
            let mut bits = Vec::new();
            if !available_at.is_empty() {
                bits.push(available_text(&available_at, today));
            }
            if !due_on.is_empty() {
                bits.push(format!("Due {}", due_text(&due_on, today)));
            }
            Some(bits.join(" · "))
        }
        ParsedScheduleInput::Recurring { .. } => {
            let schedule = recurrence_schedule(text).ok()?;
            Some(natural_rule_label(schedule.rule))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulePreview {
    pub text: String,
    pub valid: bool,
    pub repeats: bool,
}

const SCHEDULE_HELP: &str =
    "Try \"tomorrow at 9am\", \"due next friday\" or \"every friday at 09:00\".";

/// Says when dates take effect, in the terms the task list uses.
pub fn dates_preview(available_at: &str, due_on: &str, today: NaiveDate) -> String {
    let mut bits = Vec::new();
    if !available_at.is_empty() {
        let until = available_text(available_at, today);
        bits.push(format!("Hidden from Queue until {until}"));
    }
    if !due_on.is_empty() {
        bits.push(format!("Due {}", due_text(due_on, today)));
    }
    if bits.len() == 2 && due_on < &available_at[..due_on.len().min(available_at.len())] {
        bits.push("Available after its deadline, so it won't appear in Overdue".to_string());
    }
    if bits.is_empty() {
        "Available now, no deadline.".to_string()
    } else {
        format!("{}.", bits.join(". "))
    }
}

fn recurrence_preview(text: &str) -> Option<String> {
    let schedule = recurrence_schedule(text).ok()?;
    let dates: Vec<String> = upcoming_slots(&schedule, Utc::now(), 3)
        .iter()
        .map(|date| date.format("%b %-d").to_string())
        .collect();
    let time = schedule
        .available_local_time
        .map(|time| format!(" at {}", time.format("%H:%M")))
        .unwrap_or_default();
    let due = match schedule.due_policy {
        RecurrenceDuePolicy::None => "No due date",
        _ => "Same day",
    };
    Some(format!(
        "{}{time}. {due}. Next: {}.",
        natural_rule_label(schedule.rule),
        dates.join(", ")
    ))
}

pub fn schedule_preview(text: &str) -> SchedulePreview {
    let today = Local::now().date_naive();
    match parse_schedule_input(text) {
        Ok(ParsedScheduleInput::None) => SchedulePreview {
            text: dates_preview("", "", today),
            valid: true,
            repeats: false,
        },
        Ok(ParsedScheduleInput::Once {
            available_at,
            due_on,
        }) => SchedulePreview {
            text: {
                let (available_at, due_on) = resolved_dates(&available_at, &due_on);
                dates_preview(&available_at, &due_on, today)
            },
            valid: true,
            repeats: false,
        },
        Ok(ParsedScheduleInput::Recurring { .. }) => match recurrence_preview(text) {
            Some(text) => SchedulePreview {
                text,
                valid: true,
                repeats: true,
            },
            None => SchedulePreview {
                text: SCHEDULE_HELP.to_string(),
                valid: false,
                repeats: true,
            },
        },
        Err(_) => SchedulePreview {
            text: SCHEDULE_HELP.to_string(),
            valid: false,
            repeats: false,
        },
    }
}

/// The status a new task takes when the user leaves it to Aven.
pub fn automatic_status(priority: TaskPriority, repeats: bool, is_epic: bool) -> TaskStatus {
    if repeats || is_epic || priority.promotes_inbox_to_todo() {
        TaskStatus::Todo
    } else {
        TaskStatus::Inbox
    }
}

/// A task being written in the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub title: String,
    pub body: String,
    pub schedule: String,
    pub project: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: TaskPriority,
    pub labels: Vec<String>,
    pub metadata: Vec<(String, String)>,
    pub is_epic: bool,
    pub images: Vec<PendingImage>,
    /// The series this one takes over from, which stops once it exists.
    pub replaces: Option<RecurrenceSeriesId>,
}

/// A draft as it is kept across launches.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SavedDraft {
    pub title: String,
    pub body: String,
    pub schedule: String,
    pub project: Option<String>,
    pub status: Option<String>,
    pub priority: String,
    pub labels: Vec<String>,
    pub metadata: Vec<(String, String)>,
    pub is_epic: bool,
    pub images: Vec<(String, Vec<u8>)>,
    pub replaces: Option<RecurrenceSeriesId>,
}

impl From<&Draft> for SavedDraft {
    fn from(draft: &Draft) -> Self {
        Self {
            title: draft.title.clone(),
            body: draft.body.clone(),
            schedule: draft.schedule.clone(),
            project: draft.project.clone(),
            status: draft.status.map(|status| status.as_str().to_string()),
            priority: draft.priority.as_str().to_string(),
            labels: draft.labels.clone(),
            metadata: draft.metadata.clone(),
            is_epic: draft.is_epic,
            images: draft
                .images
                .iter()
                .map(|image| (image.name.clone(), image.bytes.to_vec()))
                .collect(),
            replaces: draft.replaces.clone(),
        }
    }
}

impl From<SavedDraft> for Draft {
    fn from(saved: SavedDraft) -> Self {
        Self {
            title: saved.title,
            body: saved.body,
            schedule: saved.schedule,
            project: saved.project,
            status: saved.status.and_then(|text| TaskStatus::parse(&text).ok()),
            priority: TaskPriority::parse(&saved.priority).unwrap_or(TaskPriority::None),
            labels: saved.labels,
            metadata: saved.metadata,
            is_epic: saved.is_epic,
            images: saved
                .images
                .into_iter()
                .map(|(name, bytes)| PendingImage {
                    name,
                    bytes: bytes.into(),
                })
                .collect(),
            replaces: saved.replaces,
        }
    }
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            schedule: String::new(),
            project: None,
            status: None,
            priority: TaskPriority::None,
            labels: Vec::new(),
            metadata: Vec::new(),
            is_epic: false,
            images: Vec::new(),
            replaces: None,
        }
    }
}

impl Draft {
    /// Whether closing the composer would lose something typed.
    pub fn has_text(&self) -> bool {
        !self.title.trim().is_empty() || !self.body.trim().is_empty()
    }

    pub fn repeats(&self) -> bool {
        schedule_preview(&self.schedule).repeats
    }

    /// The task to create, or why it cannot be. Tokens in the title win over
    /// the fields.
    pub fn task(&self, projects: &[NavEntry]) -> Result<NewTask, DraftProblem> {
        let parts = parse_title(&self.title, projects);
        if parts.title.is_empty() {
            return Err(DraftProblem::Title);
        }
        let preview = schedule_preview(&self.schedule);
        if !preview.valid {
            return Err(DraftProblem::Schedule);
        }
        if preview.repeats && self.is_epic {
            return Err(DraftProblem::RecurringEpic);
        }
        let mut labels = self.labels.clone();
        for label in parts.labels {
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        let priority = parts.priority.unwrap_or(self.priority);
        let status = self
            .status
            .unwrap_or_else(|| automatic_status(priority, preview.repeats, self.is_epic));
        if preview.repeats && !status.is_open() {
            return Err(DraftProblem::RecurringStatus);
        }
        if preview.repeats && !self.images.is_empty() {
            return Err(DraftProblem::RecurringAttachments);
        }
        Ok(NewTask {
            title: parts.title,
            description: self.body.clone(),
            project: parts.project.or(self.project.clone()),
            status: Some(status.as_str().to_string()),
            priority: priority.as_str().to_string(),
            labels,
            metadata: self.metadata.clone(),
            is_epic: self.is_epic,
            schedule: self.schedule.trim().to_string(),
            images: self.images.clone(),
            replaces: self.replaces.clone(),
            first_project: None,
        })
    }

    /// A task that could not be saved, back as a draft.
    pub fn from_task(task: NewTask) -> Self {
        let mut draft = Self {
            images: task.images.clone(),
            ..Self::default()
        };
        draft.fill(task);
        draft
    }

    /// Takes the fields a reader produced. Attachments stay.
    pub fn fill(&mut self, task: NewTask) {
        self.title = task.title;
        self.body = task.description;
        self.schedule = task.schedule;
        self.project = task.project.or(self.project.take());
        self.status = task
            .status
            .as_deref()
            .and_then(|status| TaskStatus::parse(status).ok());
        self.priority = TaskPriority::parse(&task.priority).unwrap_or(TaskPriority::None);
        self.labels = task.labels;
        self.metadata = task.metadata;
        self.is_epic = task.is_epic;
        self.replaces = task.replaces;
    }

    /// Takes the reader's task, unless the draft has moved on from `notes`.
    pub fn fill_from_notes(&mut self, notes: &str, task: NewTask) -> bool {
        let current = self.intake_text() == notes;
        if current {
            self.fill(task);
        }
        current
    }

    /// The rough notes handed to the natural language reader.
    pub fn intake_text(&self) -> String {
        match (self.title.trim(), self.body.trim()) {
            ("", "") => String::new(),
            (title, "") => title.to_string(),
            ("", body) => format!("Description:\n{body}"),
            (title, body) => format!("Title:\n{title}\n\nDescription:\n{body}"),
        }
    }

    /// What stays for the next task after creating one with Add another.
    pub fn after_created(&self) -> Self {
        Self {
            project: self.project.clone(),
            status: self.status,
            priority: self.priority,
            labels: self.labels.clone(),
            ..Self::default()
        }
    }

    /// Sets the field a token names. A label joins once.
    pub fn apply(&mut self, token: Token) {
        match token {
            Token::Label(label) => {
                if !self.labels.contains(&label) {
                    self.labels.push(label);
                }
            }
            Token::Priority(priority) => self.priority = priority,
            Token::Project(key) => self.project = Some(key),
        }
    }

    /// Moves the title's tokens into the fields they set, as creating would
    /// read them, so the fields can be edited on their own.
    pub fn settle_title(&mut self, projects: &[NavEntry]) {
        let parts = parse_title(&self.title, projects);
        let mut title = self.title.clone();
        for (range, _) in title_tokens(&self.title, projects).into_iter().rev() {
            let (range, glue) = cut(&title, range);
            title.replace_range(range, glue);
        }
        self.title = title;
        for label in parts.labels {
            self.apply(Token::Label(label));
        }
        if let Some(priority) = parts.priority {
            self.priority = priority;
        }
        if let Some(key) = parts.project {
            self.project = Some(key);
        }
    }

    /// Attaches an image once, however often it is dropped.
    pub fn add_image(&mut self, image: PendingImage) {
        if !self.images.iter().any(|held| held.bytes == image.bytes) {
            self.images.push(image);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftProblem {
    Title,
    Schedule,
    RecurringEpic,
    RecurringStatus,
    RecurringAttachments,
}

impl DraftProblem {
    /// Why the draft was refused, as the terminal app refuses it.
    pub fn toast(self) -> Toast {
        match self {
            Self::Title => Toast::info("Add a title"),
            Self::Schedule => Toast::info("Check the schedule"),
            Self::RecurringEpic => Toast::info("A recurring task cannot be an epic"),
            Self::RecurringStatus => Toast::info("A recurring task starts open")
                .detail("Choose Inbox, Backlog, Todo or Active"),
            Self::RecurringAttachments => Toast::info("Add images once the recurring task exists"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn title_tokens_become_fields_and_escapes_stay_text() {
        let parts = parse_title("Ship #Mobile-App the \\#build !HIGH today #2 !nope", &[]);
        assert_eq!(parts.title, "Ship the #build today #2 !nope");
        assert_eq!(parts.labels, ["mobile-app"]);
        assert_eq!(parts.priority, Some(TaskPriority::High));
    }

    fn projects() -> Vec<NavEntry> {
        let project = |name: &str, key: &str, prefix: &str| NavEntry {
            name: name.to_string(),
            key: key.to_string(),
            prefix: prefix.to_string(),
            count: 0,
        };
        vec![
            project("Aven", "aven", "AVN"),
            project("Mobile App", "mobile-app", "APP"),
        ]
    }

    #[test]
    fn a_project_token_names_a_key_name_or_prefix_and_beats_the_field() {
        let projects = projects();
        let parts = parse_title("Fix @AVN sync @nope \\@aven", &projects);
        assert_eq!(parts.title, "Fix sync @nope @aven");
        assert_eq!(parts.project.as_deref(), Some("aven"));
        let named = |text| parse_title(text, &projects).project;
        assert_eq!(named("@Mobile-App").as_deref(), Some("mobile-app"));
        assert_eq!(named("@aven @app").as_deref(), Some("aven"));
        assert_eq!(named("@"), None);

        let draft = Draft {
            title: "Ship @app".to_string(),
            project: Some("aven".to_string()),
            ..Draft::default()
        };
        assert_eq!(
            draft.task(&projects).unwrap().project.as_deref(),
            Some("mobile-app")
        );
        assert_eq!(draft.task(&[]).unwrap().title, "Ship @app");
    }

    #[test]
    fn token_ranges_are_bytes_of_the_title() {
        let text = "Käy #ruoka !HIGH \\#x @avn #2";
        let tokens = title_tokens(text, &projects());
        let found: Vec<(&str, Token)> = tokens
            .into_iter()
            .map(|(range, token)| (&text[range], token))
            .collect();
        assert_eq!(
            found,
            [
                ("#ruoka", Token::Label("ruoka".to_string())),
                ("!HIGH", Token::Priority(TaskPriority::High)),
                ("@avn", Token::Project("aven".to_string())),
            ]
        );
    }

    #[test]
    fn only_a_typed_space_finishes_a_token() {
        let projects = projects();
        let label = Token::Label("bug".to_string());
        assert_eq!(
            finished_token("Fix #bug", "Fix #bug ", 9, &projects),
            Some((4..9, label.clone()))
        );
        assert_eq!(
            finished_token("Fix #bug later", "Fix #bug  later", 9, &projects),
            Some((4..9, label))
        );
        assert_eq!(
            finished_token("ä @avn", "ä @avn ", 8, &projects),
            Some((3..8, Token::Project("aven".to_string())))
        );
        // A paste of the whole token, an unknown token and plain words.
        assert_eq!(finished_token("Fix", "Fix #bug ", 9, &projects), None);
        assert_eq!(
            finished_token("Fix !nope", "Fix !nope ", 10, &projects),
            None
        );
        assert_eq!(finished_token("Fix it", "Fix it ", 7, &projects), None);
        assert_eq!(finished_token("Fix #bug", "Fix #bugs", 9, &projects), None);
    }

    #[test]
    fn a_cut_leaves_one_space_between_the_words_either_side() {
        let apply = |text: &str, range| {
            let (range, glue) = cut(text, range);
            let mut text = text.to_string();
            text.replace_range(range.clone(), glue);
            (text, range.start + glue.len())
        };
        assert_eq!(apply("Fix #bug  later", 4..9), ("Fix later".to_string(), 4));
        assert_eq!(apply("#bug fix", 0..5), ("fix".to_string(), 0));
        assert_eq!(apply("Fix #bug ", 4..9), ("Fix ".to_string(), 4));
    }

    #[test]
    fn the_caret_word_is_a_trigger_and_name_ending_at_the_caret() {
        let word = |text: &str, caret| caret_word(text, caret).map(|word| (word.start, word.word));
        assert_eq!(word("Fix #bu", 7), Some((4, "#bu".to_string())));
        assert_eq!(word("Fix #bu more", 7), Some((4, "#bu".to_string())));
        assert_eq!(word("ä @", 4), Some((3, "@".to_string())));
        assert_eq!(word("Fix#bu", 6), None);
        assert_eq!(word("Fix bu", 6), None);
        assert_eq!(word("#a.b", 4), None);
    }

    #[test]
    fn suggestions_leave_out_held_labels_and_hide_once_settled() {
        let projects = projects();
        let labels = ["bug", "build", "docs"].map(String::from);
        let held = ["build".to_string()];
        let shown = |word: &str| -> Vec<String> {
            let word = caret_word(word, word.len()).unwrap();
            suggestions(&word, &held, &labels, &projects)
                .into_iter()
                .map(|suggestion| suggestion.label)
                .collect()
        };
        assert_eq!(shown("#bu"), ["bug", "New label \u{201c}bu\u{201d}"]);
        assert!(shown("#bug").is_empty());
        assert!(shown("#build").is_empty());
        assert_eq!(shown("#"), ["bug", "docs"]);
        assert_eq!(shown("!h"), ["High"]);
        assert!(shown("!high").is_empty());
        assert_eq!(shown("@a"), ["Aven", "Mobile App"]);
        assert!(shown("@avn").is_empty());
        assert!(shown("!x").is_empty());
        assert!(shown("#2").is_empty());
        let many: Vec<String> = (0..20).map(|n| format!("tag{n}")).collect();
        let word = caret_word("#tag", 4).unwrap();
        let found = suggestions(&word, &[], &many, &projects);
        assert_eq!(found.len(), SUGGESTION_LIMIT + 1);
        assert_eq!(found.last().unwrap().label, "New label \u{201c}tag\u{201d}");
    }

    #[test]
    fn settling_a_title_moves_its_tokens_into_the_fields() {
        let mut draft = Draft {
            title: "Fix \\#x #bug !high sync @avn".to_string(),
            labels: vec!["home".to_string()],
            priority: TaskPriority::Low,
            ..Draft::default()
        };
        let before = draft.task(&projects()).unwrap();
        draft.settle_title(&projects());
        // The space a cut leaves at the end is where typing carries on.
        assert_eq!(draft.title, "Fix \\#x sync ");
        assert_eq!(draft.labels, ["home", "bug"]);
        assert_eq!(draft.priority, TaskPriority::High);
        assert_eq!(draft.project.as_deref(), Some("aven"));
        assert_eq!(draft.task(&projects()).unwrap(), before);
    }

    #[test]
    fn a_draft_saved_with_the_details_toggle_still_loads() {
        let saved: SavedDraft =
            serde_json::from_str(r#"{"title":"Pay rent","more":true,"labels":["home"]}"#).unwrap();
        let draft = Draft::from(saved);
        assert_eq!(draft.title, "Pay rent");
        assert_eq!(draft.labels, ["home"]);
    }

    #[test]
    fn a_reader_reply_leaves_a_draft_edited_since() {
        let mut draft = Draft {
            title: "Call the bank".to_string(),
            ..Draft::default()
        };
        let notes = draft.intake_text();
        draft.title = "Call the bank about the card".to_string();
        assert!(!draft.fill_from_notes(&notes, NewTask::titled("From notes")));
        assert_eq!(draft.title, "Call the bank about the card");

        let notes = draft.intake_text();
        assert!(draft.fill_from_notes(&notes, NewTask::titled("From notes")));
        assert_eq!(draft.title, "From notes");
    }

    #[test]
    fn the_automatic_status_follows_the_terminal_app() {
        assert_eq!(
            automatic_status(TaskPriority::None, false, false),
            TaskStatus::Inbox
        );
        assert_eq!(
            automatic_status(TaskPriority::Low, false, false),
            TaskStatus::Inbox
        );
        assert_eq!(
            automatic_status(TaskPriority::Medium, false, false),
            TaskStatus::Todo
        );
        assert_eq!(
            automatic_status(TaskPriority::None, true, false),
            TaskStatus::Todo
        );
        assert_eq!(
            automatic_status(TaskPriority::None, false, true),
            TaskStatus::Todo
        );
    }

    #[test]
    fn a_recurring_draft_refuses_what_the_terminal_app_refuses() {
        let mut draft = Draft {
            title: "Pay rent".to_string(),
            schedule: "every friday at 09:00".to_string(),
            is_epic: true,
            ..Draft::default()
        };
        let problem = draft.task(&[]).unwrap_err();
        assert_eq!(problem, DraftProblem::RecurringEpic);
        assert_eq!(problem.toast().title, "A recurring task cannot be an epic");

        draft.is_epic = false;
        draft.status = Some(TaskStatus::Done);
        assert_eq!(draft.task(&[]), Err(DraftProblem::RecurringStatus));

        draft.status = None;
        draft.add_image(PendingImage {
            name: "receipt.png".to_string(),
            bytes: Arc::new(vec![1]),
        });
        let problem = draft.task(&[]).unwrap_err();
        assert_eq!(problem, DraftProblem::RecurringAttachments);
        assert_eq!(
            problem.toast().title,
            "Add images once the recurring task exists"
        );

        draft.schedule = "tomorrow".to_string();
        assert!(draft.task(&[]).is_ok());
    }

    #[test]
    fn a_draft_needs_a_title_and_a_readable_schedule() {
        let mut draft = Draft::default();
        assert_eq!(draft.task(&[]), Err(DraftProblem::Title));
        draft.title = "#only-a-label".to_string();
        assert_eq!(draft.task(&[]), Err(DraftProblem::Title));
        draft.title = "Pay rent".to_string();
        draft.schedule = "whenever suits".to_string();
        assert_eq!(draft.task(&[]), Err(DraftProblem::Schedule));
        draft.schedule = "every friday at 09:00".to_string();
        let task = draft.task(&[]).unwrap();
        assert_eq!(task.status.as_deref(), Some("todo"));
        assert!(draft.repeats());
    }

    #[test]
    fn token_labels_join_chosen_ones_once() {
        let draft = Draft {
            title: "Pay #bills #home !urgent".to_string(),
            labels: vec!["home".to_string()],
            ..Draft::default()
        };
        let task = draft.task(&[]).unwrap();
        assert_eq!(task.title, "Pay");
        assert_eq!(task.labels, ["home", "bills"]);
        assert_eq!(task.priority, "urgent");
        assert_eq!(task.status.as_deref(), Some("todo"));
    }

    #[test]
    fn add_another_keeps_the_choices_and_clears_the_task() {
        let image = PendingImage {
            name: "a.png".to_string(),
            bytes: Arc::new(vec![1]),
        };
        let draft = Draft {
            title: "One".to_string(),
            body: "Body".to_string(),
            schedule: "tomorrow".to_string(),
            project: Some("key".to_string()),
            status: Some(TaskStatus::Active),
            priority: TaskPriority::High,
            labels: vec!["a".to_string()],
            metadata: vec![("k".to_string(), "v".to_string())],
            is_epic: true,
            images: vec![image],
            replaces: None,
        };
        let next = draft.after_created();
        assert_eq!(next.project.as_deref(), Some("key"));
        assert_eq!(next.status, Some(TaskStatus::Active));
        assert_eq!(next.priority, TaskPriority::High);
        assert_eq!(next.labels, ["a"]);
        assert!(next.title.is_empty() && next.body.is_empty() && next.schedule.is_empty());
        assert!(next.metadata.is_empty() && next.images.is_empty() && !next.is_epic);
    }

    #[test]
    fn intake_text_matches_the_terminal_app() {
        let mut draft = Draft::default();
        assert_eq!(draft.intake_text(), "");
        draft.title = "T".to_string();
        assert_eq!(draft.intake_text(), "T");
        draft.body = "D".to_string();
        assert_eq!(draft.intake_text(), "Title:\nT\n\nDescription:\nD");
        draft.title.clear();
        assert_eq!(draft.intake_text(), "Description:\nD");
    }

    #[test]
    fn intake_keeps_attachments_and_the_same_image_lands_once() {
        let image = PendingImage {
            name: "a.png".to_string(),
            bytes: Arc::new(vec![1]),
        };
        let mut draft = Draft::default();
        draft.add_image(image.clone());
        draft.add_image(PendingImage {
            name: "copy.png".to_string(),
            ..image
        });
        assert_eq!(draft.images.len(), 1);
        draft.fill(NewTask::titled("From notes"));
        assert_eq!(draft.title, "From notes");
        assert_eq!(draft.images.len(), 1);
    }

    #[test]
    fn previews_speak_the_lists_terms() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        assert_eq!(dates_preview("", "", today), "Available now, no deadline.");
        assert!(dates_preview("", "2026-11-02", today).starts_with("Due "));
        let bad = schedule_preview("whenever suits");
        assert!(!bad.valid);
        assert!(bad.text.starts_with("Try "));
        let repeat = schedule_preview("every friday at 09:00");
        assert!(repeat.repeats && repeat.valid);
        assert!(repeat.text.contains(". Next: "), "{}", repeat.text);
        let date = schedule_preview("due 2026-12-24");
        assert!(date.valid && !date.repeats);
        let typed = schedule_preview("tomorrow, due 2099-12-24");
        assert!(!typed.text.contains("until ."), "{}", typed.text);
        assert!(typed.text.contains("Due "), "{}", typed.text);
        let chip = schedule_chip("tomorrow").unwrap();
        assert!(!chip.is_empty());
    }
}
