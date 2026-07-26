use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use aven_core::operations::RecurrenceTemplateUpdate;
use aven_core::query::{
    RecurrenceCounts, RecurrenceHistoryEntry, RecurrenceHistoryKind, RecurrenceHistoryPage,
    RecurrenceSeriesDetail, RecurrenceSeriesSummary,
};
use aven_core::recurrence::{
    RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceRule, RecurrenceSchedule, TimeZoneId,
    WeekdaySet,
};
use aven_core::types::RecurrenceSeries;
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, Utc};
use serde::Serialize;

use crate::cli::{
    RecurCommand, RecurEditArgs, RecurHistoryArgs, RecurListArgs, RecurRefArgs, RecurShowArgs,
    RecurStopArgs, RecurSubcommand,
};
use crate::input::read_optional_text;
use crate::render::{KvLine, changed_text, print_json_pretty, print_multiline_block, quote};
use crate::workspaces::Workspace;

const REPORT_VERSION: u32 = 1;

pub(crate) async fn cmd_recur(
    database: &Database,
    workspace: &Workspace,
    args: RecurCommand,
) -> Result<()> {
    match args.command {
        RecurSubcommand::List(args) => list(database, workspace, args).await,
        RecurSubcommand::Show(args) => show(database, workspace, args).await,
        RecurSubcommand::History(args) => history(database, workspace, args).await,
        RecurSubcommand::Edit(args) => edit(database, workspace, args).await,
        RecurSubcommand::Skip(args) => skip(database, workspace, args).await,
        RecurSubcommand::Pause(args) => pause(database, workspace, args).await,
        RecurSubcommand::Resume(args) => resume(database, workspace, args).await,
        RecurSubcommand::Stop(args) => stop(database, workspace, args).await,
    }
}

pub(crate) fn recurrence_schedule(
    rule: &str,
    repeat_at: Option<&str>,
    repeat_due: Option<&str>,
    time_zone: Option<&str>,
    start_on: Option<&str>,
) -> Result<RecurrenceSchedule> {
    let timezone = time_zone.map_or_else(local_timezone, parse_timezone)?;
    let zone = timezone
        .as_str()
        .parse::<chrono_tz::Tz>()
        .expect("core-validated time zone parses with chrono-tz");
    let start_on = start_on.map_or_else(
        || Ok(Utc::now().with_timezone(&zone).date_naive()),
        parse_date,
    )?;
    let rule = parse_rule(rule, start_on)?;
    let available_local_time = repeat_at.map(parse_repeat_time).transpose()?.flatten();
    let due_policy = parse_due_policy(repeat_due.unwrap_or("same-day"))?;
    Ok(RecurrenceSchedule::new(
        rule,
        timezone,
        start_on,
        available_local_time,
        due_policy,
    ))
}

fn local_timezone() -> Result<TimeZoneId> {
    let value = iana_time_zone::get_timezone().context(
        "error local-time-zone-unavailable hint=\"pass --time-zone with an IANA zone such as Europe/Stockholm\"",
    )?;
    parse_timezone(&value)
}

fn parse_timezone(value: &str) -> Result<TimeZoneId> {
    value.parse().map_err(Into::into)
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").with_context(|| {
        format!(
            "error invalid-recurrence-date value={value:?} hint=\"use a real calendar date in YYYY-MM-DD form\""
        )
    })
}

fn parse_repeat_time(value: &str) -> Result<Option<NaiveTime>> {
    if value == "none" {
        return Ok(None);
    }
    if value.len() != 5 || value.as_bytes().get(2) != Some(&b':') {
        bail!("error invalid-repeat-at value={value:?} hint=\"use HH:MM or none\"");
    }
    NaiveTime::parse_from_str(value, "%H:%M")
        .map(Some)
        .with_context(|| {
            format!("error invalid-repeat-at value={value:?} hint=\"use a valid 24-hour time such as 09:00\"")
        })
}

fn parse_due_policy(value: &str) -> Result<RecurrenceDuePolicy> {
    match value {
        "same-day" => Ok(RecurrenceDuePolicy::SameDay),
        "none" => Ok(RecurrenceDuePolicy::None),
        _ => bail!("error invalid-repeat-due value={value:?} hint=\"use same-day or none\""),
    }
}

