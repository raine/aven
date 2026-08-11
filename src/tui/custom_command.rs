use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::config::{
    CustomTuiCommandConfig, CustomTuiCommandExecution, CustomTuiCommandSuccess,
    CustomTuiCommandTarget,
};
use crate::query::{AttachmentMetadata, TaskDependencyLink, TaskListItem, TaskNote};
use crate::workspaces::Workspace;

#[derive(Clone)]
pub(crate) struct CustomCommandInvocation {
    pub(crate) name: String,
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: BTreeMap<OsString, OsString>,
    pub(crate) timeout: Duration,
    pub(crate) stdin_json: Vec<u8>,
    pub(crate) execution: CustomTuiCommandExecution,
    pub(crate) on_success: CustomTuiCommandSuccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CustomCommandPlanningContext {
    pub(crate) origin_cwd: PathBuf,
    pub(crate) home_dir: Option<PathBuf>,
    pub(crate) tui_pid: u32,
    pub(crate) aven_exe: Option<PathBuf>,
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) db_path: Option<PathBuf>,
    pub(crate) blob_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TargetSource {
    None,
    Focused,
    Marked,
}

#[derive(Debug)]
pub(crate) struct ResolvedCommandTargets<'a> {
    pub(crate) policy: CustomTuiCommandTarget,
    pub(crate) resolved_from: TargetSource,
    pub(crate) tasks: Vec<&'a TaskListItem>,
}

pub(crate) fn resolve_command_targets<'a>(
    policy: CustomTuiCommandTarget,
    primary: Option<&'a TaskListItem>,
    marked: &[&'a TaskListItem],
) -> std::result::Result<ResolvedCommandTargets<'a>, &'static str> {
    let (resolved_from, tasks) = match policy {
        CustomTuiCommandTarget::None => (TargetSource::None, Vec::new()),
        CustomTuiCommandTarget::Focused => (
            TargetSource::Focused,
            vec![primary.ok_or("requires a focused task")?],
        ),
        CustomTuiCommandTarget::Marked => {
            if marked.is_empty() {
                return Err("requires one or more marked tasks");
            }
            (TargetSource::Marked, marked.to_vec())
        }
        CustomTuiCommandTarget::MarkedOrFocused if !marked.is_empty() => {
            (TargetSource::Marked, marked.to_vec())
        }
        CustomTuiCommandTarget::MarkedOrFocused => (
            TargetSource::Focused,
            vec![primary.ok_or("requires a marked or focused task")?],
        ),
    };
    Ok(ResolvedCommandTargets {
        policy,
        resolved_from,
        tasks,
    })
}

pub(crate) fn plan_invocation(
    command: &CustomTuiCommandConfig,
    invoked_as: &str,
    workspace: &Workspace,
    primary: Option<&TaskListItem>,
    marked: &[&TaskListItem],
    targets: &ResolvedCommandTargets<'_>,
    planning: &CustomCommandPlanningContext,
) -> Result<CustomCommandInvocation> {
    let program = crate::config::expand_tilde_from(&command.program, planning.home_dir.as_deref())?;
    let cwd = match &command.cwd {
        Some(configured) => {
            let expanded =
                crate::config::expand_tilde_from(configured, planning.home_dir.as_deref())?;
            absolute_path(&expanded, &planning.origin_cwd)
        }
        None => planning.origin_cwd.clone(),
    };
    let invocation = InvocationContext {
        cwd: json_path(&cwd),
        origin_cwd: json_path(&planning.origin_cwd),
        tui_pid: planning.tui_pid,
        aven_exe: planned_json_path(planning.aven_exe.as_deref(), planning),
        config_dir: planned_json_path(planning.config_dir.as_deref(), planning),
        db_path: planned_json_path(planning.db_path.as_deref(), planning),
        blob_dir: planned_json_path(planning.blob_dir.as_deref(), planning),
    };
    let input = CommandInput {
        version: 1,
        command: CommandIdentity {
            name: &command.name,
            invoked_as,
        },
        invocation,
        workspace: WorkspaceContext {
            id: workspace.id.to_string(),
            key: &workspace.key,
            name: &workspace.name,
        },
        targeting: TargetingContext {
            policy: targets.policy,
            resolved_from: targets.resolved_from,
            targets: targets
                .tasks
                .iter()
                .map(|item| target_identity(item))
                .collect(),
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
        program,
        args: command.args.clone(),
        cwd,
        env: command
            .env
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect(),
        timeout: Duration::from_secs(
            command
                .timeout_seconds
                .unwrap_or(crate::config::DEFAULT_CUSTOM_COMMAND_TIMEOUT_SECONDS),
        ),
        stdin_json,
        execution: command.execution,
        on_success: command.on_success,
    })
}

