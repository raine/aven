//! The quick-capture launcher's rules and the jobs it starts, free of GPUI so
//! they can be tested on their own.
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result};
use aven_core::choices::{TaskPriority, TaskSource};
use aven_core::db::Database;
use aven_core::ids::{TaskId, new_id};
use aven_core::operations::{
    CreateRecurrenceSeriesParams, TaskAttachmentAddInput, TaskCreationOptions, TaskCreationUndo,
    TaskDraft,
};
use aven_core::workspaces::Workspace;
use aven_host::attachments::lifecycle::LifecyclePolicy;
use aven_host::config::TaskIntakeConfig;
use aven_host::task_intake::{
    TaskIntakeContext, TaskIntakeImage, TaskIntakeResult, delivers_images,
    parsed_output_to_result_with_database, run_task_intake_command_with_images,
};

use crate::adapter::{
    AgentSetup, NavEntry, NewTask, PendingImage, attachment_input, capitalised, intake_task,
};
use crate::draft::{CaretWord, Suggestion, Token, suggestions, title_tokens};

/// A workspace as the launcher offers it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Place {
    pub key: String,
    pub name: String,
    pub projects: Vec<NavEntry>,
    pub labels: Vec<String>,
}

impl Place {
    pub fn project(&self, key: &str) -> Option<&NavEntry> {
        self.projects.iter().find(|project| project.key == key)
    }
}

/// Every workspace the launcher can send to, with the key of the one the main
/// window shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub active: String,
    pub places: Vec<Place>,
    /// Whether an agent command is configured to write tasks.
    pub agent: bool,
}

impl Catalog {
    pub fn place(&self, key: &str) -> Option<&Place> {
        self.places.iter().find(|place| place.key == key)
    }
}

/// Whether `token` names something `place` has. Unknown `#` words are text.
fn known(token: &Token, place: &Place) -> bool {
    match token {
        Token::Label(label) => place.labels.contains(label),
        Token::Priority(_) | Token::Project(_) => true,
    }
}

/// Where the tokens of launcher text are and what each sets.
pub fn tokens(text: &str, place: &Place) -> Vec<(Range<usize>, Token)> {
    title_tokens(text, &place.projects)
        .into_iter()
        .filter(|(_, token)| known(token, place))
        .collect()
}

/// What the tokens of launcher text set. The first project and priority win.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reading {
    pub labels: Vec<String>,
    pub project: Option<String>,
    pub priority: Option<TaskPriority>,
}

pub fn read(text: &str, place: &Place) -> Reading {
    let mut reading = Reading::default();
    for (_, token) in tokens(text, place) {
        match token {
            Token::Label(label) => {
                if !reading.labels.contains(&label) {
                    reading.labels.push(label);
                }
            }
            Token::Project(key) => {
                reading.project.get_or_insert(key);
            }
            Token::Priority(priority) => {
                reading.priority.get_or_insert(priority);
            }
        }
    }
    reading
}

