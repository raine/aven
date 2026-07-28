use anyhow::{Result, bail};
use aven_core::choices::TaskSource;
use aven_core::db::Database;
use serde::Serialize;

use super::validation::{validate_optional_priority, validate_optional_status, validate_priority};
use crate::cli::{
    AddArgs, InternalNaturalAddArgs, ListArgs, RefArgs, ShowArgs, TaskEditArgs, TaskSearchArgs,
};
use crate::config::AppConfig;
use crate::input::read_optional_text;
use crate::operations::{TaskDraft, TaskUpdate};
use crate::query::{
    self, SortDirection, TaskAvailabilityFilter, TaskFilters, TaskQueryMode, TaskSearchQuery,
    TaskSearchResult, TaskSort,
};
use crate::refs::DisplayRefContext;
use crate::render::{KvLine, changed_text, print_json_pretty, quote};
use crate::task_render::{
    TaskConflictReport, TaskFullReport, attachment_metadata_json, print_full_task_report,
    print_task_line_item, task_full_json, task_line_json_item,
};
use crate::types::Task;
use crate::workspaces::Workspace;

pub(crate) async fn cmd_add(
    database: &Database,
    workspace: &Workspace,
    config: &AppConfig,
    args: AddArgs,
) -> Result<()> {
    validate_priority(&args.priority)?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?
    .unwrap_or_default();
    let mut draft = if args.natural {
        if !description.is_empty()
            || args.project.is_some()
            || args.priority != "none"
            || !args.label.is_empty()
            || args.available_at.is_some()
            || args.due.is_some()
        {
            bail!(
                "error natural-add-exclusive hint=\"use plain add flags or --natural, not both\""
            );
        }
        let context =
            crate::task_intake::TaskIntakeContext::load_with_database(database, workspace, None)
                .await?;
        let output = crate::task_intake::run_task_intake_command(
            &config.agent.task_intake,
            &context,
            &args.title,
        )
        .await?;
        crate::task_intake::parsed_output_to_draft_with_database(
            database,
            &context,
            &output,
            TaskSource::Cli,
        )
        .await?
    } else {
        TaskDraft {
            title: args.title,
            description,
            project: args.project,
            status: "inbox".to_string(),
            priority: args.priority,
            source: TaskSource::Cli,
            labels: args.label,
            available_at: args
                .available_at
                .as_deref()
                .map(crate::time_input::parse_available_at_input)
                .transpose()?,
            due_on: args
                .due
                .as_deref()
                .map(crate::time_input::parse_due_on_input)
                .transpose()?,
            is_epic: args.epic,
        }
    };
    if let Some(project) = draft.project.as_deref() {
        draft.project = Some(
            crate::projects::resolve_project_key_for_add_with_database(
                database,
                &workspace.id,
                project,
            )
            .await?,
        );
    } else {
        draft.project =
            crate::projects::inferred_project_key_for_add_with_database(database, workspace)
                .await?;
    }
    let outcome = database.create_task(workspace, draft).await?;
    let task = outcome.task;
    let display_refs = database.display_ref_context(&workspace.id).await?;
    print_created_task(&task, workspace, &display_refs);
    Ok(())
}

pub(crate) async fn cmd_internal_natural_add(
    database: &Database,
    config: &AppConfig,
    args: InternalNaturalAddArgs,
) -> Result<()> {
    let workspace = database.workspace_for_id(&args.workspace_id).await?;
    let outcome = async {
        let context = crate::task_intake::TaskIntakeContext::load_with_database(
            database,
            &workspace,
            args.project.as_deref(),
        )
        .await?;
        let output = crate::task_intake::run_task_intake_command(
            &config.agent.task_intake,
            &context,
            &args.input,
        )
        .await?;
        let draft = crate::task_intake::parsed_output_to_draft_with_database(
            database,
            &context,
            &output,
            TaskSource::Tui,
        )
        .await?;
        let undo = if args.tui_undo {
            aven_core::operations::TaskCreationUndo::TuiTask
        } else {
            aven_core::operations::TaskCreationUndo::None
        };
        let outcome = database
            .create_task_with_undo(&workspace, draft, undo)
            .await?;
        Ok(outcome)
    }
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::error!(
                workspace_id = %args.workspace_id,
                has_project_context = args.project.is_some(),
                error = %error,
                "internal natural-add failed"
            );
            return Err(error);
        }
    };
    let task = outcome.task;
    tracing::info!(
        workspace_id = %args.workspace_id,
        task_id = %task.id,
        project = %task.project_key,
        "created task from internal natural-add"
    );
    let display_refs = database.display_ref_context(&workspace.id).await?;
    print_created_task(&task, &workspace, &display_refs);
    crate::daemon::wake_if_enabled(config);
    Ok(())
}

fn print_created_task(task: &Task, workspace: &Workspace, display_refs: &DisplayRefContext) {
    println!(
        "created {} ref={} project={} status={} priority={}{}{} title={}",
        display_refs.display_ref(task),
        display_refs.display_suffix(&workspace.id, &task.id),
        task.project_key,
        task.status,
        task.priority,
        available_at_display(task.available_at.as_deref()),
        due_on_display(task.due_on.as_deref()),
        quote(&task.title)
    );
}