fn absolute_path(path: &Path, origin_cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        origin_cwd.join(path)
    }
}

fn planned_json_path(
    path: Option<&Path>,
    planning: &CustomCommandPlanningContext,
) -> Option<String> {
    path.and_then(|path| json_path(&absolute_path(path, &planning.origin_cwd)))
}

fn json_path(path: &Path) -> Option<String> {
    path.to_str().map(str::to_string)
}

#[derive(Serialize)]
struct CommandInput<'a> {
    version: u8,
    command: CommandIdentity<'a>,
    invocation: InvocationContext,
    workspace: WorkspaceContext<'a>,
    targeting: TargetingContext<'a>,
    selection: SelectionContext<'a>,
}

#[derive(Serialize)]
struct CommandIdentity<'a> {
    name: &'a str,
    invoked_as: &'a str,
}

#[derive(Serialize)]
struct InvocationContext {
    cwd: Option<String>,
    origin_cwd: Option<String>,
    tui_pid: u32,
    aven_exe: Option<String>,
    config_dir: Option<String>,
    db_path: Option<String>,
    blob_dir: Option<String>,
}

#[derive(Serialize)]
struct WorkspaceContext<'a> {
    id: String,
    key: &'a str,
    name: &'a str,
}

#[derive(Serialize)]
struct TargetingContext<'a> {
    policy: CustomTuiCommandTarget,
    resolved_from: TargetSource,
    targets: Vec<TargetIdentity<'a>>,
}

#[derive(Serialize)]
struct TargetIdentity<'a> {
    id: String,
    r#ref: &'a str,
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

