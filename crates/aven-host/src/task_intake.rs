use crate::ids::WorkspaceId;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::choices::{PRIORITIES, TaskPriority, TaskSource};
use crate::config::TaskIntakeConfig;
use crate::operations::TaskDraft;
use crate::query::ProjectListItem;
use crate::workspaces::Workspace;
use aven_core::operations::RecurrenceSeriesDraft;

#[derive(Debug, Deserialize)]
struct ParsedTaskPayload {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    available_at: Option<String>,
    #[serde(default)]
    due_on: Option<String>,
    #[serde(default)]
    repeat: Option<String>,
    #[serde(default)]
    repeat_at: Option<String>,
    #[serde(default)]
    repeat_due: Option<String>,
    #[serde(default)]
    time_zone: Option<String>,
    #[serde(default)]
    repeat_start_on: Option<String>,
}

pub struct TaskIntakeResult {
    pub task: TaskDraft,
    pub recurrence: Option<aven_core::recurrence::RecurrenceSchedule>,
}

impl TaskIntakeResult {
    pub fn into_recurrence_draft(self) -> Option<RecurrenceSeriesDraft> {
        let schedule = self.recurrence?;
        Some(RecurrenceSeriesDraft {
            title: self.task.title,
            description: self.task.description,
            project: self.task.project.unwrap_or_default(),
            priority: self.task.priority,
            initial_status: self.task.status.unwrap_or_else(|| "todo".to_string()),
            labels: self.task.labels,
            metadata: self.task.metadata,
            schedule,
        })
    }
}

/// An image submitted with intake text for the agent to interpret.
pub struct TaskIntakeImage<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
}

pub struct TaskIntakeContext {
    pub workspace_id: WorkspaceId,
    pub fixed_project: Option<String>,
    pub inferred_project: Option<String>,
    pub projects: Vec<ProjectListItem>,
    pub labels: Vec<String>,
}

impl TaskIntakeContext {
    pub async fn load_with_database(
        database: &Database,
        workspace: &Workspace,
        project: Option<&str>,
    ) -> Result<Self> {
        let config = if project.is_some() {
            crate::config::AppConfig::default()
        } else {
            crate::config::AppConfig::load()?
        };
        let routing = crate::routing::InvocationRouting::new(&config);
        Self::load_with_routing(database, workspace, project, &routing).await
    }

    /// Context whose `hint` is only a fallback the agent may override. Nothing
    /// is inferred from the process, so no hint leaves the project to the agent.
    pub async fn with_project_hint(
        database: &Database,
        workspace: &Workspace,
        hint: Option<&str>,
    ) -> Result<Self> {
        let inferred_project = match hint {
            Some(hint) => Some(
                database
                    .resolve_existing_project(&workspace.id, hint)
                    .await?
                    .key,
            ),
            None => None,
        };
        Ok(Self {
            workspace_id: workspace.id.clone(),
            fixed_project: None,
            inferred_project,
            projects: database.list_project_items(&workspace.id).await?,
            labels: database.list_labels(&workspace.id, None).await?,
        })
    }

    pub async fn load_with_routing(
        database: &Database,
        workspace: &Workspace,
        project: Option<&str>,
        routing: &crate::routing::InvocationRouting<'_>,
    ) -> Result<Self> {
        let (fixed_project, inferred_project) = match project {
            Some(project) => {
                let project = database
                    .resolve_existing_project(&workspace.id, project)
                    .await?
                    .key;
                (Some(project.clone()), Some(project))
            }
            None => (
                None,
                crate::projects::inferred_project_key_for_add_with_routing(
                    database, workspace, routing,
                )
                .await?,
            ),
        };
        let projects = database.list_project_items(&workspace.id).await?;
        let labels = database.list_labels(&workspace.id, None).await?;
        Ok(Self {
            workspace_id: workspace.id.clone(),
            fixed_project,
            inferred_project,
            projects,
            labels,
        })
    }
}

pub async fn run_task_intake_command(
    config: &TaskIntakeConfig,
    context: &TaskIntakeContext,
    input: &str,
) -> Result<String> {
    run_task_intake_command_with_images(config, context, input, &[]).await
}

