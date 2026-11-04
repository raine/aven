//! The Recent actions ledger: days, then bursts of activity, then runs of
//! actions on one task.

use aven_core::change_log::op_type;
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::query::RecentActionItem;
use chrono::{DateTime, Local, NaiveDate, TimeDelta, Timelike as _};
use std::collections::{HashMap, HashSet};

use super::model::{Ledger, LedgerLine, LedgerRun, LedgerStep, StepMark};
use super::rows::{available_text, capitalised, due_text, parse_local_timestamp};
use crate::kit::{priority_name, status_name};

/// A pause this long between two actions starts a new burst.
const BURST_GAP: TimeDelta = TimeDelta::minutes(40);

/// The last known status and priority of each entity, as of an action.
type Known = HashMap<String, (Option<TaskStatus>, Option<TaskPriority>)>;

/// Groups actions, newest first, into the ledger's lines.
pub(super) fn ledger(actions: &[RecentActionItem], now: DateTime<Local>) -> Ledger {
    let today = now.date_naive();
    let timed: Vec<(DateTime<Local>, &RecentActionItem)> = actions
        .iter()
        .filter_map(|action| Some((parse_local_timestamp(&action.created_at)?, action)))
        .collect();
    let steps = steps(&timed, today);
    let mut lines = Vec::new();
    let mut day_line = 0;
    let mut burst: Vec<usize> = Vec::new();
    let mut touched = HashSet::new();
    let mut counted = 0;
    for (ix, (at, action)) in timed.iter().enumerate() {
        let day = at.date_naive();
        if day == today {
            counted += 1;
            touched.insert(action.entity_id.as_str());
        }
        let previous = ix.checked_sub(1).map(|ix| timed[ix].0);
        let new_day = previous.is_none_or(|previous| previous.date_naive() != day);
        let new_burst = new_day || previous.is_some_and(|previous| previous - *at >= BURST_GAP);
        if new_burst {
            close_burst(&mut lines, &timed, &steps, &burst, now);
            burst.clear();
        }
        if new_day {
            day_line = lines.len();
            lines.push(LedgerLine::Day {
                label: day_label(day, today),
                count: 0,
            });
        }
        if let LedgerLine::Day { count, .. } = &mut lines[day_line] {
            *count += 1;
        }
        burst.push(ix);
    }
    close_burst(&mut lines, &timed, &steps, &burst, now);
    Ledger {
        lines,
        today: (counted, touched.len()),
    }
}

/// Adds a burst's heading and its runs.
fn close_burst(
    lines: &mut Vec<LedgerLine>,
    timed: &[(DateTime<Local>, &RecentActionItem)],
    steps: &[LedgerStep],
    burst: &[usize],
    now: DateTime<Local>,
) {
    let (Some(&newest), Some(&oldest)) = (burst.first(), burst.last()) else {
        return;
    };
    let (last, first) = (timed[newest].0, timed[oldest].0);
    let range = if clock(first) == clock(last) {
        clock(last)
    } else {
        format!("{}–{}", clock(first), clock(last))
    };
    lines.push(LedgerLine::Burst {
        label: burst_label(last, now),
        range,
    });
    let mut start = 0;
    while start < burst.len() {
        let entity = &timed[burst[start]].1.entity_id;
        let end = burst[start..]
            .iter()
            .position(|&ix| timed[ix].1.entity_id != *entity)
            .map_or(burst.len(), |len| start + len);
        let run = &burst[start..end];
        lines.push(LedgerLine::Run(ledger_run(
            timed[run[0]],
            run.iter().map(|&ix| steps[ix].clone()).collect(),
        )));
        start = end;
    }
}

