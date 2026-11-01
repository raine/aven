//! The prototype's sample data, loaded into a throwaway database.
//!
//! Core reads wall time directly, so scenes use relative fixtures rather than an
//! injected clock. `fixture.json` holds the prototype's data as generated at a
//! fixed instant. A run shifts it to an anchor just before now, seeds core, and
//! freezes the prototype at that anchor. Both sides then classify the same data
//! at the same moment, whatever the time of day.
use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use aven_core::choices::TaskSource;
use aven_core::db::Database;
use aven_core::ids::TaskId;
use aven_core::metadata::TaskMetadataInput;
use aven_core::operations::TaskDraft;
use aven_core::workspaces::Workspace;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MINUTE_FORMAT: &str = "%Y-%m-%dT%H:%M";
const DAY_FORMAT: &str = "%Y-%m-%d";
const PRIORITIES: [&str; 5] = ["none", "low", "medium", "high", "urgent"];
/// Scene runs refuse this close to local midnight, where the seconds between
/// the anchor and core's queries could cross a date boundary.
const MIDNIGHT_MARGIN_MINUTES: u32 = 10;

#[derive(Debug, Serialize, Deserialize)]
pub struct Fixture {
    /// The prototype's clock when `data` was generated, as local wall time.
    pub frozen_at: String,
    pub data: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureTask {
    id: String,
    project: String,
    title: String,
    status: String,
    pri: usize,
    due: Option<String>,
    available_at: Option<String>,
    labels: Vec<String>,
    body: String,
    #[serde(default)]
    notes: Vec<Value>,
    blocked_by: Vec<String>,
    related: Vec<String>,
    epic: Option<String>,
    is_epic: bool,
    created_at: String,
    updated_at: String,
    source: Option<String>,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    meta: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct FixtureData {
    tasks: Vec<FixtureTask>,
    workspaces: BTreeMap<String, String>,
    projects: BTreeMap<String, (String, String, String)>,
}

/// The fixture as seeded, with what the prototype needs to show the same data.
pub struct Seeded {
    pub workspace: Workspace,
    /// Fixture task id to core task id.
    pub ids: BTreeMap<String, TaskId>,
    pub prototype_data: Value,
}

/// The instant a scene run treats as now, whole-minute like the fixture.
pub fn anchor_now() -> Result<DateTime<Local>> {
    let now = Local::now();
    let minutes = now.hour() * 60 + now.minute();
    if !(MIDNIGHT_MARGIN_MINUTES..24 * 60 - MIDNIGHT_MARGIN_MINUTES).contains(&minutes) {
        bail!("scene runs refuse the {MIDNIGHT_MARGIN_MINUTES} minutes around local midnight");
    }
    now.with_second(0)
        .and_then(|time| time.with_nanosecond(0))
        .context("truncate anchor")
}

impl Fixture {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    /// The fixture's data as it reads at `anchor`.
    pub fn shifted(&self, anchor: DateTime<Local>) -> Result<Value> {
        let frozen = NaiveDateTime::parse_from_str(&self.frozen_at, MINUTE_FORMAT)?;
        let anchor = anchor.naive_local();
        let mut data = self.data.clone();
        shift_strings(
            &mut data,
            anchor - frozen,
            (anchor.date() - frozen.date()).num_days(),
        );
        Ok(data)
    }
}

/// Timestamps move by the exact offset, so their distance from now is preserved.
/// Dates move by whole days, so their distance from today is preserved.
fn shift_strings(value: &mut Value, offset: TimeDelta, days: i64) {
    match value {
        Value::String(text) => {
            if let Ok(time) = NaiveDateTime::parse_from_str(text, MINUTE_FORMAT) {
                *text = (time + offset).format(MINUTE_FORMAT).to_string();
            } else if text.len() == 10
                && let Ok(date) = NaiveDate::parse_from_str(text, DAY_FORMAT)
            {
                *text = (date + TimeDelta::days(days))
                    .format(DAY_FORMAT)
                    .to_string();
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| shift_strings(item, offset, days)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|item| shift_strings(item, offset, days)),
        _ => {}
    }
}

/// Replaces whole-string fixture ids, leaving prose alone.
fn rename_ids(value: &mut Value, names: &BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(name) = names.get(text.as_str()) {
                text.clone_from(name);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| rename_ids(item, names)),
        Value::Object(fields) => fields.values_mut().for_each(|item| rename_ids(item, names)),
        _ => {}
    }
}

fn utc(local: &str) -> Result<String> {
    let time = NaiveDateTime::parse_from_str(local, MINUTE_FORMAT)
        .with_context(|| format!("parse {local}"))?;
    let time = Local
        .from_local_datetime(&time)
        .earliest()
        .with_context(|| format!("{local} has no local time"))?;
    Ok(time.to_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

fn source(name: Option<&str>) -> TaskSource {
    match name {
        Some("cli") => TaskSource::Cli,
        Some("tui") => TaskSource::Tui,
        Some("ios") => TaskSource::Ios,
        Some("android") => TaskSource::Android,
        _ => TaskSource::Unknown,
    }
}

/// Loads shifted fixture data through core. Only timestamps go through SQL,
/// because core stamps them on write and scenes need them in the past.
///
/// Conflicts and recurrence series are not seeded yet, so the prototype's copy
/// drops them too and both sides keep the same state.
pub async fn seed(database: &Database, shifted: &Value) -> Result<Seeded> {
    let data: FixtureData = serde_json::from_value(shifted.clone()).context("read fixture")?;
    let mut workspaces = BTreeMap::new();
    let existing = database.list_workspaces().await?;
    for (key, name) in &data.workspaces {
        let found = existing.iter().find(|workspace| workspace.name == *name);
        let workspace = match found {
            Some(workspace) => workspace.clone(),
            None => database.create_workspace(name).await?,
        };
        workspaces.insert(key.clone(), workspace);
    }
    for (prefix, (name, workspace, _)) in &data.projects {
        database
            .create_project(&workspaces[workspace], name, Some(prefix))
            .await
            .with_context(|| format!("create project {prefix}"))?;
    }

    let workspace_of = |task: &FixtureTask| &workspaces[&data.projects[&task.project].1];
    let mut ids = BTreeMap::new();
    for task in &data.tasks {
        let workspace = workspace_of(task);
        for label in &task.labels {
            database.create_label(workspace, label).await.ok();
        }
        let outcome = database
            .create_task(
                workspace,
                TaskDraft {
                    title: task.title.clone(),
                    description: task.body.clone(),
                    project: Some(data.projects[&task.project].0.clone()),
                    status: Some(task.status.clone()),
                    priority: PRIORITIES[task.pri].to_string(),
                    source: source(task.source.as_deref()),
                    labels: task.labels.clone(),
                    metadata: task
                        .meta
                        .iter()
                        .map(|(key, value)| TaskMetadataInput {
                            expected_field_id: None,
                            key: key.clone(),
                            value: value.clone(),
                        })
                        .collect(),
                    available_at: task.available_at.as_deref().map(utc).transpose()?,
                    due_on: task.due.clone(),
                    is_epic: task.is_epic,
                },
            )
            .await
            .with_context(|| format!("create {}", task.id))?;
        ids.insert(task.id.clone(), outcome.task.id);
    }

    for task in &data.tasks {
        let workspace = workspace_of(task);
        let id = &ids[&task.id];
        if let Some(epic) = &task.epic {
            database
                .add_task_to_epic(workspace, id, &ids[epic])
                .await
                .with_context(|| format!("add {} to {epic}", task.id))?;
        }
        for blocker in &task.blocked_by {
            database
                .add_task_dependency(workspace, id, &ids[blocker])
                .await
                .with_context(|| format!("block {} on {blocker}", task.id))?;
        }
        for related in &task.related {
            if task.id < *related {
                database
                    .add_task_related_link(workspace, id, &ids[related])
                    .await?;
            }
        }
        for note in &task.notes {
            if let Some(body) = note
                .get("text")
                .or(note.get("body"))
                .and_then(Value::as_str)
            {
                database.add_note(workspace, id, body.to_string()).await?;
            }
        }
        if task.deleted {
            database.set_task_deleted(workspace, id, true).await?;
        }
    }

    let pool = aven_core::test_support::pool(database);
    for task in &data.tasks {
        sqlx::query(
            "UPDATE tasks SET created_at = ?, updated_at = ?, queue_activity_at = ? WHERE id = ?",
        )
        .bind(utc(&task.created_at)?)
        .bind(utc(&task.updated_at)?)
        .bind(utc(&task.updated_at)?)
        .bind(&ids[&task.id])
        .execute(&pool)
        .await?;
    }

    let mut suffixes = BTreeMap::new();
    for (key, workspace) in &workspaces {
        let in_workspace: Vec<_> = data
            .tasks
            .iter()
            .filter(|task| data.projects[&task.project].1 == *key)
            .map(|task| ids[&task.id].clone())
            .collect();
        let refs = database
            .display_refs_for_task_ids(&workspace.id, &in_workspace)
            .await?;
        for task in &data.tasks {
            if let Some(display) = refs.get(&ids[&task.id]) {
                let suffix = display.rsplit('-').next().unwrap_or(display);
                suffixes.insert(task.id.clone(), suffix.to_string());
            }
        }
    }
    let mut prototype_data = shifted.clone();
    rename_ids(&mut prototype_data, &suffixes);
    prototype_data["series"] = Value::Array(Vec::new());
    for task in prototype_data["tasks"].as_array_mut().into_iter().flatten() {
        task["conflicts"] = Value::Array(Vec::new());
        if let Some(fields) = task.as_object_mut() {
            fields.remove("rcr");
        }
    }

    let workspace = workspaces
        .get("personal")
        .context("fixture has no personal workspace")?
        .clone();
    Ok(Seeded {
        workspace,
        ids,
        prototype_data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Fixture {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes/fixture.json");
        Fixture::load(&path).unwrap()
    }

    fn anchor(text: &str) -> DateTime<Local> {
        let naive = NaiveDateTime::parse_from_str(text, MINUTE_FORMAT).unwrap();
        Local.from_local_datetime(&naive).earliest().unwrap()
    }

    fn timestamps(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::String(text) => found.push(text.clone()),
            Value::Array(items) => items.iter().for_each(|item| timestamps(item, found)),
            Value::Object(map) => map.values().for_each(|item| timestamps(item, found)),
            _ => {}
        }
    }

    #[test]
    fn shifting_moves_timestamps_by_minutes_and_dates_by_days() {
        let fixture = fixture();
        let frozen = NaiveDateTime::parse_from_str(&fixture.frozen_at, MINUTE_FORMAT).unwrap();
        let shifted = fixture.shifted(anchor("2027-03-10T15:37")).unwrap();
        let tasks = |data: &Value| data["tasks"].as_array().unwrap().clone();
        let offset =
            NaiveDateTime::parse_from_str("2027-03-10T15:37", MINUTE_FORMAT).unwrap() - frozen;
        let days = NaiveDate::from_ymd_opt(2027, 3, 10).unwrap() - frozen.date();

        for (before, after) in tasks(&fixture.data).iter().zip(tasks(&shifted)) {
            let before_at = before["createdAt"].as_str().unwrap();
            let after_at = after["createdAt"].as_str().unwrap();
            let moved = NaiveDateTime::parse_from_str(before_at, MINUTE_FORMAT).unwrap() + offset;
            assert_eq!(moved.format(MINUTE_FORMAT).to_string(), after_at);
            if let Some(due) = before["due"].as_str() {
                let moved = NaiveDate::parse_from_str(due, DAY_FORMAT).unwrap() + days;
                assert_eq!(after["due"], moved.format(DAY_FORMAT).to_string());
            }
        }
    }

    /// Core's queries run seconds after the anchor, so a fixture timestamp this
    /// close to it could classify differently on the two sides.
    #[test]
    fn nothing_in_the_fixture_is_within_minutes_of_the_frozen_clock() {
        let fixture = fixture();
        let frozen = NaiveDateTime::parse_from_str(&fixture.frozen_at, MINUTE_FORMAT).unwrap();
        let mut found = Vec::new();
        timestamps(&fixture.data, &mut found);
        let near: Vec<_> = found
            .iter()
            .filter(|text| {
                NaiveDateTime::parse_from_str(text, MINUTE_FORMAT)
                    .is_ok_and(|at| (at - frozen).num_minutes().abs() < 5)
            })
            .collect();
        assert_eq!(near, Vec::<&String>::new());
    }

    #[test]
    fn renaming_replaces_whole_ids_only() {
        let names = BTreeMap::from([("t1".to_string(), "ABCD".to_string())]);
        let mut value =
            serde_json::json!({ "id": "t1", "blockedBy": ["t1", "t10"], "title": "t1 x" });
        rename_ids(&mut value, &names);
        assert_eq!(
            value,
            serde_json::json!({ "id": "ABCD", "blockedBy": ["ABCD", "t10"], "title": "t1 x" })
        );
    }
}
