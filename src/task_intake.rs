use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sqlx::SqliteConnection;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::choices::{PRIORITIES, validate_choice};
use crate::config::TaskIntakeConfig;
use crate::labels::resolve_labels_in_workspace;
use crate::operations::TaskDraft;
use crate::projects::{
    inferred_project_key_for_add_in_workspace, resolve_existing_project_in_workspace,
};
use crate::query::{self, ProjectListItem};

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
    pub(crate) async fn load(conn: &mut SqliteConnection) -> Result<Self> {
        let workspace_id = crate::workspaces::active_workspace_id();
        let inferred_project =
            inferred_project_key_for_add_in_workspace(conn, workspace_id.as_str()).await?;
        let projects = query::list_project_items(conn).await?;
        let labels = crate::labels::list_labels(conn, None).await?;
        Ok(Self {
            workspace_id,
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
    let context = TaskIntakeContext::load(conn).await?;
    let output = run_task_intake_command(config, &context, input).await?;
    parsed_output_to_draft(conn, &context, input, &output).await
}

async fn run_task_intake_command(
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
    let prompt = task_intake_prompt(context, input);
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

fn task_intake_prompt(context: &TaskIntakeContext, input: &str) -> String {
    let projects = context
        .projects
        .iter()
        .map(|project| format!("- {} ({})", project.key, project.name))
        .collect::<Vec<_>>()
        .join("\n");
    let labels = if context.labels.is_empty() {
        "(none)".to_string()
    } else {
        context
            .labels
            .iter()
            .map(|label| format!("- {label}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "You turn raw task intake text into one Aven task payload.\n\n\
Return only JSON with this shape:\n\
{{\"title\":\"short imperative task title\",\"description\":\"optional durable context\",\"project\":\"optional project key or name\",\"priority\":\"none|low|medium|high|urgent\",\"labels\":[\"existing-label\"]}}\n\n\
Rules:\n\
- The title is required and should be concise.\n\
- Use only these priorities: {}.\n\
- Use project only when the text clearly names one of the available projects.\n\
- Use only existing labels.\n\
- Put the original request and useful context in description when helpful.\n\n\
Inferred project: {}\n\n\
Available projects:\n{}\n\n\
Available labels:\n{}\n\n\
Raw intake text:\n{}\n",
        PRIORITIES.join(", "),
        context.inferred_project.as_deref().unwrap_or("none"),
        projects,
        labels,
        input
    )
}

async fn parsed_output_to_draft(
    conn: &mut SqliteConnection,
    context: &TaskIntakeContext,
    raw_input: &str,
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
    validate_choice("priority", &priority, PRIORITIES)?;
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
    let description = if parsed.description.trim().is_empty() {
        raw_input.trim().to_string()
    } else {
        parsed.description.trim().to_string()
    };
    Ok(TaskDraft {
        title: title.to_string(),
        description,
        project,
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