/// Runs the agent with `images` staged as files in a private directory that
/// lives only as long as the command. The prompt's `{images}` lists their
/// paths, and `image_args` passes them as arguments.
pub async fn run_task_intake_command_with_images(
    config: &TaskIntakeConfig,
    context: &TaskIntakeContext,
    input: &str,
    images: &[TaskIntakeImage<'_>],
) -> Result<String> {
    let command = config
        .command
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context(
            "error task-intake-command-required hint=\"configure agent.task_intake.command\"",
        )?;
    let template = task_intake_template(config);
    if !images.is_empty() && !delivers_images(config) {
        bail!(
            "error task-intake-images-unsupported hint=\"set agent.task_intake.image_args or use {{images}} in agent.task_intake.system_prompt\""
        );
    }
    let staged = stage_images(images)?;
    let paths = staged
        .as_ref()
        .map_or(&[][..], |(_, paths)| paths.as_slice());
    let prompt = expand_task_intake_prompt(template, context, input, paths);
    let prompt_arg = config.args.iter().any(|arg| arg.contains("{prompt}"));
    let mut args = Vec::new();
    if let Some((dir, paths)) = &staged {
        let dir = dir.path().display().to_string();
        let each = config.image_args.iter().any(|arg| arg.contains("{image}"));
        let rounds = if each { paths.len() } else { 1 };
        for path in paths.iter().take(rounds) {
            let path = path.display().to_string();
            args.extend(
                config
                    .image_args
                    .iter()
                    .map(|arg| arg.replace("{image_dir}", &dir).replace("{image}", &path)),
            );
        }
    }
    args.extend(
        config
            .args
            .iter()
            .map(|arg| arg.replace("{prompt}", &prompt).replace("{input}", input)),
    );
    let stdin = if prompt_arg {
        Stdio::null()
    } else {
        Stdio::piped()
    };
    let mut child = Command::new(command)
        .args(args)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("could not start task intake command {command}"))?;
    if !prompt_arg {
        let mut stdin = child
            .stdin
            .take()
            .context("could not open task intake stdin")?;
        tokio::spawn(async move {
            let _ = stdin.write_all(prompt.as_bytes()).await;
        });
    }
    let wait = child.wait_with_output();
    let duration = Duration::from_secs(config.timeout_seconds.unwrap_or(45).max(1));
    let output = timeout(duration, wait)
        .await
        .context("error task-intake-timeout")?
        .context("task intake command failed")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "error task-intake-command-failed status={} stderr={}",
            output.status,
            stderr.trim()
        );
    }
    String::from_utf8(output.stdout).context("task intake output was not utf-8")
}

/// Whether `config` gives the agent submitted images, through `image_args` or
/// a prompt that lists them.
pub fn delivers_images(config: &TaskIntakeConfig) -> bool {
    !config.image_args.is_empty() || task_intake_template(config).contains("{images}")
}

fn task_intake_template(config: &TaskIntakeConfig) -> &str {
    config
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_task_intake_system_prompt())
}