fn parse_rule(value: &str, start_on: NaiveDate) -> Result<RecurrenceRule> {
    match value {
        "daily" => return Ok(RecurrenceRule::daily()),
        "weekdays" => return Ok(RecurrenceRule::weekdays()),
        "weekly" => return Ok(RecurrenceRule::weekly(start_on.weekday())),
        "fortnightly" => {
            return RecurrenceRule::every_n_weeks_on(2, [start_on.weekday()]).map_err(Into::into);
        }
        "monthly" => return Ok(RecurrenceRule::monthly()),
        _ => {}
    }
    if let Some(days) = value.strip_prefix("weekly on ") {
        let weekdays = days.parse::<WeekdaySet>().map_err(anyhow::Error::msg)?;
        return RecurrenceRule::weekly_on(weekdays.iter()).map_err(Into::into);
    }
    let words = value.split(' ').collect::<Vec<_>>();
    if let ["every", interval, "weeks"] = words.as_slice() {
        let interval = parse_week_interval(interval)?;
        return RecurrenceRule::every_n_weeks_on(interval, [start_on.weekday()])
            .map_err(Into::into);
    }
    if let ["every", interval, "weeks", "on", days] = words.as_slice() {
        let interval = parse_week_interval(interval)?;
        let weekdays = days.parse::<WeekdaySet>().map_err(anyhow::Error::msg)?;
        return RecurrenceRule::every_n_weeks_on(interval, weekdays.iter()).map_err(Into::into);
    }
    bail!(
        "error invalid-repeat-rule value={value:?} hint=\"use daily, weekdays, weekly, fortnightly, monthly, weekly on mon,wed,fri, every N weeks, or every N weeks on mon,thu\""
    )
}

fn parse_week_interval(value: &str) -> Result<u32> {
    value.parse::<u32>().with_context(|| {
        format!(
            "error invalid-repeat-interval value={value:?} hint=\"use a whole number of weeks\""
        )
    })
}

async fn list(database: &Database, workspace: &Workspace, args: RecurListArgs) -> Result<()> {
    let items = database.list_recurrence_series(&workspace.id).await?;
    if args.json {
        print_json_pretty(&SeriesListJson {
            version: REPORT_VERSION,
            kind: "recurrence_series_list",
            series: items.iter().map(series_summary_json).collect(),
        })?;
    } else {
        for item in &items {
            print_series_summary(item);
        }
    }
    Ok(())
}

async fn show(database: &Database, workspace: &Workspace, args: RecurShowArgs) -> Result<()> {
    let series = database
        .resolve_recurrence_ref(workspace, &args.series_ref)
        .await?;
    let detail = database
        .recurrence_series_detail(&workspace.id, &series.id)
        .await?;
    let project = database
        .resolve_project_for_stored_value(&workspace.id, detail.series.project_id.as_str())
        .await?;
    if args.json {
        print_json_pretty(&SeriesShowJson {
            version: REPORT_VERSION,
            kind: "recurrence_series",
            series: series_detail_json(&detail, &project.key),
        })?;
    } else {
        print_series_summary(&detail.summary);
        println!("id={}", detail.series.id);
        println!("project={} labels={}", project.key, detail.labels.join(","));
        println!(
            "initial_status={} priority={} start_on={} timezone={} available_at={} due={}",
            detail.series.initial_status,
            detail.series.priority,
            detail.series.start_on,
            detail.series.timezone,
            format_local_time(detail.series.available_local_time),
            due_policy_label(detail.series.due_policy),
        );
        println!(
            "created={} updated={} stopped_at={}",
            detail.series.created_at,
            detail.series.updated_at,
            detail.series.stopped_at.as_deref().unwrap_or("")
        );
        if !detail.series.description.is_empty() {
            print_multiline_block("description", &detail.series.description);
        }
        for conflict in &detail.lifecycle_conflicts {
            println!(
                "conflict {} field={} variants={},{} lifecycle_blocked=yes",
                detail.summary.series_ref, conflict.field, conflict.variant_a, conflict.variant_b
            );
            println!(
                "variant {} value={}",
                conflict.variant_a,
                quote(&conflict.local_value)
            );
            println!(
                "variant {} value={}",
                conflict.variant_b,
                quote(&conflict.remote_value)
            );
        }
    }
    Ok(())
}

