use std::collections::BTreeMap;

use anyhow::Result;
use aven_core::db::Database;
use serde::Serialize;

use crate::cli::PrimeArgs;
use crate::query::{
    self, SortDirection, TaskAvailabilityFilter, TaskFilters, TaskQueryMode, TaskSort,
};
use crate::render::{print_json_pretty, quote};
use crate::workspaces::Workspace;

pub(crate) async fn run(database: &Database, workspace: &Workspace, args: PrimeArgs) -> Result<()> {
    let report = build_report(database, workspace, args.project.as_deref(), args.limit).await?;
    if args.json {
        print_json_pretty(&report)?;
    } else {
        PrimeTextRenderer::print(&report);
    }
    Ok(())
}

#[derive(Serialize)]
struct PrimeReport {
    project: Option<String>,
    unavailable_reason: Option<String>,
    open_issue_sample: usize,
    conventions: PrimeConventions,
    top_blockers: Vec<PrimeBlocker>,
    active: Vec<PrimeIssue>,
    ready: Vec<PrimeIssue>,
    blocked: Vec<PrimeIssue>,
}

#[derive(Default, Serialize)]
struct PrimeConventions {
    title_style: String,
    statuses: String,
    labels: String,
}

#[derive(Serialize)]
struct PrimeBlocker {
    r#ref: String,
    blocks: i64,
}

#[derive(Serialize)]
struct PrimeIssue {
    r#ref: String,
    status: String,
    priority: String,
    labels: Vec<String>,
    title: String,
    is_epic: bool,
    epic_parent_ref: Option<String>,
    epic_child_refs: Vec<String>,
    blocked_by_refs: Vec<String>,
    blocks_refs: Vec<String>,
}

impl PrimeReport {
    fn unavailable(project: Option<String>, reason: &str) -> Self {
        Self {
            project,
            unavailable_reason: Some(reason.to_string()),
            open_issue_sample: 0,
            conventions: PrimeConventions::default(),
            top_blockers: Vec::new(),
            active: Vec::new(),
            ready: Vec::new(),
            blocked: Vec::new(),
        }
    }
}

async fn build_report(
    database: &Database,
    workspace: &Workspace,
    project_arg: Option<&str>,
    limit: Option<usize>,
) -> Result<PrimeReport> {
    let project = if let Some(project) = project_arg {
        Some(
            database
                .resolve_existing_project(&workspace.id, project)
                .await?
                .key,
        )
    } else {
        crate::projects::inferred_project_key_for_add_with_database(database, workspace).await?
    };

    let Some(project) = project else {
        return Ok(PrimeReport::unavailable(
            None,
            "No current project could be inferred. Run with --project <project>.",
        ));
    };

    if database
        .find_project(&workspace.id, &project)
        .await?
        .is_none()
    {
        return Ok(PrimeReport::unavailable(Some(project), "No open issues."));
    }

    let items = database
        .list_task_summary_items(
            &workspace.id,
            prime_task_filters(project.clone()),
            TaskQueryMode::Flat,
            TaskSort::Updated,
            SortDirection::Desc,
            limit,
        )
        .await?;

    Ok(report_from_items(project, &items))
}

fn report_from_items(project: String, items: &[query::TaskListItem]) -> PrimeReport {
    let active = items
        .iter()
        .filter(|item| item.task.status.as_str() == "active")
        .map(prime_issue)
        .collect();
    let ready = items
        .iter()
        .filter(|item| item.task.status.as_str() != "active" && item.unresolved_blocker_count == 0)
        .map(prime_issue)
        .collect();
    let blocked = items
        .iter()
        .filter(|item| item.task.status.as_str() != "active" && item.unresolved_blocker_count > 0)
        .map(prime_issue)
        .collect();

    let mut top_blockers = items
        .iter()
        .filter(|item| item.dependent_count > 0)
        .map(|item| PrimeBlocker {
            r#ref: item.display_ref.clone(),
            blocks: item.dependent_count,
        })
        .collect::<Vec<_>>();
    top_blockers.sort_by(|left, right| {
        right
            .blocks
            .cmp(&left.blocks)
            .then_with(|| left.r#ref.cmp(&right.r#ref))
    });
    top_blockers.truncate(3);

    let status_counts = count_values(items.iter().map(|item| item.task.status.as_str()));
    let label_counts = count_values(
        items
            .iter()
            .flat_map(|item| item.labels.iter().map(String::as_str)),
    );

    PrimeReport {
        project: Some(project),
        unavailable_reason: None,
        open_issue_sample: items.len(),
        conventions: PrimeConventions {
            title_style: "sentence case".to_string(),
            statuses: format_counts(&status_counts, 4),
            labels: format_counts(&label_counts, 6),
        },
        top_blockers,
        active,
        ready,
        blocked,
    }
}

fn prime_issue(item: &query::TaskListItem) -> PrimeIssue {
    PrimeIssue {
        r#ref: item.display_ref.clone(),
        status: item.task.status.to_string(),
        priority: item.task.priority.to_string(),
        labels: item.labels.clone(),
        title: item.task.title.clone(),
        is_epic: item.task.is_epic,
        epic_parent_ref: item
            .epic_parent
            .as_ref()
            .map(|parent| parent.display_ref.clone()),
        epic_child_refs: item
            .epic_children
            .iter()
            .map(|child| child.display_ref.clone())
            .collect(),
        blocked_by_refs: unresolved_refs(&item.depends_on),
        blocks_refs: unresolved_refs(&item.blocks),
    }
}

fn unresolved_refs(links: &[query::TaskDependencyLink]) -> Vec<String> {
    links
        .iter()
        .filter(|link| link.unresolved)
        .map(|link| link.display_ref.clone())
        .collect()
}

fn count_values<'a>(values: impl Iterator<Item = &'a str>) -> Vec<(String, usize)> {
    let mut counts = BTreeMap::<String, usize>::new();
    for value in values {
        *counts.entry(value.to_string()).or_default() += 1;
    }
    let mut counts = counts.into_iter().collect::<Vec<_>>();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts
}

