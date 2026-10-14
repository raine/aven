//! Due digest and resurfacing reminders sent by the daemon.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use aven_core::db::Database;
use aven_core::local_state::ReminderLogEntry;
use aven_core::query::{SortDirection, TaskFilters, TaskQueryMode, TaskSort};
use aven_core::queue::{available_since_defer, unix_seconds};
use aven_core::types::Task;
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, Utc};
use tracing::warn;

use crate::config::RemindersConfig;
use crate::due::{DueState, due_state};

/// Upper bound on a reminder sleep, so sleep and clock changes are noticed.
pub(crate) const MAX_SLEEP: Duration = Duration::from_secs(15 * 60);
const CATCH_UP_WINDOW_SECONDS: i64 = 24 * 60 * 60;
const MAX_INDIVIDUAL_AVAILABLE: usize = 3;
const PREVIEW_TITLES: usize = 3;
/// Roughly one line of a macOS notification banner body.
const PREVIEW_LINE_CHARS: usize = 36;

const DIGEST: &str = "digest";
const AVAILABLE: &str = "available";
const SENT: &str = "sent";
const SUPPRESSED: &str = "suppressed";
const WAITING: &str = "waiting";
const EMPTY: &str = "empty";

/// An open, non-deleted task.
#[derive(Debug, Clone)]
pub(crate) struct ReminderTask {
    pub(crate) task: Task,
    pub(crate) display_ref: String,
    pub(crate) blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notification {
    pub(crate) title: String,
    pub(crate) message: String,
    /// Entries to record once the notification was sent.
    pub(crate) sent: Vec<ReminderLogEntry>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Entries to record before sending: `waiting`, `suppressed` and `empty`.
    pub(crate) record: Vec<ReminderLogEntry>,
    pub(crate) notifications: Vec<Notification>,
}

/// Decides which reminders are due at `now`, whose local wall-clock time is
/// `local`.
pub(crate) fn plan(
    tasks: &[ReminderTask],
    log: &[ReminderLogEntry],
    now: DateTime<Utc>,
    local: NaiveDateTime,
    config: &RemindersConfig,
    digest_time: NaiveTime,
) -> Plan {
    let outcomes: HashMap<(&str, &str, &str), &str> = log
        .iter()
        .map(|entry| {
            (
                (
                    entry.kind.as_str(),
                    entry.task_id.as_str(),
                    entry.trigger.as_str(),
                ),
                entry.outcome.as_str(),
            )
        })
        .collect();
    let mut plan = Plan::default();
    if config.due_digest && local.time() >= digest_time {
        plan_digest(tasks, &outcomes, local, &mut plan);
    }
    if config.available {
        plan_available(tasks, &outcomes, now, &mut plan);
    }
    plan
}

fn plan_digest(
    tasks: &[ReminderTask],
    outcomes: &HashMap<(&str, &str, &str), &str>,
    local: NaiveDateTime,
    plan: &mut Plan,
) {
    let today = local.date();
    let trigger = today.format("%Y-%m-%d").to_string();
    if outcomes.contains_key(&(DIGEST, "", trigger.as_str())) {
        return;
    }
    let mut due: Vec<(DueState, &ReminderTask)> = tasks
        .iter()
        .filter_map(|item| {
            let state = due_state(item.task.due_on.as_deref()?, today);
            state.needs_action().then_some((state, item))
        })
        .collect();
    let entry = |outcome: &str| ReminderLogEntry {
        kind: DIGEST.into(),
        task_id: String::new(),
        trigger: trigger.clone(),
        outcome: outcome.into(),
    };
    if due.is_empty() {
        plan.record.push(entry(EMPTY));
        return;
    }
    // Most overdue first, then due today.
    due.sort_by(|(left_state, left), (right_state, right)| {
        days_overdue(*right_state)
            .cmp(&days_overdue(*left_state))
            .then_with(|| left.task.title.cmp(&right.task.title))
    });
    let today_count = due
        .iter()
        .filter(|(state, _)| *state == DueState::Today)
        .count();
    let overdue_count = due.len() - today_count;
    let mut counts = Vec::new();
    if today_count > 0 {
        counts.push(format!("{today_count} due today"));
    }
    if overdue_count > 0 {
        counts.push(format!("{overdue_count} overdue"));
    }
    let list = preview(due.iter().map(|(_, item)| *item));
    plan.notifications.push(Notification {
        title: counts.join(", "),
        message: list,
        sent: vec![entry(SENT)],
    });
}

fn days_overdue(state: DueState) -> i64 {
    match state {
        DueState::Overdue(days) => days,
        _ => 0,
    }
}

fn plan_available(
    tasks: &[ReminderTask],
    outcomes: &HashMap<(&str, &str, &str), &str>,
    now: DateTime<Utc>,
    plan: &mut Plan,
) {
    let now_seconds = now.timestamp();
    let mut sendable = Vec::new();
    for item in tasks {
        let task = &item.task;
        if !available_since_defer(task, now_seconds) {
            continue;
        }
        let Some(trigger) = task.available_at.as_deref() else {
            continue;
        };
        let entry = |outcome: &str| ReminderLogEntry {
            kind: AVAILABLE.into(),
            task_id: task.id.to_string(),
            trigger: trigger.to_string(),
            outcome: outcome.into(),
        };
        // A task seen blocked while still fresh waits past the catch-up
        // window for its blocker to finish.
        match outcomes.get(&(AVAILABLE, task.id.as_str(), trigger)) {
            Some(&WAITING) if !item.blocked => sendable.push((item, entry(SENT))),
            Some(_) => {}
            None => {
                let fresh = unix_seconds(trigger).is_some_and(|available_at| {
                    now_seconds - available_at <= CATCH_UP_WINDOW_SECONDS
                });
                if !fresh {
                    plan.record.push(entry(SUPPRESSED));
                } else if item.blocked {
                    plan.record.push(entry(WAITING));
                } else {
                    sendable.push((item, entry(SENT)));
                }
            }
        }
    }
    sendable.sort_by(|(left, _), (right, _)| {
        left.task
            .available_at
            .cmp(&right.task.available_at)
            .then_with(|| left.task.title.cmp(&right.task.title))
    });
    if sendable.len() <= MAX_INDIVIDUAL_AVAILABLE {
        plan.notifications
            .extend(sendable.into_iter().map(|(item, entry)| Notification {
                title: format!("Available {}", item.display_ref),
                message: item.task.title.clone(),
                sent: vec![entry],
            }));
    } else {
        let list = preview(sendable.iter().map(|(item, _)| *item));
        plan.notifications.push(Notification {
            title: format!("{} tasks available", sendable.len()),
            message: list,
            sent: sendable.into_iter().map(|(_, entry)| entry).collect(),
        });
    }
}

/// One `REF title` line per task, at most [`PREVIEW_TITLES`] of them.
fn preview<'a>(tasks: impl ExactSizeIterator<Item = &'a ReminderTask>) -> String {
    let more = tasks.len().saturating_sub(PREVIEW_TITLES);
    let mut lines: Vec<String> = tasks
        .take(PREVIEW_TITLES)
        .map(|item| one_line(&format!("{} {}", item.display_ref, item.task.title)))
        .collect();
    if more > 0 {
        lines.push(format!("and {more} more"));
    }
    lines.join("\n")
}