async fn history(database: &Database, workspace: &Workspace, args: RecurHistoryArgs) -> Result<()> {
    let series = database
        .resolve_recurrence_ref(workspace, &args.series_ref)
        .await?;
    let page = database
        .recurrence_history(&workspace.id, &series.id, args.offset, args.limit)
        .await?;
    if args.json {
        print_json_pretty(&HistoryJson {
            version: REPORT_VERSION,
            kind: "recurrence_history",
            history: history_page_json(&page),
        })?;
    } else {
        println!(
            "history {} offset={} limit={} total={} has_more={}",
            page.series_ref, page.offset, page.limit, page.total, page.has_more
        );
        for item in &page.items {
            print_history_entry(item);
        }
    }
    Ok(())
}

async fn edit(database: &Database, workspace: &Workspace, args: RecurEditArgs) -> Result<()> {
    let series = database
        .resolve_recurrence_ref(workspace, &args.series_ref)
        .await?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?;
    let available_local_time = args
        .repeat_at
        .as_deref()
        .map(parse_repeat_time)
        .transpose()?;
    let due_policy = args
        .repeat_due
        .as_deref()
        .map(parse_due_policy)
        .transpose()?;
    let outcome = database
        .update_recurrence_template(
            workspace,
            &series.id,
            RecurrenceTemplateUpdate {
                title: args.title,
                description,
                project: args.project,
                priority: args.priority,
                initial_status: args.status,
                labels: (!args.label.is_empty()).then_some(args.label),
                available_local_time,
                due_policy,
            },
        )
        .await?;
    let series_ref = database
        .recurrence_series_ref(&workspace.id, &series.id)
        .await?;
    println!(
        "updated {} changed={} state={} title={}",
        series_ref,
        changed_text(outcome.changed),
        outcome.series.state.as_str(),
        quote(&outcome.series.title)
    );
    Ok(())
}

async fn skip(database: &Database, workspace: &Workspace, args: RecurRefArgs) -> Result<()> {
    let series = database
        .resolve_recurrence_ref(workspace, &args.series_ref)
        .await?;
    let detail = database
        .recurrence_series_detail(&workspace.id, &series.id)
        .await?;
    let occurrence = detail
        .current_occurrence
        .context("error recurrence-current-occurrence-missing")?;
    let task_id = occurrence
        .task_id
        .context("error recurrence-current-occurrence-taskless")?;
    let outcome = database
        .resolve_recurrence_occurrence(workspace, &task_id, RecurrenceOutcome::Skipped)
        .await?;
    let series_ref = database
        .recurrence_series_ref(&workspace.id, &outcome.series.id)
        .await?;
    let display_refs = database.display_ref_context(&workspace.id).await?;
    println!(
        "skipped {} slot={} occurrence={} successor={}",
        series_ref,
        outcome.resolved.slot_on,
        display_refs.display_ref(&outcome.task),
        outcome
            .successor
            .as_ref()
            .map(|task| display_refs.display_ref(task))
            .unwrap_or_default()
    );
    Ok(())
}

async fn pause(database: &Database, workspace: &Workspace, args: RecurRefArgs) -> Result<()> {
    let series = database
        .resolve_recurrence_ref(workspace, &args.series_ref)
        .await?;
    let outcome = database
        .pause_recurrence_series(workspace, &series.id)
        .await?;
    print_state_outcome(
        database,
        workspace,
        &outcome.series,
        outcome.occurrence.as_ref(),
    )
    .await
}

async fn resume(database: &Database, workspace: &Workspace, args: RecurRefArgs) -> Result<()> {
    let series = database
        .resolve_recurrence_ref(workspace, &args.series_ref)
        .await?;
    let outcome = database
        .resume_recurrence_series(workspace, &series.id, utc_now()?)
        .await?;
    print_state_outcome(
        database,
        workspace,
        &outcome.series,
        outcome.occurrence.as_ref(),
    )
    .await
}

async fn stop(database: &Database, workspace: &Workspace, args: RecurStopArgs) -> Result<()> {
    let series = database
        .resolve_recurrence_ref(workspace, &args.series_ref)
        .await?;
    let outcome = database
        .stop_recurrence_series(workspace, &series.id, args.skip_current)
        .await?;
    print_state_outcome(
        database,
        workspace,
        &outcome.series,
        outcome.occurrence.as_ref(),
    )
    .await
}

async fn print_state_outcome(
    database: &Database,
    workspace: &Workspace,
    series: &RecurrenceSeries,
    occurrence: Option<&aven_core::types::RecurrenceOccurrence>,
) -> Result<()> {
    let series_ref = database
        .recurrence_series_ref(&workspace.id, &series.id)
        .await?;
    println!(
        "{} {} current_slot={} current_task={}",
        series.state.as_str(),
        series_ref,
        occurrence
            .map(|item| item.slot_on.to_string())
            .unwrap_or_default(),
        occurrence
            .and_then(|item| item.task_id.as_ref())
            .map(|id| id.as_str())
            .unwrap_or("")
    );
    Ok(())
}

