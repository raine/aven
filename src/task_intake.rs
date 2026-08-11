use crate::ids::WorkspaceId;
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

pub(crate) struct TaskIntakeResult {
    pub(crate) task: TaskDraft,
    pub(crate) recurrence: Option<aven_core::recurrence::RecurrenceSchedule>,
}

impl TaskIntakeResult {
    pub(crate) fn into_recurrence_draft(self) -> Option<RecurrenceSeriesDraft> {
        let schedule = self.recurrence?;
        Some(RecurrenceSeriesDraft {
            title: self.task.title,
            description: self.task.description,
            project: self.task.project.unwrap_or_default(),
            priority: self.task.priority,
            initial_status: self.task.status,
            labels: self.task.labels,
            metadata: self.task.metadata,
            schedule,
        })
    }
}

pub(crate) struct TaskIntakeContext {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) fixed_project: Option<String>,
    pub(crate) inferred_project: Option<String>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) labels: Vec<String>,
}

impl TaskIntakeContext {
    pub(crate) async fn load_with_database(
        database: &Database,
        workspace: &Workspace,
        project: Option<&str>,
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
                crate::projects::inferred_project_key_for_add_with_database(database, workspace)
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

pub(crate) async fn run_task_intake_command(
    config: &TaskIntakeConfig,
    context: &TaskIntakeContext,
    input: &str,
) -> Result<String> {
    let command = config
        .command
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context(
            "error task-intake-command-required hint=\"configure agent.task_intake.command\"",
        )?;
    let prompt = task_intake_prompt(config, context, input);
    let prompt_arg = config.args.iter().any(|arg| arg.contains("{prompt}"));
    let args = config
        .args
        .iter()
        .map(|arg| arg.replace("{prompt}", &prompt).replace("{input}", input))
        .collect::<Vec<_>>();
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

fn task_intake_prompt(
    config: &TaskIntakeConfig,
    context: &TaskIntakeContext,
    input: &str,
) -> String {
    let template = config
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_task_intake_system_prompt());
    expand_task_intake_prompt(template, context, input)
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
- Set repeat only for clear recurrence intent such as daily, every Friday, or every Monday and Thursday. Ambiguous timing remains one-off.\n\
- Use natural repeat rules such as daily, weekdays, monthly, yearly, fortnightly, every Friday, every 3 days, every 3 weeks, every 2 months, or every 4 weeks on Monday and Thursday.\n\
- For recurrence, omit available_at and due_on. Set repeat_at only for a stated recurring availability time, repeat_due only for an explicit same-day or no-due policy, time_zone only for an explicit IANA zone, and repeat_start_on only for an explicit YYYY-MM-DD start date.\n\
- Recurrence defaults are no availability time, same-day due, the local time zone, and today in that zone.\n\
- Omit optional fields when they do not apply.\n\
- Put durable context in description when helpful.\n\n\
Use only these priorities: {priorities}.\n\n\
Selected project: {selected_project}\n\n\
Inferred project: {inferred_project}\n\n\
Available projects:\n{projects}\n\n\
Available labels:\n{labels}\n\n\
Raw intake text:\n{input}\n"
}

fn expand_task_intake_prompt(template: &str, context: &TaskIntakeContext, input: &str) -> String {
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

pub(crate) async fn parsed_output_to_result_with_database(
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
            crate::commands::recurrence_schedule(
                &rule,
                recurrence_options[0],
                recurrence_options[1],
                recurrence_options[2],
                recurrence_options[3],
            )
            .map(Some)
        })
        .transpose()
        .context("error task-intake-recurrence-invalid")?
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
    let status = if recurrence.is_some() {
        "todo"
    } else {
        "inbox"
    };
    Ok(TaskIntakeResult {
        task: TaskDraft {
            title: title.to_string(),
            description,
            project,
            status: status.to_string(),
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
