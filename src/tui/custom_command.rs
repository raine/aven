use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::{CustomTuiCommandConfig, CustomTuiCommandExecution, CustomTuiCommandSuccess};
use crate::query::{AttachmentMetadata, TaskDependencyLink, TaskListItem, TaskNote};
use crate::workspaces::Workspace;

#[derive(Debug, Clone)]
pub(crate) struct CustomCommandInvocation {
    pub(crate) name: String,
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) stdin_json: Vec<u8>,
    pub(crate) execution: CustomTuiCommandExecution,
    pub(crate) on_success: CustomTuiCommandSuccess,
}

pub(crate) fn plan_invocation(
    command: &CustomTuiCommandConfig,
    invoked_as: &str,
    workspace: &Workspace,
    primary: Option<&TaskListItem>,
    marked: &[&TaskListItem],
) -> Result<CustomCommandInvocation> {
    let cwd = std::env::current_dir().context("could not determine current directory")?;
    let cwd_text = cwd.to_string_lossy();
    let input = CommandInput {
        version: 1,
        command: CommandIdentity {
            name: &command.name,
            invoked_as,
        },
        invocation: InvocationContext {
            cwd: cwd_text.as_ref(),
            tui_pid: std::process::id(),
        },
        workspace: WorkspaceContext {
            id: workspace.id.to_string(),
            key: &workspace.key,
            name: &workspace.name,
        },
        selection: SelectionContext {
            primary: primary.map(task_context),
            marked: marked.iter().map(|item| task_context(item)).collect(),
        },
    };
    let mut stdin_json = serde_json::to_vec_pretty(&input)?;
    stdin_json.push(b'\n');
    Ok(CustomCommandInvocation {
        name: command.name.clone(),
        program: crate::config::expand_tilde(&command.program)?,
        args: command.args.clone(),
        cwd,
        stdin_json,
        execution: command.execution,
        on_success: command.on_success,
    })
}

#[derive(Serialize)]
struct CommandInput<'a> {
    version: u8,
    command: CommandIdentity<'a>,
    invocation: InvocationContext<'a>,
    workspace: WorkspaceContext<'a>,
    selection: SelectionContext<'a>,
}

#[derive(Serialize)]
struct CommandIdentity<'a> {
    name: &'a str,
    invoked_as: &'a str,
}

#[derive(Serialize)]
struct InvocationContext<'a> {
    cwd: &'a str,
    tui_pid: u32,
}

#[derive(Serialize)]
struct WorkspaceContext<'a> {
    id: String,
    key: &'a str,
    name: &'a str,
}

#[derive(Serialize)]
struct SelectionContext<'a> {
    primary: Option<TaskContext<'a>>,
    marked: Vec<TaskContext<'a>>,
}

#[derive(Serialize)]
struct TaskContext<'a> {
    r#ref: &'a str,
    task: TaskFields<'a>,
    project: ProjectContext<'a>,
    labels: &'a [String],
    notes: Vec<NoteContext<'a>>,
    depends_on: Vec<RelationshipContext<'a>>,
    blocks: Vec<RelationshipContext<'a>>,
    epic_parent: Option<RelationshipContext<'a>>,
    epic_children: Vec<RelationshipContext<'a>>,
    recurrence: Option<RecurrenceContext<'a>>,
    attachments: Vec<AttachmentContext<'a>>,
}

#[derive(Serialize)]
struct TaskFields<'a> {
    id: String,
    title: &'a str,
    description: &'a str,
    status: &'static str,
    priority: &'static str,
    source: &'static str,
    available_at: Option<&'a str>,
    due_on: Option<&'a str>,
    deleted: bool,
    is_epic: bool,
    created_at: &'a str,
    updated_at: &'a str,
}

#[derive(Serialize)]
struct ProjectContext<'a> {
    id: String,
    key: &'a str,
    prefix: &'a str,
}

#[derive(Serialize)]
struct NoteContext<'a> {
    id: &'a str,
    body: &'a str,
    created_at: &'a str,
}

#[derive(Serialize)]
struct RelationshipContext<'a> {
    id: String,
    r#ref: &'a str,
    title: &'a str,
    status: &'a str,
    priority: &'a str,
    unresolved: bool,
}

#[derive(Serialize)]
struct RecurrenceContext<'a> {
    series_id: String,
    series_ref: &'a str,
    slot_on: &'a str,
    rule: &'a str,
    timezone: &'a str,
    lifecycle: &'static str,
    outcome: Option<&'static str>,
    projection_state: &'static str,
}

#[derive(Serialize)]
struct AttachmentContext<'a> {
    id: &'a str,
    media_type: &'a str,
    byte_size: i64,
    filename: Option<&'a str>,
    alt_text: Option<&'a str>,
    width: Option<i64>,
    height: Option<i64>,
    created_at: &'a str,
    has_blob: bool,
}