fn utc_now() -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(&aven_core::ids::now())?.with_timezone(&Utc))
}

fn print_series_summary(item: &RecurrenceSeriesSummary) {
    let counts = &item.counts;
    let line = KvLine::new(item.series_ref.clone())
        .field("state", item.series.state.as_str())
        .quoted("rule", &item.rule_label)
        .field("timezone", item.series.timezone.as_str())
        .optional("current", item.current_task_ref.clone())
        .optional("slot", item.current_slot_on.clone())
        .field("completed", counts.completed)
        .field("skipped", counts.skipped)
        .field("missed", counts.missed)
        .quoted("title", &item.series.title)
        .finish();
    println!("{line}");
}

fn print_history_entry(item: &RecurrenceHistoryEntry) {
    match item.kind {
        RecurrenceHistoryKind::Paused => {
            let line = KvLine::new("paused")
                .field(
                    "from",
                    item.interval_started_at.as_deref().unwrap_or_default(),
                )
                .field("until", item.interval_ended_at.as_deref().unwrap_or("open"))
                .optional("task", item.task_ref.clone())
                .finish();
            println!("{line}");
        }
        _ => {
            let line = KvLine::new(history_kind_label(item.kind))
                .field("slot", item.slot_on.as_deref().unwrap_or_default())
                .optional("task", item.task_ref.clone())
                .optional("resolved_at", item.resolved_at.clone())
                .optional(
                    "archived_projection",
                    item.archived_projection.then(|| "yes".to_string()),
                )
                .field("openable", if item.openable { "yes" } else { "no" })
                .finish();
            println!("{line}");
        }
    }
}

fn history_kind_label(kind: RecurrenceHistoryKind) -> &'static str {
    match kind {
        RecurrenceHistoryKind::Completed => "completed",
        RecurrenceHistoryKind::Skipped => "skipped",
        RecurrenceHistoryKind::Missed => "missed",
        RecurrenceHistoryKind::Paused => "paused",
    }
}

fn due_policy_label(value: RecurrenceDuePolicy) -> &'static str {
    match value {
        RecurrenceDuePolicy::SameDay => "same-day",
        RecurrenceDuePolicy::None => "none",
    }
}

fn format_local_time(value: Option<NaiveTime>) -> String {
    value
        .map(|value| value.format("%H:%M").to_string())
        .unwrap_or_else(|| "slot-boundary".to_string())
}

#[derive(Serialize)]
struct SeriesListJson {
    version: u32,
    kind: &'static str,
    series: Vec<SeriesSummaryJson>,
}

#[derive(Serialize)]
struct SeriesShowJson {
    version: u32,
    kind: &'static str,
    series: SeriesDetailJson,
}

#[derive(Serialize)]
struct HistoryJson {
    version: u32,
    kind: &'static str,
    history: HistoryPageJson,
}

#[derive(Serialize)]
struct SeriesSummaryJson {
    r#ref: String,
    id: String,
    title: String,
    state: String,
    rule: String,
    timezone: String,
    start_on: String,
    available_at: String,
    due: String,
    current_task_ref: Option<String>,
    current_slot_on: Option<String>,
    counts: CountsJson,
}

#[derive(Serialize)]
struct SeriesDetailJson {
    #[serde(flatten)]
    summary: SeriesSummaryJson,
    description: String,
    project: String,
    priority: String,
    initial_status: String,
    labels: Vec<String>,
    stopped_at: Option<String>,
    created_at: String,
    updated_at: String,
    lifecycle_conflicts: Vec<SeriesConflictJson>,
}

#[derive(Serialize)]
struct SeriesConflictJson {
    field: String,
    variants: Vec<SeriesConflictVariantJson>,
}

#[derive(Serialize)]
struct SeriesConflictVariantJson {
    token: String,
    value: String,
}

#[derive(Serialize)]
struct CountsJson {
    completed: usize,
    skipped: usize,
    missed: usize,
    pause_intervals: usize,
    latest_slot_on: Option<String>,
    latest_outcome: Option<String>,
}

#[derive(Serialize)]
struct HistoryPageJson {
    series_ref: String,
    entries: Vec<HistoryEntryJson>,
    offset: usize,
    limit: usize,
    total: usize,
    has_more: bool,
}