/// Shortens `text` to [`PREVIEW_LINE_CHARS`], at a word boundary when one is
/// near, so each task takes one line.
fn one_line(text: &str) -> String {
    if text.chars().count() <= PREVIEW_LINE_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(PREVIEW_LINE_CHARS - 1).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space >= PREVIEW_LINE_CHARS / 2 => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end())
}

/// Time until the next digest time or deferred task, capped at [`MAX_SLEEP`].
pub(crate) fn next_wake(
    tasks: &[ReminderTask],
    now: DateTime<Utc>,
    local: NaiveDateTime,
    config: &RemindersConfig,
    digest_time: NaiveTime,
) -> Duration {
    let mut wait = MAX_SLEEP;
    if config.due_digest {
        let today_digest = local.date().and_time(digest_time);
        let next_digest = if local < today_digest {
            today_digest
        } else {
            today_digest + chrono::Days::new(1)
        };
        if let Ok(until) = (next_digest - local).to_std() {
            wait = wait.min(until);
        }
    }
    if config.available {
        let now_seconds = now.timestamp();
        let next_available = tasks
            .iter()
            .filter_map(|item| unix_seconds(item.task.available_at.as_deref()?))
            .filter(|available_at| *available_at > now_seconds)
            .min();
        if let Some(available_at) = next_available {
            wait = wait.min(Duration::from_secs((available_at - now_seconds) as u64));
        }
    }
    wait.max(Duration::from_secs(1))
}