/// Writes `images` into a fresh private directory, removed when the returned
/// handle drops. File names come from the index, never from the given name.
fn stage_images(
    images: &[TaskIntakeImage<'_>],
) -> Result<Option<(tempfile::TempDir, Vec<PathBuf>)>> {
    if images.is_empty() {
        return Ok(None);
    }
    let dir = tempfile::Builder::new()
        .prefix("aven-intake-")
        .tempdir()
        .context("could not stage task intake images")?;
    let mut paths = Vec::with_capacity(images.len());
    for (index, image) in images.iter().enumerate() {
        let extension = std::path::Path::new(image.name)
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| {
                !extension.is_empty()
                    && extension.len() <= 8
                    && extension.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .unwrap_or("png")
            .to_ascii_lowercase();
        let path = dir.path().join(format!("image-{}.{extension}", index + 1));
        std::fs::write(&path, image.bytes).context("could not stage task intake images")?;
        paths.push(path);
    }
    Ok(Some((dir, paths)))
}

fn default_task_intake_system_prompt() -> &'static str {
    "You turn raw task intake text into one Aven task payload.\n\n\
Return only JSON with this shape:\n\
{\"title\":\"task title\",\"description\":\"optional durable context\",\"project\":\"optional project key or name\",\"priority\":\"none|low|medium|high|urgent\",\"labels\":[\"existing-label\"],\"available_at\":\"optional one-off defer expression\",\"due_on\":\"optional one-off deadline\",\"repeat\":\"optional recurrence rule\",\"repeat_at\":\"optional HH:MM\",\"repeat_due\":\"same-day|none\",\"time_zone\":\"optional IANA zone\",\"repeat_start_on\":\"optional YYYY-MM-DD\"}\n\n\
Rules:\n\
- The title is required and should be concise.\n\
- Prefer a concise imperative task title that reads like an existing Aven task.\n\
- Start with a capitalized action verb when it reads naturally.\n\
- Include enough context to distinguish the task from nearby work.\n\
- Keep meaningful casing for names, acronyms, file names, flags, and code identifiers.\n\
- Use project only when the text clearly names one of the available projects.\n\
- When Selected project is not none, use that project regardless of the raw text.\n\
- Use only existing labels.\n\
- Set available_at only when a one-off task should not be worked before a stated time. Preserve relative expressions such as tomorrow for Aven to resolve.\n\
- Set due_on only for a one-off deadline or day by which work should be complete. Use date expressions without times.\n\
- Keep available_at and due_on independent when the input states both.\n\
- Set repeat only for clear recurrence intent such as daily, every 3 days, yearly, or every Monday and Thursday. Ambiguous timing remains one-off.\n\
- Use natural repeat rules such as daily, every 3 days, weekdays, monthly, every 3 months, yearly, every 2 years, fortnightly, every Friday, every 3 weeks, or every 4 weeks on Monday and Thursday.\n\
- For recurrence, omit available_at and due_on. Set repeat_at only for a stated recurring availability time, repeat_due only for an explicit same-day or no-due policy, time_zone only for an explicit IANA zone, and repeat_start_on only for an explicit YYYY-MM-DD start date.\n\
- Recurrence defaults are no availability time, same-day due, the local time zone, and today in that zone.\n\
- Omit optional fields when they do not apply.\n\
- Put durable context in description when helpful.\n\
- When images are attached, look at them and let them inform the task.\n\n\
Use only these priorities: {priorities}.\n\n\
Selected project: {selected_project}\n\n\
Inferred project: {inferred_project}\n\n\
Available projects:\n{projects}\n\n\
Available labels:\n{labels}\n\n\
Attached images:\n{images}\n\n\
Raw intake text:\n{input}\n"
}

fn expand_task_intake_prompt(
    template: &str,
    context: &TaskIntakeContext,
    input: &str,
    images: &[PathBuf],
) -> String {
    let images = if images.is_empty() {
        "(none)".to_string()
    } else {
        images
            .iter()
            .map(|path| format!("- {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    template
        .replace("{priorities}", &PRIORITIES.join(", "))
        .replace(
            "{selected_project}",
            context.fixed_project.as_deref().unwrap_or("none"),
        )
        .replace(
            "{inferred_project}",
            context.inferred_project.as_deref().unwrap_or("none"),
        )
        .replace("{projects}", &task_intake_projects_prompt(context))
        .replace("{labels}", &task_intake_labels_prompt(context))
        .replace("{images}", &images)
        .replace("{input}", input)
}

fn task_intake_projects_prompt(context: &TaskIntakeContext) -> String {
    context
        .projects
        .iter()
        .map(|project| format!("- {} ({})", project.key, project.name))
        .collect::<Vec<_>>()
        .join("\n")
}

fn task_intake_labels_prompt(context: &TaskIntakeContext) -> String {
    if context.labels.is_empty() {
        "(none)".to_string()
    } else {
        context
            .labels
            .iter()
            .map(|label| format!("- {label}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub async fn parsed_output_to_result_with_database(
    database: &Database,
    context: &TaskIntakeContext,
    output: &str,
    source: TaskSource,
) -> Result<TaskIntakeResult> {
    let json = extract_json(output).context("error task-intake-json-missing")?;
    let parsed: ParsedTaskPayload =
        serde_json::from_str(json).context("error task-intake-json-invalid")?;
    let title = parsed.title.trim();
    if title.is_empty() {
        bail!("error task-intake-title-required");
    }
    let priority = parsed.priority.unwrap_or_else(|| "none".to_string());
    TaskPriority::parse(&priority)?;
    let project = if let Some(project) = &context.fixed_project {
        Some(project.clone())
    } else if let Some(project) = parsed
        .project
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(
            database
                .resolve_existing_project(&context.workspace_id, project)
                .await?
                .key,
        )
    } else {
        context.inferred_project.clone()
    };
    let labels = database
        .resolve_labels(&context.workspace_id, &parsed.labels)
        .await?;
    let repeat = optional_text(parsed.repeat.as_deref());
    let recurrence_options = [
        optional_text(parsed.repeat_at.as_deref()),
        optional_text(parsed.repeat_due.as_deref()),
        optional_text(parsed.time_zone.as_deref()),
        optional_text(parsed.repeat_start_on.as_deref()),
    ];
    if repeat.is_none() && recurrence_options.iter().any(Option::is_some) {
        bail!(
            "error task-intake-recurrence-rule-required hint=\"set repeat when recurrence scheduling fields are present\""
        );
    }
    let recurrence = repeat
        .map(|repeat| {
            let rule = crate::recurrence_input::canonical_rule_input(repeat)?;
            let Some(rule) = rule else {
                if recurrence_options.iter().any(Option::is_some) {
                    bail!(
                        "error task-intake-recurrence-rule-required hint=\"set repeat to a recurrence rule when recurrence scheduling fields are present\""
                    );
                }
                return Ok(None);
            };
            crate::recurrence_input::recurrence_schedule(
                &rule,
                recurrence_options[0],
                recurrence_options[1],
                recurrence_options[2],
                recurrence_options[3],
                crate::recurrence_input::RecurrenceClock::system(),
            )
            .map(Some)
        })
        .transpose()
        .map_err(|error| {
            let context = format!("error task-intake-recurrence-invalid: {error}");
            error.context(context)
        })?
        .flatten();
    let description = parsed.description.trim().to_string();
    let available_at = parsed
        .available_at
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(crate::time_input::parse_available_at_input)
        .transpose()?;
    let due_on = parsed
        .due_on
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(crate::time_input::parse_due_on_input)
        .transpose()?;
    if recurrence.is_some() && (available_at.is_some() || due_on.is_some()) {
        bail!(
            "error task-intake-recurrence-absolute-time-conflict hint=\"use repeat_at and repeat_due for recurring tasks, not available_at or due_on\""
        );
    }
    Ok(TaskIntakeResult {
        task: TaskDraft {
            title: title.to_string(),
            description,
            project,
            status: recurrence.as_ref().map(|_| "todo".to_string()),
            priority,
            source,
            labels,
            metadata: Vec::new(),
            available_at,
            due_on,
            is_epic: false,
        },
        recurrence,
    })
}

fn optional_text(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn extract_json(output: &str) -> Option<&str> {
    let trimmed = output.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    if let Some(start) = trimmed.find("```json") {
        let body = &trimmed[start + "```json".len()..];
        if let Some(end) = body.find("```") {
            return Some(body[..end].trim());
        }
    }
    if let Some(start) = trimmed.find('{')
        && let Some(end) = trimmed.rfind('}')
        && start < end
    {
        return Some(&trimmed[start..=end]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_json() {
        assert_eq!(
            extract_json("```json\n{\"title\":\"x\"}\n```").unwrap(),
            "{\"title\":\"x\"}"
        );
    }

    fn shell(script: &str, timeout_seconds: u64) -> TaskIntakeConfig {
        TaskIntakeConfig {
            command: Some("sh".to_string()),
            args: vec!["-c".to_string(), script.to_string()],
            timeout_seconds: Some(timeout_seconds),
            ..TaskIntakeConfig::default()
        }
    }

    /// A command that records its arguments in `dir/args`, then runs `body`
    /// with `$log` set to `dir`.
    #[cfg(unix)]
    fn script(dir: &std::path::Path, body: &str) -> TaskIntakeConfig {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("agent.sh");
        let log = dir.display();
        std::fs::write(
            &path,
            format!("#!/bin/sh\nlog='{log}'\nprintf '%s\\n' \"$@\" > \"$log/args\"\n{body}\n"),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        TaskIntakeConfig {
            command: Some(path.display().to_string()),
            args: Vec::new(),
            timeout_seconds: Some(5),
            ..TaskIntakeConfig::default()
        }
    }

    fn empty_context() -> TaskIntakeContext {
        TaskIntakeContext {
            workspace_id: Workspace::default().id,
            fixed_project: None,
            inferred_project: None,
            projects: Vec::new(),
            labels: Vec::new(),
        }
    }

    async fn failure(config: &TaskIntakeConfig, images: &[TaskIntakeImage<'_>]) -> String {
        let error = run_task_intake_command_with_images(config, &empty_context(), "x", images)
            .await
            .unwrap_err();
        format!("{error:#}")
    }

    #[tokio::test]
    async fn task_intake_reports_command_failures() {
        let failed = failure(&shell("echo boom >&2; exit 3", 5), &[]).await;
        assert!(failed.contains("task-intake-command-failed"), "{failed}");
        assert!(failed.contains("boom"), "{failed}");
        let slow = failure(&shell("sleep 5", 1), &[]).await;
        assert!(slow.contains("task-intake-timeout"), "{slow}");
        let missing = TaskIntakeConfig {
            command: Some("/nonexistent/aven-intake-agent".to_string()),
            ..TaskIntakeConfig::default()
        };
        let missing = failure(&missing, &[]).await;
        assert!(missing.contains("could not start"), "{missing}");
    }

    #[tokio::test]
    async fn task_intake_refuses_images_it_cannot_deliver() {
        let config = TaskIntakeConfig {
            command: Some("/nonexistent/aven-intake-agent".to_string()),
            system_prompt: Some("Text only: {input}".to_string()),
            ..TaskIntakeConfig::default()
        };
        let image = TaskIntakeImage {
            name: "shot.png",
            bytes: b"png",
        };
        let refused = failure(&config, &[image]).await;
        assert!(
            refused.contains("task-intake-images-unsupported"),
            "{refused}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn task_intake_stages_images_for_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let config = TaskIntakeConfig {
            image_args: vec!["--image".to_string(), "{image}".to_string()],
            ..script(
                dir.path(),
                "for arg in \"$@\"; do [ -f \"$arg\" ] && cat \"$arg\" >> \"$log/bytes\"; done\n\
                 cat > \"$log/prompt\"\nprintf '{\"title\":\"Seen\"}'",
            )
        };
        let images = [
            TaskIntakeImage {
                name: "first.JPG",
                bytes: b"one",
            },
            TaskIntakeImage {
                name: "../../escape",
                bytes: b"two",
            },
        ];
        let output = run_task_intake_command_with_images(&config, &empty_context(), "x", &images)
            .await
            .unwrap();
        assert_eq!(output, "{\"title\":\"Seen\"}");
        assert_eq!(std::fs::read(dir.path().join("bytes")).unwrap(), b"onetwo");
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        let args = args.lines().collect::<Vec<_>>();
        assert_eq!(args.len(), 4, "{args:?}");
        assert_eq!([args[0], args[2]], ["--image", "--image"]);
        let (first, second) = (PathBuf::from(args[1]), PathBuf::from(args[3]));
        assert!(first.ends_with("image-1.jpg"), "{first:?}");
        assert!(second.ends_with("image-2.png"), "{second:?}");
        assert_eq!(first.parent(), second.parent());
        let prompt = std::fs::read_to_string(dir.path().join("prompt")).unwrap();
        assert!(
            prompt.contains(&format!("- {}", first.display())),
            "{prompt}"
        );
        assert!(
            prompt.contains(&format!("- {}", second.display())),
            "{prompt}"
        );
        assert!(!first.exists(), "staged images are removed afterwards");
        assert!(!first.parent().unwrap().exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn task_intake_passes_the_image_directory_once() {
        let dir = tempfile::tempdir().unwrap();
        let config = TaskIntakeConfig {
            image_args: vec!["--add-dir".to_string(), "{image_dir}".to_string()],
            args: vec!["{prompt}".to_string()],
            ..script(dir.path(), "echo '{}'")
        };
        let images = [
            TaskIntakeImage {
                name: "a.png",
                bytes: b"a",
            },
            TaskIntakeImage {
                name: "b.png",
                bytes: b"b",
            },
        ];
        run_task_intake_command_with_images(&config, &empty_context(), "x", &images)
            .await
            .unwrap();
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        let args = args.lines().collect::<Vec<_>>();
        assert_eq!(args.iter().filter(|arg| **arg == "--add-dir").count(), 1);
        assert_eq!(
            args[0], "--add-dir",
            "image arguments precede the configured ones"
        );
        assert!(args[1].contains("aven-intake-"), "{args:?}");
        assert!(
            args[2].starts_with("You turn raw task intake text"),
            "{args:?}"
        );
    }

    #[tokio::test]
    async fn project_hint_is_a_fallback_the_agent_can_override() {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("db.sqlite")).await.unwrap();
        let workspace = Workspace::default();
        let home = database
            .create_project(&workspace, "Home", None)
            .await
            .unwrap();
        let work = database
            .create_project(&workspace, "Work", None)
            .await
            .unwrap();
        let (home, work) = (home.project.key, work.project.key);

        let auto = TaskIntakeContext::with_project_hint(&database, &workspace, None)
            .await
            .unwrap();
        assert_eq!(
            (
                auto.fixed_project.as_deref(),
                auto.inferred_project.as_deref()
            ),
            (None, None)
        );
        async fn parse(
            database: &Database,
            context: &TaskIntakeContext,
            output: &str,
        ) -> Result<TaskIntakeResult> {
            parsed_output_to_result_with_database(database, context, output, TaskSource::Unknown)
                .await
        }
        let unrouted = parse(&database, &auto, "{\"title\":\"Plain\"}")
            .await
            .unwrap();
        assert_eq!(unrouted.task.project, None);

        let hinted = TaskIntakeContext::with_project_hint(&database, &workspace, Some("work"))
            .await
            .unwrap();
        assert_eq!(hinted.fixed_project, None);
        let fallback = parse(&database, &hinted, "{\"title\":\"Fallback\"}")
            .await
            .unwrap();
        assert_eq!(fallback.task.project.as_deref(), Some(work.as_str()));
        let chosen = parse(
            &database,
            &hinted,
            "{\"title\":\"Chosen\",\"project\":\"Home\"}",
        )
        .await
        .unwrap();
        assert_eq!(chosen.task.project.as_deref(), Some(home.as_str()));

        let malformed = parse(&database, &hinted, "no json here")
            .await
            .err()
            .unwrap();
        assert!(format!("{malformed:#}").contains("task-intake-json-missing"));
        let untitled = parse(&database, &hinted, "{\"title\":\" \"}")
            .await
            .err()
            .unwrap();
        assert!(format!("{untitled:#}").contains("task-intake-title-required"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aborting_task_intake_terminates_command() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("intake.sh");
        let pid_file = dir.path().join("pid");
        std::fs::write(&script, "#!/bin/sh\nprintf '%s' $$ > \"$1\"\nsleep 30\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        let config = TaskIntakeConfig {
            command: Some(script.display().to_string()),
            args: vec![pid_file.display().to_string()],
            timeout_seconds: Some(30),
            system_prompt: None,
            image_args: Vec::new(),
        };
        let context = TaskIntakeContext {
            workspace_id: Workspace::default().id,
            fixed_project: None,
            inferred_project: None,
            projects: Vec::new(),
            labels: Vec::new(),
        };
        let worker =
            tokio::spawn(
                async move { run_task_intake_command(&config, &context, "pending").await },
            );

        let pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(&pid_file) {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("task intake command should start");
        worker.abort();
        let _ = worker.await;

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let running = std::process::Command::new("kill")
                    .args(["-0", pid.trim()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
                if !running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("aborted task intake command should terminate");
    }
}