#[derive(Serialize)]
struct HistoryEntryJson {
    outcome: String,
    slot_on: Option<String>,
    interval_started_at: Option<String>,
    interval_ended_at: Option<String>,
    task_ref: Option<String>,
    task_id: Option<String>,
    openable: bool,
    archived_projection: bool,
    resolved_at: Option<String>,
}

fn series_summary_json(item: &RecurrenceSeriesSummary) -> SeriesSummaryJson {
    SeriesSummaryJson {
        r#ref: item.series_ref.clone(),
        id: item.series.id.to_string(),
        title: item.series.title.clone(),
        state: item.series.state.as_str().to_string(),
        rule: item.rule_label.clone(),
        timezone: item.series.timezone.to_string(),
        start_on: item.series.start_on.to_string(),
        available_at: format_local_time(item.series.available_local_time),
        due: due_policy_label(item.series.due_policy).to_string(),
        current_task_ref: item.current_task_ref.clone(),
        current_slot_on: item.current_slot_on.clone(),
        counts: counts_json(&item.counts),
    }
}

fn series_detail_json(item: &RecurrenceSeriesDetail, project: &str) -> SeriesDetailJson {
    SeriesDetailJson {
        summary: series_summary_json(&item.summary),
        description: item.series.description.clone(),
        project: project.to_string(),
        priority: item.series.priority.to_string(),
        initial_status: item.series.initial_status.to_string(),
        labels: item.labels.clone(),
        stopped_at: item.series.stopped_at.clone(),
        created_at: item.series.created_at.clone(),
        updated_at: item.series.updated_at.clone(),
        lifecycle_conflicts: item
            .lifecycle_conflicts
            .iter()
            .map(|conflict| SeriesConflictJson {
                field: conflict.field.clone(),
                variants: vec![
                    SeriesConflictVariantJson {
                        token: conflict.variant_a.clone(),
                        value: conflict.local_value.clone(),
                    },
                    SeriesConflictVariantJson {
                        token: conflict.variant_b.clone(),
                        value: conflict.remote_value.clone(),
                    },
                ],
            })
            .collect(),
    }
}

fn counts_json(item: &RecurrenceCounts) -> CountsJson {
    CountsJson {
        completed: item.completed,
        skipped: item.skipped,
        missed: item.missed,
        pause_intervals: item.pause_intervals,
        latest_slot_on: item.latest_slot_on.clone(),
        latest_outcome: item.latest_outcome.map(|value| value.as_str().to_string()),
    }
}

fn history_page_json(item: &RecurrenceHistoryPage) -> HistoryPageJson {
    HistoryPageJson {
        series_ref: item.series_ref.clone(),
        entries: item
            .items
            .iter()
            .map(|entry| HistoryEntryJson {
                outcome: history_kind_label(entry.kind).to_string(),
                slot_on: entry.slot_on.clone(),
                interval_started_at: entry.interval_started_at.clone(),
                interval_ended_at: entry.interval_ended_at.clone(),
                task_ref: entry.task_ref.clone(),
                task_id: entry.task_id.as_ref().map(ToString::to_string),
                openable: entry.openable,
                archived_projection: entry.archived_projection,
                resolved_at: entry.resolved_at.clone(),
            })
            .collect(),
        offset: item.offset,
        limit: item.limit,
        total: item.total,
        has_more: item.has_more,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_fixed_rule_grammar() {
        let monday = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        for rule in [
            "daily",
            "weekdays",
            "weekly",
            "fortnightly",
            "monthly",
            "weekly on mon,wed,fri",
            "every 2 weeks",
            "every 2 weeks on tue",
            "every 3 weeks on mon,thu",
        ] {
            assert!(parse_rule(rule, monday).is_ok(), "{rule}");
        }
        assert_eq!(
            parse_rule("fortnightly", monday).unwrap(),
            RecurrenceRule::every_n_weeks_on(2, [chrono::Weekday::Mon]).unwrap()
        );
        assert_eq!(
            parse_rule("every 3 weeks", monday).unwrap(),
            RecurrenceRule::every_n_weeks_on(3, [chrono::Weekday::Mon]).unwrap()
        );
        for rule in [
            "every 3 days",
            "every 0 weeks",
            "weekly on monday",
            "weekly on fri,mon",
            "every two weeks on tue",
            "daily ",
        ] {
            assert!(parse_rule(rule, monday).is_err(), "{rule}");
        }
    }
}