fn format_counts(counts: &[(String, usize)], limit: usize) -> String {
    counts
        .iter()
        .take(limit)
        .map(|(value, count)| format!("{value}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

struct PrimeTextRenderer;

impl PrimeTextRenderer {
    fn print(report: &PrimeReport) {
        Self::print_primer();
        Self::print_issue_workflow();

        if let Some(reason) = &report.unavailable_reason {
            println!("{reason}");
            return;
        }

        Self::print_conventions(report);
        Self::print_open_issues(report);
    }

    fn print_primer() {
        print!("{}", include_str!("../skill.md"));
        if !include_str!("../skill.md").ends_with('\n') {
            println!();
        }
        println!();
    }

    fn print_issue_workflow() {
        println!("## Issue Workflow");
        println!();
        println!("- Inspect an issue with `aven show <ref> --full` before changing it.");
        println!(
            "- Mark picked-up work with `aven edit <ref> --status active` before making changes."
        );
        println!(
            "- Add durable handoff context with `aven note <ref> ...` for blockers, decisions, or partial progress."
        );
        println!("- Leave blocked or unfinished work open and report the current state.");
        println!(
            "- Mark complete with `aven edit <ref> --status done` only after the requested work is complete and required code changes are committed."
        );
        println!("- Use `canceled` only when the user says the issue is no longer needed.");
        println!();
    }

    fn print_conventions(report: &PrimeReport) {
        println!("## Local Conventions");
        println!();
        println!(
            "Project: {}",
            report
                .project
                .as_deref()
                .expect("available report has project")
        );
        println!("Open issue sample: {}", report.open_issue_sample);
        println!(
            "Use {} for task titles: capitalize the first word and proper nouns, not every significant word.",
            report.conventions.title_style
        );
        if report.open_issue_sample == 0 {
            println!("No open issues are available for convention summaries.");
        } else {
            println!("Common statuses: {}.", report.conventions.statuses);
            if report.conventions.labels.is_empty() {
                println!("Common labels: none in open issue sample.");
            } else {
                println!("Common labels: {}.", report.conventions.labels);
            }
        }
        println!();
    }

    fn print_open_issues(report: &PrimeReport) {
        println!("## Open Issues");
        println!();
        if report.open_issue_sample == 0 {
            println!("No open issues.");
            return;
        }

        println!(
            "Summary: total={} active={} ready={} blocked={}",
            report.open_issue_sample,
            report.active.len(),
            report.ready.len(),
            report.blocked.len()
        );
        Self::print_top_blockers(&report.top_blockers);
        println!();
        Self::print_issue_section("Active", &report.active);
        Self::print_issue_section("Ready", &report.ready);
        Self::print_issue_section("Blocked", &report.blocked);
    }

    fn print_top_blockers(blockers: &[PrimeBlocker]) {
        if blockers.is_empty() {
            println!("Top blockers: none.");
            return;
        }
        let summary = blockers
            .iter()
            .map(|blocker| format!("{} blocks={}", blocker.r#ref, blocker.blocks))
            .collect::<Vec<_>>();
        println!("Top blockers: {}", summary.join(", "));
    }

    fn print_issue_section(label: &str, issues: &[PrimeIssue]) {
        println!("### {label}");
        if issues.is_empty() {
            println!("(none)");
            println!();
            return;
        }
        for issue in issues {
            println!("{}", format_issue_line(issue));
        }
        println!();
    }
}

fn format_issue_line(issue: &PrimeIssue) -> String {
    let mut fields = vec![format!("{} status={}", issue.r#ref, issue.status)];
    if issue.priority != "none" {
        fields.push(format!("priority={}", issue.priority));
    }
    if !issue.labels.is_empty() {
        fields.push(format!("labels={}", issue.labels.join(",")));
    }
    if let Some(dependencies) = format_dependency_refs(&issue.blocked_by_refs) {
        fields.push(format!("blocked_by={dependencies}"));
    }
    if let Some(dependents) = format_dependency_refs(&issue.blocks_refs) {
        fields.push(format!("blocks={dependents}"));
    }
    fields.push(format!("title={}", quote(&issue.title)));
    fields.join(" ")
}

fn format_dependency_refs(refs: &[String]) -> Option<String> {
    const REF_LIMIT: usize = 3;

    if refs.is_empty() {
        return None;
    }
    let mut parts = refs.iter().take(REF_LIMIT).cloned().collect::<Vec<_>>();
    if refs.len() > REF_LIMIT {
        parts.push(format!("+{}", refs.len() - REF_LIMIT));
    }
    Some(format!("[{}]", parts.join(",")))
}

fn prime_task_filters(project: String) -> TaskFilters {
    TaskFilters {
        hide_done: true,
        exclude_epics: true,
        availability: TaskAvailabilityFilter::Available,
        ..TaskFilters::default().with_project(Some(project))
    }
}