fn task_context(item: &TaskListItem) -> TaskContext<'_> {
    TaskContext {
        r#ref: &item.display_ref,
        task: TaskFields {
            id: item.task.id.to_string(),
            title: &item.task.title,
            description: &item.task.description,
            status: item.task.status.as_str(),
            priority: item.task.priority.as_str(),
            source: item.task.source.as_str(),
            available_at: item.task.available_at.as_deref(),
            due_on: item.task.due_on.as_deref(),
            deleted: item.task.deleted,
            is_epic: item.task.is_epic,
            created_at: &item.task.created_at,
            updated_at: &item.task.updated_at,
        },
        project: ProjectContext {
            id: item.task.project_id.to_string(),
            key: &item.task.project_key,
            prefix: &item.task.project_prefix,
        },
        labels: &item.labels,
        notes: item.notes.iter().map(note_context).collect(),
        depends_on: item.depends_on.iter().map(relationship_context).collect(),
        blocks: item.blocks.iter().map(relationship_context).collect(),
        epic_parent: item.epic_parent.as_ref().map(relationship_context),
        epic_children: item
            .epic_children
            .iter()
            .map(relationship_context)
            .collect(),
        recurrence: item
            .recurrence
            .as_ref()
            .map(|recurrence| RecurrenceContext {
                series_id: recurrence.series_id.to_string(),
                series_ref: &recurrence.series_ref,
                slot_on: &recurrence.slot_on,
                rule: &recurrence.rule_label,
                timezone: &recurrence.timezone,
                lifecycle: recurrence.lifecycle.as_str(),
                outcome: recurrence.outcome.map(|outcome| outcome.as_str()),
                projection_state: recurrence.projection_state.as_str(),
            }),
        attachments: item.attachments.iter().map(attachment_context).collect(),
    }
}

fn note_context(note: &TaskNote) -> NoteContext<'_> {
    NoteContext {
        id: &note.id,
        body: &note.body,
        created_at: &note.created_at,
    }
}

fn relationship_context(link: &TaskDependencyLink) -> RelationshipContext<'_> {
    RelationshipContext {
        id: link.task_id.to_string(),
        r#ref: &link.display_ref,
        title: &link.title,
        status: &link.status,
        priority: &link.priority,
        unresolved: link.unresolved,
    }
}

fn attachment_context(attachment: &AttachmentMetadata) -> AttachmentContext<'_> {
    AttachmentContext {
        id: &attachment.attachment_id,
        media_type: &attachment.media_type,
        byte_size: attachment.byte_size,
        filename: attachment.filename.as_deref(),
        alt_text: attachment.alt_text.as_deref(),
        width: attachment.width,
        height: attachment.height,
        created_at: &attachment.created_at,
        has_blob: attachment.has_blob,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CustomTuiCommandExecution, CustomTuiCommandRequirement, CustomTuiCommandSuccess,
    };

    fn command() -> CustomTuiCommandConfig {
        CustomTuiCommandConfig {
            name: "dispatch".to_string(),
            aliases: vec!["custom-dispatch".to_string()],
            description: "dispatch task".to_string(),
            program: PathBuf::from("dispatch-task"),
            args: vec!["--static".to_string(), "literal value".to_string()],
            keys: vec![],
            detail_keys: None,
            requires: CustomTuiCommandRequirement::SelectedTask,
            execution: CustomTuiCommandExecution::Wait,
            on_success: CustomTuiCommandSuccess::Quit,
        }
    }

    #[test]
    fn invocation_uses_versioned_json_and_static_arguments() {
        let mut task = crate::tui::test_support::task_list_item("quote \" and $HOME\nnext");
        task.task.description = "$(touch /tmp/not-shell)".to_string();
        task.labels = vec!["feature".to_string()];
        task.notes.push(crate::query::TaskNote {
            id: "note-1".to_string(),
            body: "multiline\nnote".to_string(),
            created_at: "2026-08-05T00:00:00Z".to_string(),
        });
        task.attachments.push(crate::query::AttachmentMetadata {
            attachment_id: "attachment-1".to_string(),
            task_id: task.task.id.to_string(),
            sha256: "secret-hash".to_string(),
            media_type: "image/png".to_string(),
            byte_size: 42,
            filename: Some("image.png".to_string()),
            alt_text: None,
            width: Some(2),
            height: Some(3),
            created_at: "2026-08-05T00:00:00Z".to_string(),
            deleted: false,
            deleted_at: None,
            bytes_state: crate::attachments::AttachmentBytesState::Present,
            has_blob: true,
        });
        let invocation = plan_invocation(
            &command(),
            "custom-dispatch",
            &Workspace::default(),
            Some(&task),
            &[&task],
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&invocation.stdin_json).unwrap();

        assert_eq!(json["version"], 1);
        assert_eq!(json["command"]["name"], "dispatch");
        assert_eq!(json["command"]["invoked_as"], "custom-dispatch");
        assert_eq!(
            json["selection"]["primary"]["task"]["title"],
            task.task.title
        );
        assert_eq!(json["selection"]["marked"].as_array().unwrap().len(), 1);
        assert_eq!(invocation.args, ["--static", "literal value"]);
        let encoded = String::from_utf8(invocation.stdin_json).unwrap();
        assert!(!encoded.contains("secret-hash"));
        assert!(!encoded.contains("bytes_state"));
    }

    #[test]
    fn invocation_without_selection_uses_null_primary() {
        let invocation =
            plan_invocation(&command(), "dispatch", &Workspace::default(), None, &[]).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&invocation.stdin_json).unwrap();

        assert!(json["selection"]["primary"].is_null());
    }
}