/// Sends the reminders due now and returns how long to wait before the next
/// check. Failures are logged and retried at a later check.
pub(crate) async fn remind(database: &Database, config: &RemindersConfig) -> Duration {
    match remind_once(database, config).await {
        Ok(wait) => wait,
        Err(error) => {
            warn!(error = %format!("{error:#}"), "reminder check failed");
            MAX_SLEEP
        }
    }
}

async fn remind_once(database: &Database, config: &RemindersConfig) -> Result<Duration> {
    let digest_time = config.digest_time()?;
    let tasks = load_tasks(database).await?;
    let now = Utc::now();
    let local = now.with_timezone(&Local).naive_local();
    let log = database.reminder_log().await?;
    let plan = plan(&tasks, &log, now, local, config, digest_time);
    database.record_reminders(&plan.record).await?;
    for notification in plan.notifications {
        let (title, message) = (notification.title.clone(), notification.message.clone());
        let sent = tokio::task::spawn_blocking(move || {
            crate::notification::send_notification(&title, &message)
        })
        .await?;
        match sent {
            Ok(()) => {
                println!(
                    "daemon-reminded title={:?} message={:?}",
                    notification.title, notification.message
                );
                database.record_reminders(&notification.sent).await?;
            }
            Err(error) => warn!(error = %format!("{error:#}"), "reminder notification failed"),
        }
    }
    Ok(next_wake(&tasks, now, local, config, digest_time))
}

