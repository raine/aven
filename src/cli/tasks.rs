use std::path::PathBuf;

use clap::{Args, Subcommand};

pub(super) const ADD_EXAMPLES: &str = r#"Examples:
  aven add "Fix login redirect" --status todo --priority high --label bug
  aven add "Write migration guide" --description-file guide.md
  aven add "Daily journal" --repeat daily --repeat-at 09:00 --time-zone Europe/Stockholm

Supply at most one description source: --description, --description-file, or
--description-stdin. --natural uses the title as the complete request and cannot
be combined with structured task fields other than --project.

Defaults:
  Plain tasks use status inbox. Recurring tasks use status todo. Priority is none.
  Recurrence uses the local time zone, today's start date, and a same-day due date.

Scheduling inputs:
  --available-at accepts tomorrow, 2d, next monday at 9am, an ISO date or timestamp.
  --due accepts tomorrow, 2w, next monday, an ISO date, none, or clear.

Recurrence rules:
  daily | weekdays | weekly | fortnightly | monthly | yearly
  every N days | every N weeks | every N months | every N years
  weekly on mon,wed,fri | every N weeks on mon,thu"#;

pub(super) const EDIT_EXAMPLES: &str = r#"Examples:
  aven edit APP-7KQ9 --status active --priority high
  aven edit APP-7KQ9 --available-at tomorrow --due "next monday"
  aven edit APP-7KQ9 --description-file description.md

--available-at accepts natural expressions, ISO dates, and ISO timestamps.
--due accepts natural date expressions, ISO dates, none, or clear. Each scheduling
value conflicts with its corresponding --clear option. Supply at most one
description source: --description, --description-file, or --description-stdin."#;

pub(super) const BULK_UPDATE_EXAMPLES: &str = r#"Examples:
  aven bulk-update --project app --filter-label bug --set-priority high --dry-run
  aven bulk-update --status inbox --set-status backlog

At least one selector is required unless --all is supplied, and at least one
update option is always required. --all only bypasses the selector requirement;
other filters still apply. Preview broad changes with --dry-run before applying."#;

pub(super) const TEXT_EXAMPLES: &str = r#"Safe edit workflow:
  aven text get APP-7KQ9 description --output description.md
  aven text diff APP-7KQ9 description --file description.md
  aven text set APP-7KQ9 description --file description.md --if-sha256 HASH

The hash guard prevents replacing text that changed after it was read. `text
set` requires exactly one input source: --file or --stdin."#;

pub(super) const LIST_HELP: &str = r#"Examples:
  aven list --ready
  aven list --open --project app --label bug
  aven list --upcoming

By default, list shows available, nondeleted tasks of every status, newest
updates first. --ready and --blocked are mutually exclusive. Dependency filters
select open tasks and cannot be combined with --all or --deleted. --upcoming and
--overdue select nondeleted, open tasks even when --all is supplied."#;

pub(super) const NOTE_HELP: &str = r#"Examples:
  aven note APP-7KQ9 "Short update"
  aven note APP-7KQ9 --file handoff.md
  aven note APP-7KQ9 --stdin

Supply exactly one text source: the TEXT argument, --file, or --stdin."#;