/// The lines of `text` without the token words `drop` matches, each line's
/// spacing tidied. `unescape` turns `\#word` into `#word`.
fn without(
    text: &str,
    place: &Place,
    drop: impl Fn(&Token) -> bool,
    unescape: bool,
) -> Vec<String> {
    text.lines()
        .map(|line| {
            line.split_whitespace()
                .filter_map(|word| {
                    let token = tokens(word, place).pop().map(|(_, token)| token);
                    if token.as_ref().is_some_and(&drop) {
                        return None;
                    }
                    if unescape
                        && let Some(rest) = word.strip_prefix('\\')
                        && rest.starts_with(['#', '!', '@'])
                    {
                        return Some(rest);
                    }
                    Some(word)
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// What the agent reads: the text without the `#label` and `@project` words
/// the launcher applies itself. `!priority` stays for the agent.
pub fn agent_text(text: &str, place: &Place) -> String {
    without(
        text,
        place,
        |token| !matches!(token, Token::Priority(_)),
        false,
    )
    .join("\n")
    .trim()
    .to_string()
}

/// A task made from launcher text without an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainTask {
    pub title: String,
    pub description: String,
    pub priority: TaskPriority,
}

/// The first line with words is the title and the rest are notes, with every
/// token read and taken out. Images alone make a "Screenshot".
pub fn plain_task(text: &str, place: &Place, images: bool) -> Option<PlainTask> {
    let lines = without(text, place, |_| true, true);
    let mut lines = lines.into_iter().skip_while(String::is_empty);
    let title = match lines.next() {
        Some(title) => title,
        None if images => "Screenshot".to_string(),
        None => return None,
    };
    Some(PlainTask {
        title,
        description: lines.collect::<Vec<_>>().join("\n").trim().to_string(),
        priority: read(text, place).priority.unwrap_or(TaskPriority::None),
    })
}

/// What the word being typed could become, from what the workspace has.
pub fn launcher_suggestions(word: &CaretWord, held: &[String], place: &Place) -> Vec<Suggestion> {
    suggestions(word, held, &place.labels, &place.projects)
        .into_iter()
        .filter(|suggestion| known(&suggestion.token, place))
        .collect()
}

/// `first`, then whatever of `then` it lacks.
pub fn merged(first: &[String], then: &[String]) -> Vec<String> {
    let mut labels = first.to_vec();
    for label in then {
        if !labels.contains(label) {
            labels.push(label.clone());
        }
    }
    labels
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    /// The agent writes the task and it is saved without review.
    Create,
    /// The agent drafts the task for the composer.
    Review,
    /// The agent reads Add Task's notes. Add Task checks the result against
    /// its own fields, so images it cannot send are left out, not refused.
    Fill,
    Plain(PlainTask),
}

/// A number for a new job, unique among the launcher's and Add Task's.
pub fn next_job_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// One submission, fixed when it is made: later launcher edits never reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchJob {
    pub id: u64,
    pub mode: LaunchMode,
    pub workspace: String,
    /// A project the agent may override. None leaves it to the agent.
    pub project: Option<String>,
    /// Labels the task gets whatever the agent says.
    pub labels: Vec<String>,
    pub text: String,
    pub images: Vec<PendingImage>,
}

/// A task a job saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub workspace: String,
    pub task: TaskId,
    pub title: String,
    pub reference: String,
    pub project: String,
    pub labels: Vec<String>,
    pub plain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchEvent {
    /// The job runs on its own now, so the launcher can let its input go.
    Accepted(u64),
    /// The job never started, so the launcher keeps its input.
    Refused(u64, String),
    Created(Box<Created>),
    /// A job that was accepted saved nothing.
    Failed {
        title: String,
        body: String,
    },
    /// A draft for the composer, for the workspace with this key.
    Draft(u64, String, Box<NewTask>),
    DraftFailed(u64, String),
}

/// The configured agent command, if there is one.
fn agent_command(config: &TaskIntakeConfig) -> Option<&str> {
    config
        .command
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())
}

/// Whether an agent command is configured, runnable or not.
pub fn has_agent(config: &TaskIntakeConfig) -> bool {
    agent_command(config).is_some()
}

/// The configured agent command and its arguments, as settings show them.
pub fn agent_setup(config: &TaskIntakeConfig) -> Option<AgentSetup> {
    agent_command(config).map(|command| AgentSetup {
        command: command.to_string(),
        args: config.args.clone(),
        runnable: runnable(command),
    })
}

/// Why `job` cannot start, known before anything runs.
pub fn refusal(config: &TaskIntakeConfig, job: &LaunchJob) -> Option<String> {
    if matches!(job.mode, LaunchMode::Plain(_)) {
        return None;
    }
    let Some(command) = agent_command(config) else {
        return Some(
            "Set agent.task_intake.command in Aven\u{2019}s config to use your agent.".to_string(),
        );
    };
    if !runnable(command) {
        return Some(format!(
            "Couldn\u{2019}t start agent.task_intake: `{command}` isn\u{2019}t on PATH. Nothing \
             was sent; your input is still here."
        ));
    }
    if job.mode != LaunchMode::Fill && !job.images.is_empty() && !delivers_images(config) {
        return Some(
            "agent.task_intake has no way to receive images. Set its image_args, or remove \
             the images."
                .to_string(),
        );
    }
    None
}

/// Whether `command` names an executable, by path or on `PATH`.
fn runnable(command: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    let executable = |path: &Path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    if command.contains('/') {
        return executable(Path::new(command));
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| executable(&dir.join(command)))
    })
}

