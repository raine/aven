//! The composer's rules, free of GPUI so they can be tested on their own.
use anyhow::{Context as _, Result, bail};
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::labels::normalize_label;
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceSchedule, RecurrenceSeriesId};
use aven_host::recurrence_input::{
    RecurrenceClock, RecurrenceDraft, natural_rule_label, upcoming_slots,
};
use aven_host::schedule_input::{ParsedScheduleInput, format_schedule_input, parse_schedule_input};
use chrono::{Local, NaiveDate, Utc};

use crate::adapter::{NewTask, PendingImage, available_text, due_text};

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
}

fn priority_token(word: &str) -> Option<TaskPriority> {
    match word.to_lowercase().as_str() {
        "!urgent" => Some(TaskPriority::Urgent),
        "!high" => Some(TaskPriority::High),
        "!medium" => Some(TaskPriority::Medium),
        "!low" => Some(TaskPriority::Low),
        _ => None,
    }
}

/// Reads `#label` and `!priority` out of a title. `\#word` stays as text.
pub fn parse_title(text: &str) -> TitleParts {
    let mut parts = TitleParts::default();
    let mut kept = Vec::new();
    for word in text.split_whitespace() {
        if let Some(rest) = word.strip_prefix('\\')
            && rest.starts_with(['#', '!'])
        {
            kept.push(rest);
        } else if word.len() > 1
            && word.starts_with('#')
            && word[1..].starts_with(|c: char| c.is_ascii_alphabetic())
        {
            parts.labels.push(normalize_label(&word[1..]));
        } else if let Some(priority) = priority_token(word) {
            parts.priority = parts.priority.or(Some(priority));
        } else {
            kept.push(word);
        }
    }
    parts.title = kept.join(" ");
    parts
}

/// Says what the tokens in a title will do, or nothing without any.
pub fn token_preview(text: &str) -> String {
    let parts = parse_title(text);
    if parts.labels.is_empty() && parts.priority.is_none() {
        return String::new();
    }
    let mut line = format!("Creates \u{201c}{}\u{201d}", parts.title);
    if !parts.labels.is_empty() {
        let labels: Vec<String> = parts.labels.iter().map(|l| format!("#{l}")).collect();
        line.push_str(&format!(" with {}", labels.join(" ")));
    }
    if let Some(priority) = parts.priority {
        line.push_str(&format!(
            ", {} priority",
            crate::kit::priority_name(priority)
        ));
    }
    line.push_str(". Write \\#tag to keep it as text.");
    line
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
            text: dates_preview(&available_at, &due_on, today),
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
    pub more: bool,
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
    pub more: bool,
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
            more: draft.more,
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
            more: saved.more,
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
            more: false,
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

    pub fn shown_status(&self) -> TaskStatus {
        let priority = parse_title(&self.title).priority.unwrap_or(self.priority);
        self.status
            .unwrap_or_else(|| automatic_status(priority, self.repeats(), self.is_epic))
    }

    /// The task to create, or why it cannot be.
    pub fn task(&self) -> Result<NewTask, DraftProblem> {
        let parts = parse_title(&self.title);
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
            project: self.project.clone(),
            status: Some(status.as_str().to_string()),
            priority: priority.as_str().to_string(),
            labels,
            metadata: self.metadata.clone(),
            is_epic: self.is_epic,
            schedule: self.schedule.trim().to_string(),
            images: self.images.clone(),
            replaces: self.replaces.clone(),
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
        self.more = !self.body.is_empty() || !self.schedule.is_empty();
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
            more: self.more,
            ..Self::default()
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
    /// What the terminal app says when it refuses the same draft.
    pub fn message(self) -> &'static str {
        match self {
            Self::Title => "Add a title",
            Self::Schedule => "Check the schedule",
            Self::RecurringEpic => "recurring tasks cannot be epic containers",
            Self::RecurringStatus => {
                "recurring tasks require an open initial status: inbox, backlog, todo, or active"
            }
            Self::RecurringAttachments => {
                "create the recurring task before adding occurrence attachments"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn title_tokens_become_fields_and_escapes_stay_text() {
        let parts = parse_title("Ship #Mobile-App the \\#build !HIGH today #2 !nope");
        assert_eq!(parts.title, "Ship the #build today #2 !nope");
        assert_eq!(parts.labels, ["mobile-app"]);
        assert_eq!(parts.priority, Some(TaskPriority::High));
        assert_eq!(
            token_preview("Ship #ios !low"),
            "Creates \u{201c}Ship\u{201d} with #ios, Low priority. Write \\#tag to keep it as text."
        );
        assert_eq!(token_preview("Plain title"), "");
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
        let problem = draft.task().unwrap_err();
        assert_eq!(problem, DraftProblem::RecurringEpic);
        assert_eq!(
            problem.message(),
            "recurring tasks cannot be epic containers"
        );

        draft.is_epic = false;
        draft.status = Some(TaskStatus::Done);
        assert_eq!(draft.task(), Err(DraftProblem::RecurringStatus));

        draft.status = None;
        draft.add_image(PendingImage {
            name: "receipt.png".to_string(),
            bytes: Arc::new(vec![1]),
        });
        let problem = draft.task().unwrap_err();
        assert_eq!(problem, DraftProblem::RecurringAttachments);
        assert_eq!(
            problem.message(),
            "create the recurring task before adding occurrence attachments"
        );

        draft.schedule = "tomorrow".to_string();
        assert!(draft.task().is_ok());
    }

    #[test]
    fn a_draft_needs_a_title_and_a_readable_schedule() {
        let mut draft = Draft::default();
        assert_eq!(draft.task(), Err(DraftProblem::Title));
        draft.title = "#only-a-label".to_string();
        assert_eq!(draft.task(), Err(DraftProblem::Title));
        draft.title = "Pay rent".to_string();
        draft.schedule = "whenever suits".to_string();
        assert_eq!(draft.task(), Err(DraftProblem::Schedule));
        draft.schedule = "every friday at 09:00".to_string();
        let task = draft.task().unwrap();
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
        let task = draft.task().unwrap();
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
            more: true,
            images: vec![image],
            replaces: None,
        };
        let next = draft.after_created();
        assert_eq!(next.project.as_deref(), Some("key"));
        assert_eq!(next.status, Some(TaskStatus::Active));
        assert_eq!(next.priority, TaskPriority::High);
        assert_eq!(next.labels, ["a"]);
        assert!(next.more);
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
    }

    #[test]
    fn the_schedule_fields_read_their_own_stored_shape() {
        use aven_host::time_input::{parse_available_at_input, parse_due_on_input};
        assert!(parse_available_at_input("2026-12-24 at 09:00").is_ok());
        assert!(parse_available_at_input("2026-12-24T09:00:00Z").is_ok());
        assert_eq!(parse_available_at_input("now").unwrap(), "");
        assert_eq!(parse_due_on_input("2026-12-24").unwrap(), "2026-12-24");
    }
}
