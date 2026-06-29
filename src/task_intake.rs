use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sqlx::SqliteConnection;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{error, info};

use crate::choices::{PRIORITIES, TaskPriority};
use crate::config::TaskIntakeConfig;
use crate::labels::{list_labels_in_workspace, resolve_labels_in_workspace};
use crate::operations::TaskDraft;
use crate::projects::{
    inferred_project_key_for_add_in_workspace, resolve_existing_project_in_workspace,
};
use crate::query::{ProjectListItem, list_project_items_in_workspace};
use crate::workspaces::Workspace;

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
}

pub(crate) struct TaskIntakeContext {
    pub(crate) workspace_id: String,
    pub(crate) inferred_project: Option<String>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) labels: Vec<String>,
}

impl TaskIntakeContext {
    pub(crate) async fn load_with_project(
        conn: &mut SqliteConnection,
        project: Option<&str>,
    ) -> Result<Self> {
        let workspace = crate::workspaces::active_workspace();
        Self::load_for_workspace(conn, &workspace, project).await
    }

    pub(crate) async fn load_for_workspace(
        conn: &mut SqliteConnection,
        workspace: &Workspace,
        project: Option<&str>,
    ) -> Result<Self> {
        let inferred_project = match project {
            Some(project) => Some(
                resolve_existing_project_in_workspace(conn, &workspace.id, project)
                    .await?
                    .key,
            ),
            None => inferred_project_key_for_add_in_workspace(conn, workspace.id.as_str()).await?,
        };
        let projects = list_project_items_in_workspace(conn, &workspace.id).await?;
        let labels = list_labels_in_workspace(conn, &workspace.id, None).await?;
        Ok(Self {
            workspace_id: workspace.id.clone(),
            inferred_project,
            projects,
            labels,
        })
    }
}

pub(crate) async fn parse_task_intake(
    conn: &mut SqliteConnection,
    config: &TaskIntakeConfig,
    input: &str,
) -> Result<TaskDraft> {
    parse_task_intake_with_project(conn, config, input, None).await
}

pub(crate) async fn parse_task_intake_with_project(
    conn: &mut SqliteConnection,
    config: &TaskIntakeConfig,
    input: &str,
    project: Option<&str>,
) -> Result<TaskDraft> {
    let context = TaskIntakeContext::load_with_project(conn, project).await?;
    parse_task_intake_with_context(conn, config, input, &context).await
}

pub(crate) async fn parse_task_intake_in_workspace(
    conn: &mut SqliteConnection,
    config: &TaskIntakeConfig,
    input: &str,
    workspace: &Workspace,
    project: Option<&str>,
) -> Result<TaskDraft> {
    let context = TaskIntakeContext::load_for_workspace(conn, workspace, project).await?;
    parse_task_intake_with_context(conn, config, input, &context).await
}

async fn parse_task_intake_with_context(
    conn: &mut SqliteConnection,
    config: &TaskIntakeConfig,
    input: &str,
    context: &TaskIntakeContext,
) -> Result<TaskDraft> {
    info!(
        workspace_id = %context.workspace_id,
        input = %input,
        "task intake input received"
    );
    let outcome = async {
        let output = run_task_intake_command(config, context, input).await?;
        parsed_output_to_draft(conn, context, &output).await
    }
    .await;
    match outcome {
        Ok(draft) => {
            info!(
                workspace_id = %context.workspace_id,
                input = %input,
                "task intake input parsed"
            );
            Ok(draft)
        }
        Err(error) => {
            error!(
                workspace_id = %context.workspace_id,
                input = %input,
                error = %error,
                "task intake input failed"
            );
            Err(error)
        }
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
{\"title\":\"task title\",\"description\":\"optional durable context\",\"project\":\"optional project key or name\",\"priority\":\"none|low|medium|high|urgent\",\"labels\":[\"existing-label\"]}\n\n\
Rules:\n\
- The title is required and should be concise.\n\
- Prefer a concise imperative task title that reads like an existing Aven task.\n\
- Start with a capitalized action verb when it reads naturally.\n\
- Include enough context to distinguish the task from nearby work.\n\
- Keep meaningful casing for names, acronyms, file names, flags, and code identifiers.\n\
- Use project only when the text clearly names one of the available projects.\n\
- Use only existing labels.\n\
- Put durable context in description when helpful.\n\n\
Use only these priorities: {priorities}.\n\n\
Inferred project: {inferred_project}\n\n\
Available projects:\n{projects}\n\n\
Available labels:\n{labels}\n\n\
Raw intake text:\n{input}\n"
}

fn expand_task_intake_prompt(template: &str, context: &TaskIntakeContext, input: &str) -> String {
    template
        .replace("{priorities}", &PRIORITIES.join(", "))
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

pub(crate) async fn parsed_output_to_draft(
    conn: &mut SqliteConnection,
    context: &TaskIntakeContext,
    output: &str,
) -> Result<TaskDraft> {
    let json = extract_json(output).context("error task-intake-json-missing")?;
    let parsed: ParsedTaskPayload =
        serde_json::from_str(json).context("error task-intake-json-invalid")?;
    let title = parsed.title.trim();
    if title.is_empty() {
        bail!("error task-intake-title-required");
    }
    let priority = parsed.priority.unwrap_or_else(|| "none".to_string());
    TaskPriority::parse(&priority)?;
    let project = if let Some(project) = parsed
        .project
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(
            resolve_existing_project_in_workspace(conn, context.workspace_id.as_str(), project)
                .await?
                .key,
        )
    } else {
        context.inferred_project.clone()
    };
    let labels =
        resolve_labels_in_workspace(conn, context.workspace_id.as_str(), &parsed.labels).await?;
    let description = parsed.description.trim().to_string();
    Ok(TaskDraft {
        title: title.to_string(),
        description,
        project,
        status: "inbox".to_string(),
        priority,
        labels,
    })
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
}