fn available_at_display(available_at: Option<&str>) -> String {
    available_at
        .map(|available_at| format!(" available_at={available_at}"))
        .unwrap_or_default()
}

fn due_on_display(due_on: Option<&str>) -> String {
    due_on
        .map(|due_on| format!(" due_on={due_on}"))
        .unwrap_or_default()
}

async fn build_full_task_report(
    database: &Database,
    detail: query::TaskDetail,
) -> Result<TaskFullReport> {
    let task = &detail.item.task;
    let mut conflicts = Vec::with_capacity(detail.conflicts.len());
    for conflict in &detail.conflicts {
        let local_value = database
            .conflict_display_value(&task.workspace_id, &conflict.field, &conflict.local_value)
            .await?;
        let remote_value = database
            .conflict_display_value(&task.workspace_id, &conflict.field, &conflict.remote_value)
            .await?;
        conflicts.push(TaskConflictReport {
            field: conflict.field.clone(),
            variant_a: conflict.variant_a.clone(),
            local_value,
            variant_b: conflict.variant_b.clone(),
            remote_value,
        });
    }
    let attachments = database
        .attachment_read_items_by_task(&task.workspace_id, &task.id, true)
        .await?
        .into_iter()
        .map(attachment_metadata_json)
        .collect();
    Ok(TaskFullReport {
        detail,
        conflicts,
        attachments,
    })
}

pub(crate) async fn cmd_show(
    database: &Database,
    workspace: &Workspace,
    args: ShowArgs,
) -> Result<()> {
    let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
    if args.full {
        let detail = database.task_detail(&task).await?;
        let report = build_full_task_report(database, detail).await?;
        if args.json {
            print_json_pretty(&task_full_json(&report))?;
        } else {
            print_full_task_report(&report);
        }
    } else {
        let item = database
            .list_task_items(
                &workspace.id,
                TaskFilters {
                    task_ids: vec![task.id.clone()],
                    ..TaskFilters::default().include_deleted(task.deleted)
                },
                TaskQueryMode::Flat,
                TaskSort::Updated,
                SortDirection::Desc,
            )
            .await?
            .into_iter()
            .next()
            .expect("task must exist after resolve");
        if args.json {
            print_json_pretty(&task_line_json_item(&item))?;
        } else {
            print_task_line_item(&item);
        }
    }
    Ok(())
}