#[derive(Args)]
pub(crate) struct AddArgs {
    /// Task title, or natural-language request with --natural
    pub(crate) title: String,
    /// Assign the task to a project by key or name; otherwise infer it
    #[arg(long)]
    pub(crate) project: Option<String>,
    /// Set the Markdown description from this argument
    #[arg(long)]
    pub(crate) description: Option<String>,
    /// Read the Markdown description from a UTF-8 file
    #[arg(long)]
    pub(crate) description_file: Option<PathBuf>,
    /// Read the Markdown description from standard input
    #[arg(long)]
    pub(crate) description_stdin: bool,
    /// Set priority: none, low, medium, high, or urgent
    #[arg(long, default_value = "none")]
    pub(crate) priority: String,
    /// Set status: inbox, backlog, todo, active, done, or canceled; --repeat excludes terminal values
    #[arg(long)]
    pub(crate) status: Option<String>,
    /// Add a label; repeat for multiple labels
    #[arg(long)]
    pub(crate) label: Vec<String>,
    /// Set metadata; repeat for multiple fields
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    #[arg(long, help = "Create the task as an epic container")]
    pub(crate) epic: bool,
    /// Parse the title with the configured task-intake agent
    #[arg(
        long,
        conflicts_with_all = [
            "description",
            "description_file",
            "description_stdin",
            "priority",
            "status",
            "label",
            "metadata",
            "epic",
            "available_at",
            "due",
            "repeat",
            "repeat_at",
            "repeat_due",
            "time_zone",
            "repeat_start_on"
        ]
    )]
    pub(crate) natural: bool,
    /// Defer availability until a date, time, or natural expression
    #[arg(long, value_name = "WHEN")]
    pub(crate) available_at: Option<String>,
    /// Set a deadline from a date or natural expression
    #[arg(long, value_name = "WHEN")]
    pub(crate) due: Option<String>,
    /// Create a recurring series using the documented rule grammar
    #[arg(long, value_name = "RULE")]
    pub(crate) repeat: Option<String>,
    /// Set a 24-hour local availability time, or none for start of day
    #[arg(long, value_name = "HH:MM|none")]
    pub(crate) repeat_at: Option<String>,
    /// Give each occurrence a same-day deadline, or no deadline
    #[arg(long, value_name = "same-day|none")]
    pub(crate) repeat_due: Option<String>,
    /// Evaluate recurrence dates and times in this IANA time zone
    #[arg(long, value_name = "IANA_ZONE")]
    pub(crate) time_zone: Option<String>,
    /// Anchor the recurrence on this date; defaults to today in its time zone
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub(crate) repeat_start_on: Option<String>,
}

