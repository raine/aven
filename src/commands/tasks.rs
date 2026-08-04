use anyhow::{Context, Result, bail};
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
    TaskLineJson, build_full_task_report, print_full_task_report, print_task_line_item,
    task_full_json, task_line_json_item,
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
    validate_optional_status(args.status.as_deref())?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?
    .unwrap_or_default();
    if args.repeat.is_none()
        && (args.repeat_at.is_some()
            || args.repeat_due.is_some()
            || args.time_zone.is_some()
            || args.repeat_start_on.is_some())
    {
        bail!(
            "error recurrence-flags-require-repeat hint=\"pass --repeat with recurrence scheduling flags\""
        );
    }
    if let Some(rule) = args.repeat.as_deref() {
        if args.available_at.is_some() || args.due.is_some() {
            bail!(
                "error recurrence-absolute-time-conflict hint=\"use --repeat-at for local availability and --repeat-due for same-day or no due date\""
            );
        }
        if args.natural {
            bail!(
                "error natural-add-exclusive hint=\"use plain recurrence flags or --natural, not both\""
            );
        }
        if args.epic {
            bail!(
                "error recurrence-epic-unsupported hint=\"create recurring work as an ordinary task\""
            );
        }
        let project = resolve_add_project(database, workspace, args.project.as_deref()).await?;
        let schedule = super::recurrence_schedule(
            rule,
            args.repeat_at.as_deref(),
            args.repeat_due.as_deref(),
            args.time_zone.as_deref(),
            args.repeat_start_on.as_deref(),
        )?;
        let outcome = database
            .create_recurrence_series(
                workspace,
                aven_core::operations::CreateRecurrenceSeriesParams::new(
                    aven_core::operations::RecurrenceSeriesDraft {
                        title: args.title,
                        description,
                        project,
                        priority: args.priority,
                        initial_status: args.status.unwrap_or_else(|| "todo".to_string()),
                        labels: args.label,
                        schedule,
                    },
                ),
            )
            .await?;
        let display_refs = database.display_ref_context(&workspace.id).await?;
        println!(
            "created {} occurrence={} slot={} status={} priority={} title={}",
            outcome.series_ref,
            display_refs.display_ref(&outcome.task),
            outcome.occurrence.slot_on,
            outcome.task.status,
            outcome.task.priority,
            quote(&outcome.task.title)
        );
        return Ok(());
    }
    let mut draft = if args.natural {
        if !description.is_empty()
            || args.status.is_some()
            || args.priority != "none"
            || !args.label.is_empty()
            || args.available_at.is_some()
            || args.due.is_some()
        {
            bail!(
                "error natural-add-exclusive hint=\"use plain add flags or --natural, not both\""
            );
        }
        let context = crate::task_intake::TaskIntakeContext::load_with_database(
            database,
            workspace,
            args.project.as_deref(),
        )
        .await?;
        let output = crate::task_intake::run_task_intake_command(
            &config.agent.task_intake,
            &context,
            &args.title,
        )
        .await?;
        let intake = crate::task_intake::parsed_output_to_result_with_database(
            database,
            &context,
            &output,
            TaskSource::Cli,
        )
        .await?;
        if intake.recurrence.is_some() {
            let mut recurring = intake
                .into_recurrence_draft()
                .expect("recurring intake has a recurrence schedule");
            if recurring.project.is_empty() {
                recurring.project = resolve_add_project(database, workspace, None).await?;
            }
            let outcome = database
                .create_recurrence_series(
                    workspace,
                    aven_core::operations::CreateRecurrenceSeriesParams::new(recurring),
                )
                .await?;
            let display_refs = database.display_ref_context(&workspace.id).await?;
            println!(
                "created {} occurrence={} slot={} status={} priority={} title={}",
                outcome.series_ref,
                display_refs.display_ref(&outcome.task),
                outcome.occurrence.slot_on,
                outcome.task.status,
                outcome.task.priority,
                quote(&outcome.task.title)
            );
            return Ok(());
        }
        intake.task
    } else {
        TaskDraft {
            title: args.title,
            description,
            project: args.project,
            status: args.status.unwrap_or_else(|| "inbox".to_string()),
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

async fn resolve_add_project(
    database: &Database,
    workspace: &Workspace,
    project: Option<&str>,
) -> Result<String> {
    if let Some(project) = project {
        return crate::projects::resolve_project_key_for_add_with_database(
            database,
            &workspace.id,
            project,
        )
        .await;
    }
    Ok(
        crate::projects::inferred_project_key_for_add_with_database(database, workspace)
            .await?
            .unwrap_or_else(|| "default".to_string()),
    )
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
        let intake = crate::task_intake::parsed_output_to_result_with_database(
            database,
            &context,
            &output,
            TaskSource::Tui,
        )
        .await?;
        if intake.recurrence.is_some() {
            let mut draft = intake
                .into_recurrence_draft()
                .expect("recurring intake has a recurrence schedule");
            if draft.project.is_empty() {
                draft.project = resolve_add_project(database, &workspace, None).await?;
            }
            return Ok(database
                .create_recurrence_series(
                    &workspace,
                    aven_core::operations::CreateRecurrenceSeriesParams::new(draft),
                )
                .await?
                .task);
        }
        let undo = if args.tui_undo {
            aven_core::operations::TaskCreationUndo::TuiTask
        } else {
            aven_core::operations::TaskCreationUndo::None
        };
        Ok(database
            .create_task_with_undo(&workspace, intake.task, undo)
            .await?
            .task)
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
    let task = outcome;
    let display_refs = database.display_ref_context(&workspace.id).await?;
    if let Some(tui_pid) = args.tui_pid {
        crate::notification::notify_intake_task_added_if_tui_exited(
            tui_pid.get(),
            &display_refs.display_ref(&task),
            &task.title,
        );
    }
    tracing::info!(
        workspace_id = %args.workspace_id,
        task_id = %task.id,
        project = %task.project_key,
        "created task from internal natural-add"
    );
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

pub(crate) async fn cmd_show(
    database: &Database,
    workspace: &Workspace,
    args: ShowArgs,
) -> Result<()> {
    let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
    if args.full {
        let detail = database.task_detail(&task).await?;
        let report = build_full_task_report(database, workspace, detail).await?;
        if args.json {
            print_json_pretty(&task_full_json(&report))?;
        } else {
            print_full_task_report(&report);
        }
    } else {
        let item = database
            .list_task_summary_items(
                &workspace.id,
                TaskFilters {
                    task_ids: vec![task.id.clone()],
                    include_deleted: true,
                    expand_recurring: true,
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Updated,
                SortDirection::Desc,
                Some(1),
            )
            .await?
            .into_iter()
            .next()
            .context("resolved task missing from compact task read")?;
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
    let items = database
        .list_task_summary_items(
            &workspace.id,
            filters,
            TaskQueryMode::Flat,
            sort,
            direction,
            args.limit,
        )
        .await?;
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
    #[serde(flatten)]
    task: TaskLineJson,
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
    let query = TaskSearchQuery {
        text,
        project: args.project,
        include_deleted: args.all,
        limit: args.limit,
    };
    let results = if args.expand_recurring {
        database
            .search_task_occurrence_items(&workspace.id, query)
            .await?
    } else {
        database.search_task_items(&workspace.id, query).await?
    };
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
    if let Some(group) = &item.recurrence_group {
        let counts = &group.counts;
        let line = KvLine::new(group.series_ref.clone())
            .field("status", item.task.status)
            .field("priority", item.task.priority)
            .field("project", &item.task.project_key)
            .field("labels", &labels)
            .field("match", result.matched_field.as_str())
            .field("score", result.score)
            .optional(
                "latest",
                counts
                    .latest_outcome
                    .map(|value| value.as_str().to_string()),
            )
            .optional("slot", counts.latest_slot_on.clone())
            .field("completed", counts.completed)
            .field("skipped", counts.skipped)
            .field("missed", counts.missed)
            .quoted("title", &item.task.title)
            .finish();
        println!("{line}");
        if let Some(snippet) = &result.snippet {
            println!("  snippet={}", quote(snippet));
        }
        return;
    }
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
        task: task_line_json_item(&result.item),
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
        expand_recurring: args.expand_recurring,
        hide_done: args.open,
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
                create_missing_labels: false,
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