pub(crate) async fn cmd_list(
    database: &Database,
    workspace: &Workspace,
    args: ListArgs,
) -> Result<()> {
    if args.ready && args.blocked {
        bail!(
            "error list-dependency-filter-conflict hint=\"pass at most one of --ready or --blocked\""
        );
    }
    if (args.ready || args.blocked) && (args.all || args.deleted) {
        bail!(
            "error list-dependency-filter-all-conflict hint=\"dependency filters only include open tasks\""
        );
    }
    if args.ready && args.epics {
        bail!("error list-epic-ready-conflict hint=\"pass at most one of --ready or --epics\"");
    }
    if args.upcoming && (args.ready || args.blocked || args.epics || args.deleted) {
        bail!(
            "error list-upcoming-filter-conflict hint=\"combine --upcoming only with project, status, priority, label, or --all\""
        );
    }
    let filters = list_task_filters(&args);
    let sort = if args.upcoming {
        TaskSort::AvailableAt
    } else if args.overdue {
        TaskSort::DueOn
    } else {
        TaskSort::Updated
    };
    let direction = if args.upcoming || args.overdue {
        SortDirection::Asc
    } else {
        SortDirection::Desc
    };
    let mut items = database
        .list_task_items(&workspace.id, filters, TaskQueryMode::Flat, sort, direction)
        .await?;
    if let Some(limit) = args.limit {
        items.truncate(limit);
    }
    if args.json {
        let items = items.iter().map(task_line_json_item).collect::<Vec<_>>();
        print_json_pretty(&items)?;
    } else {
        for item in items {
            print_task_line_item(&item);
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct SearchJsonItem {
    r#ref: String,
    id: String,
    title: String,
    project: String,
    status: String,
    priority: String,
    labels: Vec<String>,
    deleted: bool,
    score: i64,
    matched_field: query::SearchMatchedField,
    snippet: Option<String>,
}

pub(crate) async fn cmd_search(
    database: &Database,
    workspace: &Workspace,
    args: TaskSearchArgs,
) -> Result<()> {
    let text = args.query.join(" ");
    if text.trim().is_empty() {
        bail!("error search-query-required hint=\"pass one or more search terms\"");
    }
    let results = database
        .search_task_items(
            &workspace.id,
            TaskSearchQuery {
                text,
                include_deleted: args.all,
                limit: args.limit,
            },
        )
        .await?;
    if args.json {
        let items = results.iter().map(search_json_item).collect::<Vec<_>>();
        print_json_pretty(&items)?;
    } else {
        for result in results {
            print_search_result(&result);
        }
    }
    Ok(())
}

fn print_search_result(result: &TaskSearchResult) {
    let item = &result.item;
    let labels = item.labels.join(",");
    let line = KvLine::new(item.display_ref.clone())
        .field("status", item.task.status)
        .field("priority", item.task.priority)
        .field("project", &item.task.project_key)
        .field("labels", &labels)
        .field("match", result.matched_field.as_str())
        .field("score", result.score)
        .optional("deleted", item.task.deleted.then(|| "yes".to_string()))
        .quoted("title", &item.task.title)
        .finish();
    println!("{line}");
    if let Some(snippet) = &result.snippet {
        println!("  snippet={}", quote(snippet));
    }
}

fn search_json_item(result: &TaskSearchResult) -> SearchJsonItem {
    SearchJsonItem {
        r#ref: result.item.display_ref.clone(),
        id: result.item.task.id.to_string(),
        title: result.item.task.title.clone(),
        project: result.item.task.project_key.clone(),
        status: result.item.task.status.as_str().to_string(),
        priority: result.item.task.priority.as_str().to_string(),
        labels: result.item.labels.clone(),
        deleted: result.item.task.deleted,
        score: result.score,
        matched_field: result.matched_field,
        snippet: result.snippet.clone(),
    }
}

fn list_task_filters(args: &ListArgs) -> TaskFilters {
    let terminal_status = matches!(args.status.as_deref(), Some("done" | "canceled"));
    let availability = if args.upcoming {
        TaskAvailabilityFilter::Upcoming
    } else if args.deleted || terminal_status {
        TaskAvailabilityFilter::All
    } else {
        TaskAvailabilityFilter::Available
    };
    TaskFilters {
        ready_only: args.ready,
        blocked_only: args.blocked,
        epics_only: args.epics,
        exclude_epics: args.ready,
        overdue_only: args.overdue,
        availability,
        label: args.label.clone(),
        ..TaskFilters::default()
            .with_project(args.project.clone())
            .with_status(args.status.clone())
            .with_priority(args.priority.clone())
            .include_deleted(args.all || args.deleted)
            .deleted_only(args.deleted)
    }
}

fn parse_epic_switch(value: Option<String>) -> Result<Option<bool>> {
    value
        .map(|value| match value.as_str() {
            "on" | "true" | "1" => Ok(true),
            "off" | "false" | "0" => Ok(false),
            _ => bail!("error invalid-epic value={value}"),
        })
        .transpose()
}

pub(crate) async fn cmd_edit(
    database: &Database,
    workspace: &Workspace,
    args: TaskEditArgs,
) -> Result<()> {
    let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?;
    validate_optional_status(args.status.as_deref())?;
    validate_optional_priority(args.priority.as_deref())?;
    if args.available_at.is_some() && args.clear_available_at {
        bail!(
            "error available-at-conflict hint=\"use --available-at or --clear-available-at, not both\""
        );
    }
    let available_at = if args.clear_available_at {
        Some(None)
    } else {
        args.available_at
            .as_deref()
            .map(crate::time_input::parse_available_at_input)
            .transpose()?
            .map(Some)
    };
    let due_on = if args.clear_due {
        if args.due.is_some() {
            bail!("error due-conflict hint=\"use --due or --clear-due, not both\"");
        }
        Some(None)
    } else {
        args.due
            .as_deref()
            .map(crate::time_input::parse_due_on_input)
            .transpose()?
            .map(Some)
    };
    let is_epic = parse_epic_switch(args.epic)?;
    let outcome = database
        .update_task(
            workspace,
            &task.id,
            TaskUpdate {
                title: args.title,
                description,
                project: args.project,
                status: args.status,
                priority: args.priority,
                cycle_priority: None,
                available_at,
                due_on,
                deleted: None,
                is_epic,
                add_labels: args.label,
                remove_labels: args.remove_label,
                label_selection: None,
            },
        )
        .await?;
    let task = outcome.task;
    let display_refs = database.display_ref_context(&workspace.id).await?;
    println!(
        "updated {} changed={} status={} priority={}{}{} title={}",
        display_refs.display_ref(&task),
        changed_text(outcome.changed),
        task.status,
        task.priority,
        available_at_display(task.available_at.as_deref()),
        due_on_display(task.due_on.as_deref()),
        quote(&task.title)
    );
    Ok(())
}
pub(crate) async fn cmd_delete_restore(
    database: &Database,
    workspace: &Workspace,
    args: RefArgs,
    delete: bool,
) -> Result<()> {
    let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
    let outcome = database
        .set_task_deleted(workspace, &task.id, delete)
        .await?;
    let task = outcome.task;
    let display_refs = database.display_ref_context(&workspace.id).await?;
    if delete {
        println!("deleted {}", display_refs.display_ref(&task));
    } else {
        println!("restored {}", display_refs.display_ref(&task));
    }
    Ok(())
}