#[derive(Args)]
pub(crate) struct ShowArgs {
    /// Task ref, such as APP-7KQ9 or an unambiguous suffix
    pub(crate) task_ref: String,
    /// Include descriptions, notes, relationships, metadata, and attachments
    #[arg(long)]
    pub(crate) full: bool,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct ContextArgs {
    /// Task ref, such as APP-7KQ9 or an unambiguous suffix
    pub(crate) task_ref: String,
    /// Print every note body in full
    #[arg(long)]
    pub(crate) full: bool,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct ListArgs {
    /// Select a project by key or name; omit the value to infer from cwd
    #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "", conflicts_with = "all_projects")]
    pub(crate) project: Option<String>,
    /// List across all projects, ignoring the current directory
    #[arg(short = 'A', long)]
    pub(crate) all_projects: bool,
    /// Restrict tasks to one status: inbox, backlog, todo, active, done, or canceled
    #[arg(long)]
    pub(crate) status: Option<String>,
    /// Restrict tasks to one priority: none, low, medium, high, or urgent
    #[arg(long)]
    pub(crate) priority: Option<String>,
    /// Restrict tasks to those carrying this label
    #[arg(long)]
    pub(crate) label: Option<String>,
    /// Require an exact metadata key and value; repeat to require all
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    /// Require a metadata key to be present; repeat to require all
    #[arg(long, value_name = "KEY")]
    pub(crate) has_metadata: Vec<String>,
    /// Require a metadata key to be absent; repeat to require all
    #[arg(long, value_name = "KEY")]
    pub(crate) missing_metadata: Vec<String>,
    /// Include all nonterminal statuses: inbox, backlog, todo, and active
    #[arg(
        long,
        conflicts_with_all = ["status", "all", "deleted", "upcoming"]
    )]
    pub(crate) open: bool,
    /// Include deleted tasks, except with --upcoming or --overdue
    #[arg(long)]
    pub(crate) all: bool,
    /// Show only soft-deleted tasks
    #[arg(long)]
    pub(crate) deleted: bool,
    /// Show open, available, unblocked, non-epic tasks
    #[arg(long)]
    pub(crate) ready: bool,
    /// Show open tasks with incomplete dependencies
    #[arg(long)]
    pub(crate) blocked: bool,
    /// Show epic containers
    #[arg(long)]
    pub(crate) epics: bool,
    /// Show nondeleted, open tasks that become available in the future
    #[arg(long)]
    pub(crate) upcoming: bool,
    /// Show nondeleted, open tasks whose due date has passed
    #[arg(long)]
    pub(crate) overdue: bool,
    #[arg(long, help = "Show individual recurring occurrences")]
    pub(crate) expand_recurring: bool,
    #[arg(
        long,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        help = "Maximum result count (must be at least 1)"
    )]
    pub(crate) limit: Option<usize>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct TaskSearchArgs {
    /// One or more search terms matched against task text
    pub(crate) query: Vec<String>,
    #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "", conflicts_with = "all_projects", help = "Restrict matches to a project by key or name")]
    pub(crate) project: Option<String>,
    /// Search across all projects, ignoring the current directory
    #[arg(short = 'A', long)]
    pub(crate) all_projects: bool,
    /// Require an exact metadata key and value; repeat to require all
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    /// Require a metadata key to be present; repeat to require all
    #[arg(long, value_name = "KEY")]
    pub(crate) has_metadata: Vec<String>,
    /// Require a metadata key to be absent; repeat to require all
    #[arg(long, value_name = "KEY")]
    pub(crate) missing_metadata: Vec<String>,
    #[arg(
        long,
        default_value_t = 50,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        help = "Maximum result count (must be at least 1)"
    )]
    pub(crate) limit: usize,
    #[arg(long, help = "Include deleted tasks")]
    pub(crate) all: bool,
    #[arg(long, help = "Show individual recurring occurrences")]
    pub(crate) expand_recurring: bool,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct BulkUpdateArgs {
    /// Match tasks in this project
    #[arg(long)]
    pub(crate) project: Option<String>,
    /// Match tasks with this status: inbox, backlog, todo, active, done, or canceled
    #[arg(long)]
    pub(crate) status: Option<String>,
    /// Match tasks with this priority: none, low, medium, high, or urgent
    #[arg(long)]
    pub(crate) priority: Option<String>,
    /// Match tasks carrying this label
    #[arg(long)]
    pub(crate) filter_label: Option<String>,
    /// Allow running without a selector; other filters still apply
    #[arg(long)]
    pub(crate) all: bool,
    /// Allow soft-deleted tasks to match the filters
    #[arg(long)]
    pub(crate) include_deleted: bool,
    /// Preview matching tasks and changes without writing them
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Set status on matches: inbox, backlog, todo, active, done, or canceled
    #[arg(long)]
    pub(crate) set_status: Option<String>,
    /// Set priority on matches: none, low, medium, high, or urgent
    #[arg(long)]
    pub(crate) set_priority: Option<String>,
    /// Move every matched task to this project
    #[arg(long)]
    pub(crate) set_project: Option<String>,
    /// Add a label to every matched task; repeat for multiple labels
    #[arg(long)]
    pub(crate) label: Vec<String>,
    /// Remove a label from every matched task; repeat for multiple labels
    #[arg(long)]
    pub(crate) remove_label: Vec<String>,
    /// Set metadata on every matched task; repeat for multiple fields
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    /// Remove metadata from every matched task; repeat for multiple fields
    #[arg(long, value_name = "KEY")]
    pub(crate) remove_metadata: Vec<String>,
}

#[derive(Args)]
pub(crate) struct PrimeArgs {
    /// Restrict agent context to a project by key or name
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(
        long,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        help = "Maximum result count (must be at least 1)"
    )]
    pub(crate) limit: Option<usize>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct TaskEditArgs {
    /// Task ref, such as APP-7KQ9 or an unambiguous suffix
    pub(crate) task_ref: String,
    /// Replace the task title
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// Replace the Markdown description from this argument
    #[arg(long)]
    pub(crate) description: Option<String>,
    /// Replace the Markdown description from a UTF-8 file
    #[arg(long)]
    pub(crate) description_file: Option<PathBuf>,
    /// Replace the Markdown description from standard input
    #[arg(long)]
    pub(crate) description_stdin: bool,
    /// Move the task to a project by key or name
    #[arg(long)]
    pub(crate) project: Option<String>,
    /// Set status: inbox, backlog, todo, active, done, or canceled
    #[arg(long)]
    pub(crate) status: Option<String>,
    /// Set priority: none, low, medium, high, or urgent
    #[arg(long)]
    pub(crate) priority: Option<String>,
    /// Defer availability until a date, time, or natural expression
    #[arg(long, value_name = "WHEN")]
    pub(crate) available_at: Option<String>,
    /// Remove the availability date and make the task immediately available
    #[arg(long)]
    pub(crate) clear_available_at: bool,
    /// Set a deadline from a date or natural expression
    #[arg(long, value_name = "WHEN")]
    pub(crate) due: Option<String>,
    /// Remove the task deadline
    #[arg(long)]
    pub(crate) clear_due: bool,
    /// Enable with on, true, or 1; disable with off, false, or 0
    #[arg(long, value_name = "on|off")]
    pub(crate) epic: Option<String>,
    /// Add a label; repeat for multiple labels
    #[arg(long)]
    pub(crate) label: Vec<String>,
    /// Remove a label; repeat for multiple labels
    #[arg(long)]
    pub(crate) remove_label: Vec<String>,
    /// Set metadata; repeat for multiple fields
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    /// Remove metadata by key; repeat for multiple fields
    #[arg(long, value_name = "KEY")]
    pub(crate) remove_metadata: Vec<String>,
}