fn target_identity(item: &TaskListItem) -> TargetIdentity<'_> {
    TargetIdentity {
        id: item.task.id.to_string(),
        r#ref: &item.display_ref,
    }
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
    use crate::config::{CustomTuiCommandExecution, CustomTuiCommandSuccess};

    fn planning() -> CustomCommandPlanningContext {
        let origin_cwd = std::env::current_dir().unwrap();
        CustomCommandPlanningContext {
            home_dir: dirs::home_dir(),
            tui_pid: 42,
            aven_exe: Some(origin_cwd.join("bin/aven")),
            config_dir: Some(origin_cwd.join("config")),
            db_path: Some(origin_cwd.join("aven.db")),
            blob_dir: Some(origin_cwd.join("blobs")),
            origin_cwd,
        }
    }

    fn command() -> CustomTuiCommandConfig {
        CustomTuiCommandConfig {
            name: "dispatch".to_string(),
            aliases: vec!["custom-dispatch".to_string()],
            description: "dispatch task".to_string(),
            program: PathBuf::from("dispatch-task"),
            cwd: None,
            env: BTreeMap::new(),
            timeout_seconds: None,
            args: vec!["--static".to_string(), "literal value".to_string()],
            keys: vec![],
            detail_keys: None,
            target: CustomTuiCommandTarget::Focused,
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
        let targets =
            resolve_command_targets(CustomTuiCommandTarget::Focused, Some(&task), &[&task])
                .unwrap();
        let invocation = plan_invocation(
            &command(),
            "custom-dispatch",
            &Workspace::default(),
            Some(&task),
            &[&task],
            &targets,
            &planning(),
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
        assert_eq!(json["targeting"]["policy"], "focused");
        assert_eq!(json["targeting"]["resolved_from"], "focused");
        assert_eq!(json["targeting"]["targets"][0]["id"], task.task.id.as_str());
        assert_eq!(json["targeting"]["targets"][0]["ref"], task.display_ref);
        assert_eq!(invocation.args, ["--static", "literal value"]);
        let encoded = String::from_utf8(invocation.stdin_json).unwrap();
        assert!(!encoded.contains("secret-hash"));
        assert!(!encoded.contains("bytes_state"));
    }

    #[test]
    fn static_cwd_resolves_absolute_tilde_and_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("origin");
        let home = temp.path().join("home");
        let absolute = temp.path().join("absolute");
        let tilde = home.join("tools");
        let relative = origin.join("relative/tools");
        for path in [&origin, &home, &absolute, &tilde, &relative] {
            std::fs::create_dir_all(path).unwrap();
        }
        let planning = CustomCommandPlanningContext {
            origin_cwd: origin.clone(),
            home_dir: Some(home.clone()),
            tui_pid: 42,
            aven_exe: None,
            config_dir: None,
            db_path: None,
            blob_dir: None,
        };

        for (configured, expected) in [
            (absolute.clone(), absolute),
            (PathBuf::from("~/tools"), tilde),
            (PathBuf::from("relative/tools"), relative),
        ] {
            let mut command = command();
            command.cwd = Some(configured);
            command.program = PathBuf::from("~/bin/dispatch-task");
            command.target = CustomTuiCommandTarget::None;
            let targets = resolve_command_targets(command.target, None, &[]).unwrap();
            let invocation = plan_invocation(
                &command,
                "dispatch",
                &Workspace::default(),
                None,
                &[],
                &targets,
                &planning,
            )
            .unwrap();

            assert_eq!(invocation.cwd, expected);
            assert_eq!(invocation.program, home.join("bin/dispatch-task"));
            let json: serde_json::Value = serde_json::from_slice(&invocation.stdin_json).unwrap();
            assert_eq!(json["invocation"]["cwd"], invocation.cwd.to_str().unwrap());
            assert_eq!(json["invocation"]["origin_cwd"], origin.to_str().unwrap());
        }
    }

    #[test]
    fn planner_resolves_cwd_without_reading_the_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        let planning = CustomCommandPlanningContext {
            origin_cwd: temp.path().to_path_buf(),
            home_dir: Some(temp.path().to_path_buf()),
            tui_pid: 42,
            aven_exe: None,
            config_dir: None,
            db_path: None,
            blob_dir: None,
        };
        let mut command = command();
        command.target = CustomTuiCommandTarget::None;
        command.cwd = Some(PathBuf::from("missing"));
        command
            .env
            .insert("ACCESS_TOKEN".to_string(), "secret-marker".to_string());
        let targets = resolve_command_targets(command.target, None, &[]).unwrap();

        let invocation = plan_invocation(
            &command,
            "dispatch",
            &Workspace::default(),
            None,
            &[],
            &targets,
            &planning,
        )
        .unwrap();

        assert_eq!(invocation.cwd, planning.origin_cwd.join("missing"));
        assert!(
            !String::from_utf8(invocation.stdin_json)
                .unwrap()
                .contains("secret-marker")
        );
    }

    #[test]
    fn invocation_projects_paths_and_static_settings_without_environment_values() {
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        let planning = CustomCommandPlanningContext {
            origin_cwd: origin.clone(),
            home_dir: Some(temp.path().join("home")),
            tui_pid: 12345,
            aven_exe: Some(PathBuf::from("bin/aven")),
            config_dir: Some(PathBuf::from("config")),
            db_path: Some(PathBuf::from("state/aven.db")),
            blob_dir: Some(PathBuf::from("data/blobs")),
        };
        let mut command = command();
        command.target = CustomTuiCommandTarget::None;
        command.timeout_seconds = Some(30);
        command
            .env
            .insert("EXPORT_PROFILE".to_string(), "secret-marker".to_string());
        let targets = resolve_command_targets(command.target, None, &[]).unwrap();
        let invocation = plan_invocation(
            &command,
            "dispatch",
            &Workspace::default(),
            None,
            &[],
            &targets,
            &planning,
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&invocation.stdin_json).unwrap();

        assert_eq!(invocation.cwd, origin);
        assert_eq!(invocation.timeout, Duration::from_secs(30));
        assert_eq!(
            invocation.env.get(&OsString::from("EXPORT_PROFILE")),
            Some(&OsString::from("secret-marker"))
        );
        assert_eq!(json["invocation"]["tui_pid"], 12345);
        assert_eq!(
            json["invocation"]["aven_exe"],
            planning.origin_cwd.join("bin/aven").to_str().unwrap()
        );
        assert_eq!(
            json["invocation"]["config_dir"],
            planning.origin_cwd.join("config").to_str().unwrap()
        );
        assert_eq!(
            json["invocation"]["db_path"],
            planning.origin_cwd.join("state/aven.db").to_str().unwrap()
        );
        assert_eq!(
            json["invocation"]["blob_dir"],
            planning.origin_cwd.join("data/blobs").to_str().unwrap()
        );
        assert!(
            !String::from_utf8(invocation.stdin_json)
                .unwrap()
                .contains("secret-marker")
        );
    }

    #[test]
    fn invocation_defaults_preserve_inherited_execution_settings_and_null_paths() {
        let temp = tempfile::tempdir().unwrap();
        let planning = CustomCommandPlanningContext {
            origin_cwd: temp.path().to_path_buf(),
            home_dir: None,
            tui_pid: 7,
            aven_exe: None,
            config_dir: None,
            db_path: None,
            blob_dir: None,
        };
        let mut command = command();
        command.target = CustomTuiCommandTarget::None;
        let targets = resolve_command_targets(command.target, None, &[]).unwrap();
        let invocation = plan_invocation(
            &command,
            "dispatch",
            &Workspace::default(),
            None,
            &[],
            &targets,
            &planning,
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&invocation.stdin_json).unwrap();

        assert_eq!(invocation.cwd, planning.origin_cwd);
        assert!(invocation.env.is_empty());
        assert_eq!(
            invocation.timeout,
            Duration::from_secs(crate::config::DEFAULT_CUSTOM_COMMAND_TIMEOUT_SECONDS)
        );
        for field in ["aven_exe", "config_dir", "db_path", "blob_dir"] {
            assert!(json["invocation"][field].is_null(), "{field}");
        }
    }

    #[test]
    fn invocation_without_selection_uses_null_primary() {
        let mut command = command();
        command.target = CustomTuiCommandTarget::None;
        let targets = resolve_command_targets(command.target, None, &[]).unwrap();
        let invocation = plan_invocation(
            &command,
            "dispatch",
            &Workspace::default(),
            None,
            &[],
            &targets,
            &planning(),
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&invocation.stdin_json).unwrap();

        assert!(json["selection"]["primary"].is_null());
        assert_eq!(json["targeting"]["policy"], "none");
        assert_eq!(json["targeting"]["resolved_from"], "none");
        assert_eq!(json["targeting"]["targets"], serde_json::json!([]));
    }

    #[test]
    fn target_policies_resolve_availability_and_precedence() {
        let primary = crate::tui::test_support::task_list_item("primary");
        let first = crate::tui::test_support::task_list_item("first marked");
        let second = crate::tui::test_support::task_list_item("second marked");
        let marked = [&first, &second];

        let none = resolve_command_targets(CustomTuiCommandTarget::None, None, &[]).unwrap();
        assert_eq!(none.resolved_from, TargetSource::None);
        assert!(none.tasks.is_empty());

        let focused =
            resolve_command_targets(CustomTuiCommandTarget::Focused, Some(&primary), &marked)
                .unwrap();
        assert_eq!(focused.resolved_from, TargetSource::Focused);
        assert_eq!(focused.tasks[0].task.id, primary.task.id);
        assert!(resolve_command_targets(CustomTuiCommandTarget::Focused, None, &marked).is_err());

        let marked_targets =
            resolve_command_targets(CustomTuiCommandTarget::Marked, Some(&primary), &marked)
                .unwrap();
        assert_eq!(marked_targets.resolved_from, TargetSource::Marked);
        assert_eq!(marked_targets.tasks[0].task.id, first.task.id);
        assert_eq!(marked_targets.tasks[1].task.id, second.task.id);
        assert!(
            resolve_command_targets(CustomTuiCommandTarget::Marked, Some(&primary), &[]).is_err()
        );

        let fallback =
            resolve_command_targets(CustomTuiCommandTarget::MarkedOrFocused, Some(&primary), &[])
                .unwrap();
        assert_eq!(fallback.resolved_from, TargetSource::Focused);
        assert_eq!(fallback.tasks[0].task.id, primary.task.id);

        let preferred = resolve_command_targets(
            CustomTuiCommandTarget::MarkedOrFocused,
            Some(&primary),
            &marked,
        )
        .unwrap();
        assert_eq!(preferred.resolved_from, TargetSource::Marked);
        assert_eq!(preferred.tasks.len(), 2);
        assert!(
            resolve_command_targets(CustomTuiCommandTarget::MarkedOrFocused, None, &[]).is_err()
        );
    }

    #[test]
    fn marked_target_json_preserves_projection_order_and_raw_selection() {
        let primary = crate::tui::test_support::task_list_item("primary");
        let first = crate::tui::test_support::task_list_item("first marked");
        let second = crate::tui::test_support::task_list_item("second marked");
        let marked = [&first, &second];
        let targets =
            resolve_command_targets(CustomTuiCommandTarget::Marked, Some(&primary), &marked)
                .unwrap();
        let mut command = command();
        command.target = CustomTuiCommandTarget::Marked;

        let invocation = plan_invocation(
            &command,
            "dispatch",
            &Workspace::default(),
            Some(&primary),
            &marked,
            &targets,
            &planning(),
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&invocation.stdin_json).unwrap();

        assert_eq!(
            json["selection"]["primary"]["task"]["id"],
            primary.task.id.as_str()
        );
        assert_eq!(json["selection"]["marked"].as_array().unwrap().len(), 2);
        assert_eq!(json["targeting"]["resolved_from"], "marked");
        assert_eq!(
            json["targeting"]["targets"][0]["id"],
            first.task.id.as_str()
        );
        assert_eq!(
            json["targeting"]["targets"][1]["id"],
            second.task.id.as_str()
        );
    }
}