fn ledger_run(
    (at, action): (DateTime<Local>, &RecentActionItem),
    steps: Vec<LedgerStep>,
) -> LedgerRun {
    let target = &action.target;
    let status = target
        .status
        .as_deref()
        .and_then(|s| TaskStatus::parse(s).ok());
    let task = (action.entity_type == "task")
        .then(|| action.entity_id.parse().ok())
        .flatten();
    LedgerRun {
        key: action.change_id.clone(),
        clock: clock(at),
        task,
        reference: target.display_ref.clone().unwrap_or_default(),
        project_key: target.project_key.clone().unwrap_or_default(),
        title: target.title.clone().unwrap_or_default(),
        closed: status.is_some_and(TaskStatus::is_terminal),
        deleted: target.deleted,
        steps,
    }
}

/// Each action as a step, reading its previous status or priority from the
/// older actions still in the list.
fn steps(timed: &[(DateTime<Local>, &RecentActionItem)], today: NaiveDate) -> Vec<LedgerStep> {
    let mut known = Known::new();
    let mut steps: Vec<LedgerStep> = timed
        .iter()
        .rev()
        .map(|(_, action)| step(action, &mut known, today))
        .collect();
    steps.reverse();
    steps
}

fn step(action: &RecentActionItem, known: &mut Known, today: NaiveDate) -> LedgerStep {
    let detail = action.detail.clone().filter(|detail| !detail.is_empty());
    let entry = known.entry(action.entity_id.clone()).or_default();
    let plain = |icon: &'static str, lead: &str, short: &str| LedgerStep {
        mark: StepMark::Icon(icon),
        lead: lead.to_string(),
        value: None,
        chip: None,
        short: short.to_string(),
        tone: None,
    };
    let named = |icon, lead: &str, short, chip: Option<String>| LedgerStep {
        chip,
        ..plain(icon, lead, short)
    };
    if action.entity_type != "task" {
        let title = action.target.title.as_deref().unwrap_or_default();
        let summary = capitalised(&action.task_activity_summary(title));
        let icon = match action.verb.as_str() {
            "project" => "icons/ic-projects.svg",
            "label" => "icons/ic-labels.svg",
            "recurrence" => "icons/ic-recurring.svg",
            _ => "icons/ra-edit.svg",
        };
        return plain(icon, &summary, "Edited");
    }
    let field = action.field.as_deref();
    match (action.op_type.as_str(), field) {
        (op_type::CREATE_TASK, _) => {
            let status = action
                .initial_status
                .as_deref()
                .and_then(|status| TaskStatus::parse(status).ok());
            entry.0 = status;
            let lead = status.map_or_else(
                || "Created".to_string(),
                |status| format!("Created in {}", status_name(status)),
            );
            LedgerStep {
                mark: StepMark::Created,
                ..plain("icons/ra-create.svg", &lead, "Created")
            }
        }
        (op_type::SET_FIELD, Some("status")) => {
            let Some(to) = detail.as_deref().and_then(|s| TaskStatus::parse(s).ok()) else {
                return plain("icons/ra-edit.svg", "Status changed", "Status");
            };
            let lead = match entry.0.replace(to) {
                Some(from) => format!("{} →", status_name(from)),
                None => "Moved to".to_string(),
            };
            LedgerStep {
                mark: StepMark::Status(to),
                lead,
                value: Some(status_name(to).to_string()),
                chip: None,
                short: status_name(to).to_string(),
                tone: Some(to),
            }
        }
        (op_type::SET_FIELD, Some("priority")) => {
            let Some(to) = detail.as_deref().and_then(|s| TaskPriority::parse(s).ok()) else {
                return plain("icons/ra-edit.svg", "Priority changed", "Priority");
            };
            let lead = match entry.1.replace(to) {
                Some(from) => format!("Priority {} →", priority_name(from)),
                None => "Priority".to_string(),
            };
            LedgerStep {
                mark: StepMark::Priority(to),
                lead,
                value: Some(priority_name(to).to_string()),
                chip: None,
                short: "Priority".to_string(),
                tone: None,
            }
        }
        (op_type::SET_FIELD, Some("title")) => plain("icons/ra-edit.svg", "Renamed", "Renamed"),
        (op_type::SET_FIELD, Some("description")) => {
            let lead = if detail.is_some() {
                "Description edited"
            } else {
                "Description cleared"
            };
            plain("icons/ra-desc.svg", lead, "Description")
        }
        (op_type::SET_FIELD, Some("available_at")) => match detail {
            Some(at) => plain(
                "icons/ic-upcoming.svg",
                &format!("Deferred to {}", available_text(&at, today)),
                "Deferred",
            ),
            None => plain("icons/ic-upcoming.svg", "Available now", "Available"),
        },
        (op_type::SET_FIELD, Some("due_on")) => match detail {
            Some(due) => plain(
                "icons/ic-overdue.svg",
                &format!("Due {}", due_text(&due, today)),
                "Due date",
            ),
            None => plain("icons/ic-overdue.svg", "Due date cleared", "Due date"),
        },
        (op_type::SET_FIELD, Some("project")) => plain(
            "icons/ic-projects.svg",
            &format!("Moved to {}", detail.unwrap_or_default()),
            "Project",
        ),
        (op_type::SET_FIELD, Some("deleted")) => {
            let deleted = action.summary.starts_with("deleted");
            let lead = if deleted { "Deleted" } else { "Restored" };
            plain("icons/ic-trash.svg", lead, lead)
        }
        (op_type::SET_FIELD, Some("is_epic")) => {
            let lead = if action.summary.starts_with("marked") {
                "Marked as epic"
            } else {
                "No longer an epic"
            };
            plain("icons/ic-epics.svg", lead, "Epic")
        }
        (op_type::NOTE_ADD, _) => plain("icons/ra-note.svg", "Note added", "Note"),
        (op_type::NOTE_EDIT, _) => plain("icons/ra-note.svg", "Note edited", "Note"),
        (op_type::NOTE_DELETE, _) => plain("icons/ra-note.svg", "Note deleted", "Note deleted"),
        (op_type::ATTACHMENT_ADD, _) => {
            named("icons/ra-attach.svg", "Attached", "Attachment", detail)
        }
        (op_type::ATTACHMENT_DELETE, _) => named(
            "icons/ra-attach.svg",
            "Attachment removed",
            "Attachment removed",
            detail,
        ),
        (op_type::LABEL_ADD, _) => named("icons/ic-labels.svg", "Label added", "Label", detail),
        (op_type::LABEL_REMOVE, _) => {
            named("icons/ic-labels.svg", "Label removed", "Label", detail)
        }
        (op_type::DEPENDENCY_ADD, _) => {
            named("icons/ic-blocked.svg", "Waits on", "Blocker", detail)
        }
        (op_type::DEPENDENCY_REMOVE, _) => named(
            "icons/ic-blocked.svg",
            "No longer waits on",
            "Blocker",
            detail,
        ),
        (op_type::EPIC_LINK_ADD, _) => named("icons/ic-epics.svg", "Added to epic", "Epic", detail),
        (op_type::EPIC_LINK_REMOVE, _) => {
            named("icons/ic-epics.svg", "Removed from epic", "Epic", detail)
        }
        (op_type::RELATED_ADD, _) => named("icons/ra-edit.svg", "Related to", "Related", detail),
        (op_type::RELATED_REMOVE, _) => named(
            "icons/ra-edit.svg",
            "No longer related to",
            "Related",
            detail,
        ),
        (op_type::RESOLVE_FIELD, _) => {
            let title = action.target.title.as_deref().unwrap_or_default();
            let summary = capitalised(&action.task_activity_summary(title));
            plain("icons/ic-conflicts.svg", &summary, "Conflict")
        }
        _ => {
            let title = action.target.title.as_deref().unwrap_or_default();
            let summary = capitalised(&action.task_activity_summary(title));
            plain("icons/ra-edit.svg", &summary, "Edited")
        }
    }
}