/// What a job needs from the app.
#[derive(Clone)]
pub struct LaunchEnv {
    pub database: Database,
    pub intake: TaskIntakeConfig,
    pub blob_dir: PathBuf,
    pub policy: LifecyclePolicy,
}

/// A reply that is not one valid task.
#[derive(Debug)]
struct InvalidReply;

impl std::fmt::Display for InvalidReply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the agent's reply was not one valid task")
    }
}

/// A recurring task with images, which Aven cannot save.
#[derive(Debug)]
struct RepeatsWithImages {
    title: String,
    schedule: String,
}

impl std::fmt::Display for RepeatsWithImages {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} repeats {}", self.title, self.schedule)
    }
}

impl std::error::Error for RepeatsWithImages {}

/// Runs `job` to its end. A cancelled job's agent stops with its future.
pub async fn run(env: &LaunchEnv, job: LaunchJob) -> LaunchEvent {
    if matches!(job.mode, LaunchMode::Review | LaunchMode::Fill) {
        return match review(env, &job).await {
            Ok(task) => LaunchEvent::Draft(job.id, job.workspace.clone(), Box::new(task)),
            Err(error) if job.mode == LaunchMode::Fill => {
                LaunchEvent::DraftFailed(job.id, explain(&error))
            }
            Err(error) => LaunchEvent::DraftFailed(job.id, review_failure(&error)),
        };
    }
    match create(env, &job).await {
        Ok(created) => LaunchEvent::Created(Box::new(created)),
        Err(error) => failure(&job, &error),
    }
}

async fn workspace(database: &Database, key: &str) -> Result<Workspace> {
    database
        .list_workspaces()
        .await?
        .into_iter()
        .find(|workspace| workspace.key == key)
        .context("That workspace no longer exists")
}

/// The agent's task, with the job's labels and refusing images on a series.
async fn interpret(
    env: &LaunchEnv,
    job: &LaunchJob,
    workspace: &Workspace,
) -> Result<TaskIntakeResult> {
    let context =
        TaskIntakeContext::with_project_hint(&env.database, workspace, job.project.as_deref())
            .await?;
    let sends_images = job.mode != LaunchMode::Fill || delivers_images(&env.intake);
    let images = job
        .images
        .iter()
        .filter(|_| sends_images)
        .map(|image| TaskIntakeImage {
            name: &image.name,
            bytes: &image.bytes,
        })
        .collect::<Vec<_>>();
    let output =
        run_task_intake_command_with_images(&env.intake, &context, &job.text, &images).await?;
    let mut result = parsed_output_to_result_with_database(
        &env.database,
        &context,
        &output,
        TaskSource::Unknown,
    )
    .await
    .context(InvalidReply)?;
    result.task.labels = merged(&job.labels, &result.task.labels);
    if let Some(schedule) = &result.recurrence
        && job.mode != LaunchMode::Fill
        && !job.images.is_empty()
    {
        return Err(RepeatsWithImages {
            title: result.task.title.clone(),
            schedule: crate::draft::schedule_text("", "", Some(schedule)),
        }
        .into());
    }
    Ok(result)
}

async fn review(env: &LaunchEnv, job: &LaunchJob) -> Result<NewTask> {
    let workspace = workspace(&env.database, &job.workspace).await?;
    let result = interpret(env, job, &workspace).await?;
    Ok(NewTask {
        images: job.images.clone(),
        ..intake_task(result)
    })
}