#[derive(Args)]
pub(crate) struct NoteArgs {
    /// Task ref to receive the note
    pub(crate) task_ref: String,
    /// Note text; omit when using --file or --stdin
    pub(crate) text: Option<String>,
    /// Read note text from a UTF-8 file
    #[arg(long)]
    pub(crate) file: Option<PathBuf>,
    /// Read note text from standard input
    #[arg(long)]
    pub(crate) stdin: bool,
}

#[derive(Args)]
pub(crate) struct NoteDeleteArgs {
    /// Task ref containing the note
    pub(crate) task_ref: String,
    /// Exact note ID printed by `aven show --full`
    pub(crate) note_id: String,
}

#[derive(Args)]
pub(crate) struct NoteShowArgs {
    /// Task ref containing the notes
    pub(crate) task_ref: String,
    /// Exact note IDs printed by `aven context` or `aven show --full`
    #[arg(required = true)]
    pub(crate) note_ids: Vec<String>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct MoveArgs {
    /// Task refs in the active source workspace
    #[arg(required = true)]
    pub(crate) task_refs: Vec<String>,
    /// Existing destination workspace by name or key
    #[arg(long, value_name = "WORKSPACE")]
    pub(crate) to_workspace: String,
    /// Existing project in the destination workspace
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: String,
}

#[derive(Args)]
pub(crate) struct RefArgs {
    /// Task ref, such as APP-7KQ9 or an unambiguous suffix
    pub(crate) task_ref: String,
}

#[derive(Args)]
pub(crate) struct TextCommand {
    #[command(subcommand)]
    pub(crate) command: TextSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum TextSubcommand {
    /// Read a long text field
    Get(TextGetArgs),
    /// Compare a long text field with a file
    Diff(TextDiffArgs),
    /// Update a long text field safely
    #[command(after_long_help = TEXT_EXAMPLES)]
    Set(TextSetArgs),
}

#[derive(Args)]
pub(crate) struct TextGetArgs {
    /// Task ref to read
    pub(crate) task_ref: String,
    /// Long text field; currently description
    pub(crate) field: String,
    /// Print only field bytes when --output is omitted
    #[arg(long)]
    pub(crate) raw: bool,
    /// Write field bytes to this file while printing the hash
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct TextDiffArgs {
    /// Task ref to compare
    pub(crate) task_ref: String,
    /// Long text field; currently description
    pub(crate) field: String,
    /// UTF-8 file containing the proposed value
    #[arg(long)]
    pub(crate) file: PathBuf,
}

#[derive(Args)]
pub(crate) struct TextSetArgs {
    /// Task ref to update
    pub(crate) task_ref: String,
    /// Long text field; currently description
    pub(crate) field: String,
    /// Read the replacement value from a UTF-8 file
    #[arg(long)]
    pub(crate) file: Option<PathBuf>,
    /// Read the replacement value from standard input
    #[arg(long)]
    pub(crate) stdin: bool,
    /// Require the stored value to match this SHA-256 before replacing it
    #[arg(long)]
    pub(crate) if_sha256: String,
}