fn clock(at: DateTime<Local>) -> String {
    at.format("%H:%M").to_string()
}

fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    match (today - day).num_days() {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        _ => day.format("%A, %b %-d").to_string(),
    }
}

/// How long ago a burst ended, or the part of the day it was in once that is
/// no longer today.
fn burst_label(last: DateTime<Local>, now: DateTime<Local>) -> String {
    let minutes = (now - last).num_minutes();
    if minutes < 45 {
        return "Just now".to_string();
    }
    if last.date_naive() == now.date_naive() {
        let hours = (minutes as f64 / 60.).round() as i64;
        return format!("{hours} {} ago", if hours == 1 { "hour" } else { "hours" });
    }
    match last.hour() {
        0..12 => "Morning",
        12..18 => "Afternoon",
        _ => "Evening",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use aven_core::query::RecentActionTarget;
    use chrono::TimeZone as _;

    use super::*;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .unwrap()
    }

    fn action(
        entity: &str,
        when: DateTime<Local>,
        op: &str,
        field: Option<&str>,
        detail: Option<&str>,
    ) -> RecentActionItem {
        RecentActionItem {
            change_id: format!("{entity}-{}", when.timestamp()),
            entity_type: "task".to_string(),
            entity_id: entity.to_string(),
            op_type: op.to_string(),
            field: field.map(str::to_string),
            created_at: when.to_rfc3339(),
            synced: false,
            target: RecentActionTarget {
                display_ref: Some(format!("AVN-{entity}")),
                title: Some(format!("Task {entity}")),
                project_key: Some("aven".to_string()),
                status: Some("todo".to_string()),
                deleted: false,
            },
            verb: String::new(),
            summary: String::new(),
            detail: detail.map(str::to_string),
            accent: String::new(),
            grouped_change_count: 1,
            initial_status: None,
        }
    }

    fn status(entity: &str, when: DateTime<Local>, to: &str) -> RecentActionItem {
        action(entity, when, op_type::SET_FIELD, Some("status"), Some(to))
    }

    fn runs(ledger: &Ledger) -> Vec<&LedgerRun> {
        ledger
            .lines
            .iter()
            .filter_map(|line| match line {
                LedgerLine::Run(run) => Some(run),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn days_and_pauses_split_the_ledger() {
        let now = at(4, 17, 20);
        let actions = [
            status("A", at(4, 17, 13), "done"),
            status("B", at(4, 17, 2), "done"),
            status("C", at(4, 15, 14), "active"),
            status("D", at(3, 16, 14), "active"),
        ];
        let ledger = ledger(&actions, now);
        let heads: Vec<String> = ledger
            .lines
            .iter()
            .filter_map(|line| match line {
                LedgerLine::Day { label, count } => Some(format!("{label} {count}")),
                LedgerLine::Burst { label, range } => Some(format!("{label} {range}")),
                LedgerLine::Run(_) => None,
            })
            .collect();
        assert_eq!(
            heads,
            [
                "Today 3",
                "Just now 17:02–17:13",
                "2 hours ago 15:14",
                "Yesterday 1",
                "Afternoon 16:14",
            ]
        );
        assert_eq!(ledger.today, (3, 3));
    }

    #[test]
    fn consecutive_actions_on_a_task_form_one_run() {
        let now = at(4, 12, 0);
        let mut created = action("A", at(4, 11, 50), op_type::CREATE_TASK, None, None);
        created.initial_status = Some("todo".to_string());
        let actions = [
            status("A", at(4, 11, 58), "done"),
            status("A", at(4, 11, 55), "active"),
            created,
            status("B", at(4, 11, 40), "active"),
        ];
        let ledger = ledger(&actions, now);
        let runs = runs(&ledger);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].clock, "11:58");
        let leads: Vec<&str> = runs[0].steps.iter().map(|s| s.lead.as_str()).collect();
        assert_eq!(leads, ["Active →", "Todo →", "Created in Todo"]);
        assert_eq!(runs[1].steps[0].lead, "Moved to");
        assert_eq!(ledger.today, (4, 2));
    }
}