/// Open tasks across every workspace in the database.
async fn load_tasks(database: &Database) -> Result<Vec<ReminderTask>> {
    let mut tasks = Vec::new();
    for workspace in database.list_workspaces().await? {
        let items = database
            .list_task_summary_items(
                &workspace.id,
                TaskFilters {
                    hide_done: true,
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Created,
                SortDirection::Asc,
                None,
            )
            .await?;
        tasks.extend(items.into_iter().map(|item| ReminderTask {
            blocked: item.unresolved_blocker_count > 0,
            display_ref: item.display_ref,
            task: item.task,
        }));
    }
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use aven_core::choices::{TaskPriority, TaskStatus};

    use super::*;

    const DIGEST_AT: &str = "09:00";

    fn config() -> RemindersConfig {
        RemindersConfig {
            enabled: true,
            ..RemindersConfig::default()
        }
    }

    fn task(name: &str) -> ReminderTask {
        ReminderTask {
            task: Task {
                id: aven_core::test_support::task_id(name),
                workspace_id: "0000000000000001".parse().unwrap(),
                title: name.to_string(),
                description: String::new(),
                project_id: "0000000000000001".parse().unwrap(),
                project_key: "app".to_string(),
                project_prefix: "APP".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                source: aven_core::choices::TaskSource::Unknown,
                created_at: "2026-10-01T08:00:00Z".to_string(),
                updated_at: "2026-10-02T08:00:00Z".to_string(),
                queue_activity_at: "2026-10-02T08:00:00Z".to_string(),
                available_at: None,
                due_on: None,
                deleted: false,
                is_epic: false,
            },
            display_ref: format!("APP-{name}"),
            blocked: false,
        }
    }

    fn due(name: &str, due_on: &str) -> ReminderTask {
        let mut item = task(name);
        item.task.due_on = Some(due_on.to_string());
        item
    }

    fn deferred(name: &str, available_at: &str) -> ReminderTask {
        let mut item = task(name);
        item.task.available_at = Some(available_at.to_string());
        item
    }

    fn at(utc: &str) -> (DateTime<Utc>, NaiveDateTime) {
        let now = utc.parse::<DateTime<Utc>>().unwrap();
        // Tests use UTC as the local time zone.
        (now, now.naive_utc())
    }

    fn run(tasks: &[ReminderTask], log: &[ReminderLogEntry], utc: &str) -> Plan {
        let (now, local) = at(utc);
        plan(
            tasks,
            log,
            now,
            local,
            &config(),
            NaiveTime::parse_from_str(DIGEST_AT, "%H:%M").unwrap(),
        )
    }

    /// Records what the daemon would after every send succeeds.
    fn apply(log: &mut Vec<ReminderLogEntry>, plan: Plan) {
        let entries = plan
            .record
            .into_iter()
            .chain(plan.notifications.into_iter().flat_map(|n| n.sent));
        for entry in entries {
            log.retain(|existing| {
                (&existing.kind, &existing.task_id, &existing.trigger)
                    != (&entry.kind, &entry.task_id, &entry.trigger)
            });
            log.push(entry);
        }
    }

    fn messages(plan: &Plan) -> Vec<String> {
        plan.notifications
            .iter()
            .map(|n| format!("{}\n{}", n.title, n.message))
            .collect()
    }

    #[test]
    fn preview_lines_fit_one_banner_line() {
        assert_eq!(one_line("APP-7KQ9 Pay invoice"), "APP-7KQ9 Pay invoice");
        assert_eq!(
            one_line("KTJ-1FNM Fix homepage target-language switching"),
            "KTJ-1FNM Fix homepage…"
        );
        assert_eq!(
            one_line("APP-7KQ9 Ääkkösiäsisältävätehtävänimiolematonsana"),
            "APP-7KQ9 Ääkkösiäsisältävätehtäväni…"
        );
    }

    #[test]
    fn digest_sends_once_per_local_date_from_digest_time() {
        let tasks = [
            due("Ship", "2026-10-14"),
            due("Pay", "2026-10-12"),
            due("Later", "2026-10-20"),
        ];
        let mut log = Vec::new();

        assert!(messages(&run(&tasks, &log, "2026-10-14T08:59:00Z")).is_empty());

        let plan = run(&tasks, &log, "2026-10-14T13:00:00Z");
        assert_eq!(
            messages(&plan),
            ["1 due today, 1 overdue\nAPP-Pay Pay\nAPP-Ship Ship"]
        );
        apply(&mut log, plan);
        assert!(messages(&run(&tasks, &log, "2026-10-14T18:00:00Z")).is_empty());

        // A missed day is not sent late; the next day has its own digest.
        let plan = run(&tasks, &log, "2026-10-16T09:00:00Z");
        assert_eq!(messages(&plan), ["2 overdue\nAPP-Pay Pay\nAPP-Ship Ship"]);
    }

    #[test]
    fn digest_includes_blocked_and_deferred_tasks() {
        let mut blocked = due("Blocked", "2026-10-14");
        blocked.blocked = true;
        let mut deferred = due("Deferred", "2026-10-14");
        deferred.task.available_at = Some("2026-10-20T09:00:00Z".to_string());

        let plan = run(&[blocked, deferred], &[], "2026-10-14T09:00:00Z");

        assert_eq!(
            messages(&plan),
            ["2 due today\nAPP-Blocked Blocked\nAPP-Deferred Deferred"]
        );
    }

    #[test]
    fn empty_digest_consumes_the_day() {
        let mut log = Vec::new();
        let plan = run(&[], &log, "2026-10-14T09:00:00Z");
        assert!(plan.notifications.is_empty());
        assert_eq!(plan.record[0].outcome, EMPTY);
        apply(&mut log, plan);

        let tasks = [due("Ship", "2026-10-14")];
        assert!(messages(&run(&tasks, &log, "2026-10-14T15:00:00Z")).is_empty());
    }

    #[test]
    fn resurfacing_notifies_once_per_deferral() {
        let mut tasks = [deferred("Call", "2026-10-14T10:00:00Z")];
        let mut log = Vec::new();

        assert!(messages(&run(&tasks, &log, "2026-10-14T09:59:00Z")).is_empty());
        let plan = run(&tasks, &log, "2026-10-14T10:00:30Z");
        assert_eq!(messages(&plan), ["Available APP-Call\nCall"]);
        apply(&mut log, plan);
        assert!(messages(&run(&tasks, &log, "2026-10-14T10:05:00Z")).is_empty());

        tasks[0].task.available_at = Some("2026-10-14T11:00:00Z".to_string());
        let plan = run(&tasks, &log, "2026-10-14T11:00:00Z");
        assert_eq!(messages(&plan), ["Available APP-Call\nCall"]);
    }

    #[test]
    fn blocked_task_waits_for_its_blocker_past_the_catch_up_window() {
        let mut tasks = [deferred("Deploy", "2026-10-14T10:00:00Z")];
        tasks[0].blocked = true;
        let mut log = Vec::new();

        let plan = run(&tasks, &log, "2026-10-14T10:01:00Z");
        assert!(plan.notifications.is_empty());
        assert!(plan.record.iter().any(|entry| entry.outcome == WAITING));
        apply(&mut log, plan);

        tasks[0].blocked = false;
        let plan = run(&tasks, &log, "2026-10-17T12:00:00Z");
        assert_eq!(messages(&plan), ["Available APP-Deploy\nDeploy"]);
        apply(&mut log, plan);
        assert!(messages(&run(&tasks, &log, "2026-10-17T12:05:00Z")).is_empty());
    }

    #[test]
    fn stale_resurfacing_is_suppressed() {
        let tasks = [deferred("Old", "2026-10-13T08:00:00Z")];

        let plan = run(&tasks, &[], "2026-10-14T08:30:00Z");

        assert!(plan.notifications.is_empty());
        assert_eq!(plan.record.len(), 1);
        assert_eq!(plan.record[0].outcome, SUPPRESSED);
    }

    #[test]
    fn more_than_three_resurfacing_tasks_are_summarized() {
        let tasks: Vec<_> = ["A", "B", "C", "D"]
            .into_iter()
            .map(|name| deferred(name, "2026-10-14T10:00:00Z"))
            .collect();

        let plan = run(&tasks, &[], "2026-10-14T10:00:00Z");
        assert_eq!(
            messages(&plan),
            ["4 tasks available\nAPP-A A\nAPP-B B\nAPP-C C\nand 1 more"]
        );
        assert_eq!(plan.notifications[0].sent.len(), 4);

        let plan = run(&tasks[..3], &[], "2026-10-14T10:00:00Z");
        assert_eq!(plan.notifications.len(), 3);
    }

    #[test]
    fn next_wake_targets_the_earliest_event_within_the_cap() {
        let digest_time = NaiveTime::parse_from_str(DIGEST_AT, "%H:%M").unwrap();
        let (now, local) = at("2026-10-14T08:50:00Z");
        assert_eq!(
            next_wake(&[], now, local, &config(), digest_time),
            Duration::from_secs(600)
        );

        let tasks = [deferred("Soon", "2026-10-14T08:52:00Z")];
        assert_eq!(
            next_wake(&tasks, now, local, &config(), digest_time),
            Duration::from_secs(120)
        );

        let (now, local) = at("2026-10-14T12:00:00Z");
        assert_eq!(
            next_wake(&[], now, local, &config(), digest_time),
            MAX_SLEEP
        );
    }
}