/// Saves the job's task. Aven needs a project, so a task without one goes to
/// the workspace's first. Nothing after the write can fail the job, which
/// would invite saving it twice.
async fn create(env: &LaunchEnv, job: &LaunchJob) -> Result<Created> {
    let database = &env.database;
    let workspace = workspace(database, &job.workspace).await?;
    let (mut draft, recurrence) = match &job.mode {
        LaunchMode::Plain(plain) => (
            TaskDraft {
                title: plain.title.clone(),
                description: plain.description.clone(),
                project: job.project.clone(),
                status: None,
                priority: plain.priority.to_string(),
                source: TaskSource::Unknown,
                labels: job.labels.clone(),
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
            None,
        ),
        _ => {
            let result = interpret(env, job, &workspace).await?;
            (result.task, result.recurrence)
        }
    };
    let projects = database.list_project_items(&workspace.id).await?;
    if draft.project.is_none() {
        draft.project = projects.first().map(|project| project.key.clone());
    }
    draft
        .project
        .as_ref()
        .context("This workspace has no project to add the task to")?;
    let labels = draft.labels.clone();
    let options =
        || TaskCreationOptions::standalone(TaskCreationUndo::None).with_create_missing_labels();
    let task = if let Some(schedule) = recurrence {
        let series = TaskIntakeResult {
            task: draft,
            recurrence: Some(schedule),
        }
        .into_recurrence_draft()
        .context("That schedule does not repeat")?;
        let params = CreateRecurrenceSeriesParams::new(series).with_create_missing_labels();
        database
            .create_recurrence_series(&workspace, params)
            .await?
            .task
    } else if job.images.is_empty() {
        database
            .create_task_with_options(&workspace, draft, options())
            .await?
            .task
    } else {
        let inputs = job
            .images
            .iter()
            .map(|image| TaskAttachmentAddInput {
                attachment_id: new_id(),
                input: attachment_input(image, String::new()),
            })
            .collect();
        database
            .create_task_with_attachments_and_options(
                &workspace,
                &env.blob_dir,
                env.policy,
                draft,
                inputs,
                options(),
            )
            .await?
            .task
    };
    let reference = match database.display_ref_context(&workspace.id).await {
        Ok(context) => context.display_ref(&task),
        Err(_) => String::new(),
    };
    let project = projects
        .iter()
        .find(|project| project.key == task.project_key)
        .map_or_else(|| task.project_key.clone(), |project| project.name.clone());
    Ok(Created {
        workspace: workspace.key,
        task: task.id,
        title: task.title,
        reference,
        project,
        labels,
        plain: matches!(job.mode, LaunchMode::Plain(_)),
    })
}

/// What went wrong, in words for a notification or the launcher.
fn explain(error: &anyhow::Error) -> String {
    if error.downcast_ref::<InvalidReply>().is_some() {
        return "Your agent\u{2019}s reply wasn\u{2019}t a single valid task.".to_string();
    }
    let text = format!("{error:#}");
    if text.contains("task-intake-timeout") {
        "Your agent didn\u{2019}t reply in time.".to_string()
    } else if text.contains("task-intake-command-failed") {
        "Your agent stopped with an error.".to_string()
    } else if text.contains("could not start task intake command") {
        "Couldn\u{2019}t start agent.task_intake.".to_string()
    } else {
        let mut message = capitalised(&error.to_string());
        if !message.ends_with('.') {
            message.push('.');
        }
        message
    }
}

fn failure(job: &LaunchJob, error: &anyhow::Error) -> LaunchEvent {
    if let Some(repeats) = error.downcast_ref::<RepeatsWithImages>() {
        return LaunchEvent::Failed {
            title: "Couldn\u{2019}t create recurring task".to_string(),
            body: format!(
                "\u{201c}{}\u{201d} repeats {}, and recurring tasks can\u{2019}t have images. \
                 Nothing was created.",
                repeats.title, repeats.schedule
            ),
        };
    }
    let title = match job.mode {
        LaunchMode::Plain(_) => "Couldn\u{2019}t add task",
        _ => "Couldn\u{2019}t create task",
    };
    LaunchEvent::Failed {
        title: title.to_string(),
        body: format!("{} Nothing was created.", explain(error)),
    }
}

fn review_failure(error: &anyhow::Error) -> String {
    if let Some(repeats) = error.downcast_ref::<RepeatsWithImages>() {
        return format!(
            "This reads as a recurring task ({}). Remove the image, or attach it after creating \
             the task.",
            repeats.schedule
        );
    }
    format!("{} Edit the text and try again.", explain(error))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn place() -> Place {
        let project = |name: &str, key: &str, prefix: &str| NavEntry {
            name: name.to_string(),
            key: key.to_string(),
            prefix: prefix.to_string(),
            count: 0,
        };
        Place {
            key: "default".to_string(),
            name: "Personal".to_string(),
            projects: vec![
                project("Home", "home", "HOM"),
                project("Work", "work", "WRK"),
            ],
            labels: vec!["bug".to_string(), "errand".to_string()],
        }
    }

    #[test]
    fn only_known_labels_are_tokens() {
        let place = place();
        let reading = read("Fix #bug #unknown @work !high #bug @home", &place);
        assert_eq!(reading.labels, ["bug"]);
        assert_eq!(reading.project.as_deref(), Some("work"));
        assert_eq!(reading.priority, Some(TaskPriority::High));
        let found = tokens("a #unknown #errand", &place);
        assert_eq!(found, [(11..18, Token::Label("errand".to_string()))]);
    }

    #[test]
    fn agent_text_keeps_priority_and_unknown_words() {
        let text = "Call  plumber #errand @home !urgent\n#unknown tag\n\nmore";
        assert_eq!(
            agent_text(text, &place()),
            "Call plumber !urgent\n#unknown tag\n\nmore"
        );
    }

    #[test]
    fn plain_task_splits_title_from_notes() {
        let place = place();
        let task = plain_task(
            "#bug\nFix login !high @work\n\nSteps:\n1. \\#hash",
            &place,
            false,
        )
        .unwrap();
        assert_eq!(task.title, "Fix login");
        assert_eq!(task.description, "Steps:\n1. #hash");
        assert_eq!(task.priority, TaskPriority::High);
        assert_eq!(
            plain_task("#bug @home", &place, true).unwrap().title,
            "Screenshot"
        );
        assert_eq!(plain_task("  ", &place, false), None);
    }

    #[test]
    fn suggestions_never_offer_new_labels() {
        let place = place();
        let word = |word: &str| CaretWord {
            start: 0,
            word: word.to_string(),
        };
        let labels = |word: CaretWord| {
            launcher_suggestions(&word, &[], &place)
                .into_iter()
                .map(|suggestion| suggestion.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(word("#")), ["bug", "errand"]);
        assert!(labels(word("#zzz")).is_empty());
        assert_eq!(labels(word("@w")), ["Work"]);
    }

    #[test]
    fn merged_labels_keep_explicit_ones_first() {
        let first = ["bug".to_string()];
        let then = ["ui".to_string(), "bug".to_string()];
        assert_eq!(merged(&first, &then), ["bug", "ui"]);
    }

    fn job(mode: LaunchMode) -> LaunchJob {
        LaunchJob {
            id: 1,
            mode,
            workspace: "default".to_string(),
            project: None,
            labels: Vec::new(),
            text: "something".to_string(),
            images: Vec::new(),
        }
    }

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::new(4, 4)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn image() -> PendingImage {
        PendingImage {
            name: "shot.png".to_string(),
            bytes: Arc::new(png()),
        }
    }

    #[test]
    fn refusals_leave_plain_tasks_alone() {
        let missing = TaskIntakeConfig::default();
        assert!(!has_agent(&missing));
        let refused = refusal(&missing, &job(LaunchMode::Create)).unwrap();
        assert!(refused.contains("agent.task_intake.command"), "{refused}");
        let plain = LaunchMode::Plain(PlainTask {
            title: "x".to_string(),
            description: String::new(),
            priority: TaskPriority::None,
        });
        assert_eq!(refusal(&missing, &job(plain)), None);

        let absent = TaskIntakeConfig {
            command: Some("aven-no-such-agent".to_string()),
            ..TaskIntakeConfig::default()
        };
        assert!(has_agent(&absent));
        let refused = refusal(&absent, &job(LaunchMode::Review)).unwrap();
        assert!(
            refused.contains("`aven-no-such-agent` isn\u{2019}t on PATH"),
            "{refused}"
        );

        let text_only = TaskIntakeConfig {
            command: Some("/bin/sh".to_string()),
            system_prompt: Some("{input}".to_string()),
            ..TaskIntakeConfig::default()
        };
        assert_eq!(refusal(&text_only, &job(LaunchMode::Create)), None);
        let with_image = LaunchJob {
            images: vec![image()],
            ..job(LaunchMode::Create)
        };
        assert!(
            refusal(&text_only, &with_image)
                .unwrap()
                .contains("image_args")
        );
    }

    /// A database with Home and Work projects and an agent that prints `reply`.
    async fn env(reply: &str) -> (tempfile::TempDir, LaunchEnv) {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("db.sqlite")).await.unwrap();
        let workspace = Workspace::default();
        for name in ["Home", "Work"] {
            database
                .create_project(&workspace, name, None)
                .await
                .unwrap();
        }
        for label in ["bug", "ui"] {
            database.create_label(&workspace, label).await.unwrap();
        }
        let agent = dir.path().join("agent.sh");
        let reply = reply.replace('\'', "'\\''");
        std::fs::write(
            &agent,
            format!("#!/bin/sh\ncat > /dev/null\nprintf '%s' '{reply}'\n"),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&agent).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&agent, permissions).unwrap();
        let env = LaunchEnv {
            database,
            intake: TaskIntakeConfig {
                command: Some(agent.display().to_string()),
                args: Vec::new(),
                ..TaskIntakeConfig::default()
            },
            blob_dir: dir.path().join("blobs"),
            policy: aven_host::config::AttachmentLifecycleConfig::default().policy(),
        };
        (dir, env)
    }

    async fn task_count(env: &LaunchEnv) -> i64 {
        let workspace = Workspace::default();
        let projects = env
            .database
            .list_project_items(&workspace.id)
            .await
            .unwrap();
        projects.iter().map(|project| project.open_count).sum()
    }

    fn created(event: LaunchEvent) -> Created {
        match event {
            LaunchEvent::Created(created) => *created,
            other => panic!("expected a created task, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_leaves_the_project_to_the_agent() {
        let (_dir, env) = env(r#"{"title":"Fix the sink","project":"Home"}"#).await;
        let task = created(run(&env, job(LaunchMode::Create)).await);
        assert_eq!(
            (task.title.as_str(), task.project.as_str()),
            ("Fix the sink", "Home")
        );
        assert!(!task.plain);
        assert!(task.reference.starts_with("HMO-"), "{}", task.reference);
    }

    #[tokio::test]
    async fn the_agent_can_override_a_project_hint() {
        let (_dir, env) = env(r#"{"title":"Fix the sink","project":"Home"}"#).await;
        let hinted = LaunchJob {
            project: Some("work".to_string()),
            ..job(LaunchMode::Create)
        };
        assert_eq!(created(run(&env, hinted).await).project, "Home");

        let (_dir, env) = self::env(r#"{"title":"Plan the offsite"}"#).await;
        let hinted = LaunchJob {
            project: Some("work".to_string()),
            ..job(LaunchMode::Create)
        };
        assert_eq!(created(run(&env, hinted).await).project, "Work");
    }

    #[tokio::test]
    async fn chosen_labels_survive_the_agent() {
        let (_dir, env) = env(r#"{"title":"Fix layout","labels":["ui"]}"#).await;
        let labelled = LaunchJob {
            labels: vec!["bug".to_string()],
            ..job(LaunchMode::Create)
        };
        assert_eq!(created(run(&env, labelled).await).labels, ["bug", "ui"]);
    }

    #[tokio::test]
    async fn recurring_tasks_with_images_save_nothing() {
        let (_dir, env) = env(r#"{"title":"Water plants","repeat":"daily"}"#).await;
        let pictured = LaunchJob {
            images: vec![image()],
            ..job(LaunchMode::Create)
        };
        let LaunchEvent::Failed { title, body } = run(&env, pictured.clone()).await else {
            panic!("a recurring task with images should fail");
        };
        assert_eq!(title, "Couldn\u{2019}t create recurring task");
        assert!(
            body.contains("\u{201c}Water plants\u{201d} repeats"),
            "{body}"
        );
        let review = LaunchJob {
            mode: LaunchMode::Review,
            ..pictured
        };
        let LaunchEvent::DraftFailed(1, message) = run(&env, review).await else {
            panic!("a recurring draft with images should fail");
        };
        assert!(
            message.starts_with("This reads as a recurring task"),
            "{message}"
        );
        assert_eq!(task_count(&env).await, 0);
    }

    #[tokio::test]
    async fn invalid_replies_save_nothing() {
        let (_dir, env) = env("I could not decide").await;
        let LaunchEvent::Failed { body, .. } = run(&env, job(LaunchMode::Create)).await else {
            panic!("an invalid reply should fail");
        };
        assert_eq!(
            body,
            "Your agent\u{2019}s reply wasn\u{2019}t a single valid task. Nothing was created."
        );
        assert_eq!(task_count(&env).await, 0);
    }

    #[tokio::test]
    async fn review_drafts_without_writing() {
        let (_dir, env) = env(r#"{"title":"Fix layout","labels":["ui"]}"#).await;
        let review = LaunchJob {
            labels: vec!["bug".to_string()],
            images: vec![image()],
            ..job(LaunchMode::Review)
        };
        let LaunchEvent::Draft(1, workspace, task) = run(&env, review).await else {
            panic!("review should produce a draft");
        };
        assert_eq!(workspace, "default");
        assert_eq!(task.title, "Fix layout");
        assert_eq!(task.labels, ["bug", "ui"]);
        assert_eq!(task.images.len(), 1);
        assert_eq!(task_count(&env).await, 0);
    }

    #[tokio::test]
    async fn add_task_reads_leave_checks_to_add_task() {
        let (_dir, mut env) = env(r#"{"title":"Water plants","repeat":"daily"}"#).await;
        env.intake.system_prompt = Some("{input}".to_string());
        let fill = LaunchJob {
            images: vec![image()],
            ..job(LaunchMode::Fill)
        };
        assert_eq!(
            refusal(&env.intake, &fill),
            None,
            "an agent without image_args reads the text"
        );
        let LaunchEvent::Draft(1, _, task) = run(&env, fill).await else {
            panic!("a read draft should come back for Add Task to check");
        };
        assert!(task.schedule.contains("day"), "{}", task.schedule);

        let (_dir, env) = self::env("I could not decide").await;
        let LaunchEvent::DraftFailed(1, why) = run(&env, job(LaunchMode::Fill)).await else {
            panic!("an invalid reply should fail");
        };
        assert_eq!(
            why,
            "Your agent\u{2019}s reply wasn\u{2019}t a single valid task."
        );
    }

    #[tokio::test]
    async fn plain_tasks_skip_the_agent() {
        let (_dir, mut env) = env("unused").await;
        env.intake = TaskIntakeConfig::default();
        let plain = LaunchJob {
            labels: vec!["bug".to_string()],
            images: vec![image()],
            ..job(LaunchMode::Plain(PlainTask {
                title: "Broken button".to_string(),
                description: "See screenshot".to_string(),
                priority: TaskPriority::High,
            }))
        };
        let task = created(run(&env, plain).await);
        assert!(task.plain);
        assert_eq!(task.title, "Broken button");
        assert_eq!(
            task.project, "Home",
            "a task without a project joins the first"
        );
        assert_eq!(task.labels, ["bug"]);
        assert_eq!(task_count(&env).await, 1);
    }
}
