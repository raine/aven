use super::*;
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceOutcome, RecurrenceProposalIds,
    RecurrenceRule, RecurrenceSchedule, RecurrenceSeriesId, RecurrenceSeriesState, TimeZoneId,
    WeekdaySet, derive_occurrence_identity, is_slot, next_slot_after,
};
use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, NaiveTime};
use serde_json::Value;

pub(super) fn validate_recurrence_task(change: &ChangeWire) -> Result<()> {
    let workspace_id: WorkspaceId = required_string_payload("workspace_id", &change.payload)?
        .parse()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change workspace_id invalid-id"))?;
    let series_id: RecurrenceSeriesId = required_string_payload("series_id", &change.payload)?
        .parse()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change series_id invalid-id"))?;
    let slot_on = recurrence_date_payload("slot_on", &change.payload)?;
    let schedule = recurrence_schedule_payload(&change.payload)?;
    if !is_slot(&schedule.rule, schedule.start_on, slot_on) {
        bail!("error invalid-sync-change recurrence-slot-off-lattice");
    }
    let identity = derive_occurrence_identity(&workspace_id, &series_id, &schedule, slot_on)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let slot = crate::recurrence::slot_values(&schedule, slot_on)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    for (key, expected) in [
        ("task_id", identity.task_id.as_str()),
        ("created_at", identity.created_at.as_str()),
        ("updated_at", identity.updated_at.as_str()),
        ("available_at", slot.available_at.as_str()),
        ("due_on", slot.due_on.as_deref().unwrap_or("")),
        (
            "occurrence_field_version_seed",
            identity.field_version_seeds.occurrence.as_str(),
        ),
    ] {
        if required_string_payload(key, &change.payload)? != expected {
            bail!("error invalid-sync-change recurrence-deterministic-mismatch field={key}");
        }
    }
    let ids = identity.generated_task_ids(&change.payload)?;
    require_generation_ids(&change.payload, &ids)?;
    if change.entity_id != identity.task_id.as_str()
        || change.change_id != ids.task_change_id
        || change.created_at != identity.created_at
    {
        bail!("error invalid-sync-change recurrence-deterministic-mismatch field=change_identity");
    }
    Ok(())
}

/// A generated record's change IDs and seed must all belong to one derivation form.
fn require_generation_ids(payload: &Value, ids: &RecurrenceProposalIds) -> Result<()> {
    for (key, expected) in [
        ("task_change_id", ids.task_change_id.as_str()),
        ("occurrence_change_id", ids.occurrence_change_id.as_str()),
        (
            "task_field_version_seed",
            ids.task_field_version_seed.as_str(),
        ),
    ] {
        if required_string_payload(key, payload)? != expected {
            bail!("error invalid-sync-change recurrence-deterministic-mismatch field={key}");
        }
    }
    Ok(())
}

pub(super) fn validate_recurrence_create(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "recurrence_series")?;
    let series_id: RecurrenceSeriesId = change
        .entity_id
        .parse()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change entity_id invalid-id"))?;
    required_workspace_payload(&change.payload)?;
    if required_string_payload("series_id", &change.payload)? != series_id.as_str() {
        bail!("error invalid-sync-change recurrence-series-id-mismatch");
    }
    required_string_payload("title", &change.payload)?;
    required_string_payload("description", &change.payload)?;
    let project_id = required_string_payload("project_id", &change.payload)?;
    ensure_project_id("project_id", &project_id)?;
    required_string_payload("project_key", &change.payload)?;
    required_string_payload("project_name", &change.payload)?;
    required_string_payload("project_prefix", &change.payload)?;
    validate_sync_task_field_value(
        TaskField::Priority,
        &required_string_payload("priority", &change.payload)?,
    )?;
    let status = required_string_payload("initial_status", &change.payload)?;
    validate_sync_task_field_value(TaskField::Status, &status)?;
    if matches!(status.as_str(), "done" | "canceled") {
        bail!("error invalid-sync-change recurrence-terminal-template");
    }
    recurrence_schedule_payload(&change.payload)?;
    optional_string_array_payload("labels", &change.payload)?;
    validate_metadata_array_payload(&change.payload)?;
    if required_string_payload("state", &change.payload)? != "active"
        || !required_string_payload("stopped_at", &change.payload)?.is_empty()
    {
        bail!("error invalid-sync-change recurrence-create-state");
    }
    required_timestamp_payload("created_at", &change.payload)?;
    required_timestamp_payload("updated_at", &change.payload)?;
    Ok(())
}

pub(super) fn validate_recurrence_template(change: &ChangeWire) -> Result<()> {
    validate_recurrence_entity(change)?;
    let fields = change
        .payload
        .get("fields")
        .and_then(Value::as_array)
        .context("error invalid-sync-change payload.fields missing")?;
    if fields.len() > 8 {
        bail!("error invalid-sync-change recurrence-template-fields-too-large");
    }
    let base_versions = change
        .payload
        .get("base_versions")
        .and_then(Value::as_object)
        .context("error invalid-sync-change payload.base_versions missing")?;
    let mut seen = HashSet::new();
    for pair in fields {
        let pair = pair
            .as_array()
            .filter(|pair| pair.len() == 2)
            .context("error invalid-sync-change recurrence-template-field-pair")?;
        let field = pair[0]
            .as_str()
            .context("error invalid-sync-change recurrence-template-field")?;
        let value = pair[1]
            .as_str()
            .context("error invalid-sync-change recurrence-template-value")?;
        if !seen.insert(field) {
            bail!("error invalid-sync-change recurrence-template-duplicate-field");
        }
        if !matches!(
            base_versions.get(field),
            Some(Value::String(_)) | Some(Value::Null)
        ) {
            bail!("error invalid-sync-change recurrence-template-base-version field={field}");
        }
        match field {
            "title" | "description" => {}
            "project" => ensure_project_id("project_id", value)?,
            "priority" => validate_sync_task_field_value(TaskField::Priority, value)?,
            "initial_status" => {
                validate_sync_task_field_value(TaskField::Status, value)?;
                if matches!(value, "done" | "canceled") {
                    bail!("error invalid-sync-change recurrence-terminal-template");
                }
            }
            "available_local_time" => {
                validate_local_time(value)?;
            }
            "due_policy" => {
                RecurrenceDuePolicy::parse(value)
                    .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
            }
            _ => bail!("error invalid-sync-change recurrence-template-field={field}"),
        }
    }
    optional_string_array_payload("labels", &change.payload)?;
    let labels_changed = change
        .payload
        .get("labels_changed")
        .and_then(Value::as_bool)
        .context("error invalid-sync-change payload.labels_changed missing")?;
    if labels_changed
        && !matches!(
            base_versions.get("labels"),
            Some(Value::String(_)) | Some(Value::Null)
        )
    {
        bail!("error invalid-sync-change recurrence-template-base-version field=labels");
    }
    required_timestamp_payload("updated_at", &change.payload)?;
    Ok(())
}

pub(super) fn validate_recurrence_projection(change: &ChangeWire) -> Result<()> {
    validate_recurrence_entity(change)?;
    if change.field.as_deref() != Some("projection") {
        bail!("error invalid-sync-change field=projection");
    }
    let workspace_id: WorkspaceId = required_string_payload("workspace_id", &change.payload)?
        .parse()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change workspace_id invalid-id"))?;
    let series_id: RecurrenceSeriesId = change.entity_id.parse().unwrap();
    let slot_on = recurrence_date_payload("slot_on", &change.payload)?;
    let schedule = recurrence_schedule_payload(&change.payload)?;
    if !is_slot(&schedule.rule, schedule.start_on, slot_on) {
        bail!("error invalid-sync-change recurrence-slot-off-lattice");
    }
    let identity = derive_occurrence_identity(&workspace_id, &series_id, &schedule, slot_on)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    for (key, expected) in [
        ("task_id", identity.task_id.as_str()),
        (
            "projected_at",
            identity.occurrence_link.projected_at.as_str(),
        ),
        (
            "occurrence_field_version_seed",
            identity.field_version_seeds.occurrence.as_str(),
        ),
    ] {
        if required_string_payload(key, &change.payload)? != expected {
            bail!("error invalid-sync-change recurrence-deterministic-mismatch field={key}");
        }
    }
    let ids = identity.projection_ids(&required_string_payload("task_change_id", &change.payload)?);
    require_generation_ids(&change.payload, &ids)?;
    if change.change_id != ids.occurrence_change_id
        || change.created_at != identity.occurrence_link.projected_at
    {
        bail!("error invalid-sync-change recurrence-deterministic-mismatch field=change_id");
    }
    Ok(())
}

pub(super) fn validate_recurrence_outcome(change: &ChangeWire) -> Result<()> {
    validate_recurrence_entity(change)?;
    validate_timestamp_value("created_at", &change.created_at)?;
    if change.field.as_deref() != Some("outcome") {
        bail!("error invalid-sync-change field=outcome");
    }
    let slot_on = recurrence_date_payload("slot_on", &change.payload)?;
    let schedule = recurrence_schedule_payload(&change.payload)?;
    if !is_slot(&schedule.rule, schedule.start_on, slot_on) {
        bail!("error invalid-sync-change recurrence-slot-off-lattice");
    }
    let outcome = RecurrenceOutcome::parse(&required_string_payload("outcome", &change.payload)?)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    required_timestamp_payload("resolved_at", &change.payload)?;
    let conflict_resolution = change
        .payload
        .get("conflict_resolution")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let task_id = required_string_payload("task_id", &change.payload)?;
    ensure_sync_id("task_id", &task_id)?;
    let workspace_id: WorkspaceId =
        required_string_payload("workspace_id", &change.payload)?.parse()?;
    let series_id: RecurrenceSeriesId = change.entity_id.parse().unwrap();
    let identity = derive_occurrence_identity(&workspace_id, &series_id, &schedule, slot_on)?;
    if task_id != identity.task_id.as_str() {
        bail!("error invalid-sync-change recurrence-deterministic-mismatch field=task_id");
    }
    let expected_status = match outcome {
        RecurrenceOutcome::Completed => "done",
        RecurrenceOutcome::Skipped => "canceled",
    };
    if required_string_payload("task_status", &change.payload)? != expected_status {
        bail!("error invalid-sync-change recurrence-outcome-status-mismatch");
    }
    let status_change_id = required_string_payload("task_status_change_id", &change.payload)?;
    if !conflict_resolution || !status_change_id.is_empty() {
        ensure_sync_id("task_status_change_id", &status_change_id)?;
    }
    let successor = required_string_payload("successor_task_id", &change.payload)?;
    if !successor.is_empty() {
        ensure_sync_id("successor_task_id", &successor)?;
        let successor_slot = next_slot_after(&schedule.rule, schedule.start_on, slot_on)
            .context("error invalid-sync-change recurrence-successor-out-of-range")?;
        let successor_identity =
            derive_occurrence_identity(&workspace_id, &series_id, &schedule, successor_slot)?;
        if successor != successor_identity.task_id.as_str() {
            bail!(
                "error invalid-sync-change recurrence-deterministic-mismatch field=successor_task_id"
            );
        }
    }
    Ok(())
}

pub(super) fn validate_recurrence_state(change: &ChangeWire) -> Result<()> {
    validate_recurrence_entity(change)?;
    if change.field.as_deref() != Some("state") {
        bail!("error invalid-sync-change field=state");
    }
    let conflict_resolution = change
        .payload
        .get("conflict_resolution")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if change.base_version.is_none() && !conflict_resolution {
        bail!("error invalid-sync-change recurrence-lifecycle-base-version-missing");
    }
    let state = RecurrenceSeriesState::parse(&required_string_payload("state", &change.payload)?)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let stopped_at = required_string_payload("stopped_at", &change.payload)?;
    if matches!(state, RecurrenceSeriesState::Stopped) {
        required_timestamp_payload("stopped_at", &change.payload)?;
    } else if !stopped_at.is_empty() {
        bail!("error invalid-sync-change recurrence-stop-state-mismatch");
    }
    required_timestamp_payload("changed_at", &change.payload)?;
    Ok(())
}

pub(super) fn validate_recurrence_pause(change: &ChangeWire, open: bool) -> Result<()> {
    validate_recurrence_entity(change)?;
    if change.field.as_deref() != Some("pause") {
        bail!("error invalid-sync-change field=pause");
    }
    if open {
        let interval_id = required_string_payload("interval_id", &change.payload)?;
        ensure_sync_id("interval_id", &interval_id)?;
        required_timestamp_payload("paused_at", &change.payload)?;
        let slot = required_string_payload("suspended_slot_on", &change.payload)?;
        if !slot.is_empty() {
            slot.parse::<NaiveDate>()
                .context("error invalid-sync-change suspended_slot_on")?;
            let task_id = required_string_payload("suspended_task_id", &change.payload)?;
            ensure_sync_id("suspended_task_id", &task_id)?;
        }
    } else {
        required_timestamp_payload("resumed_at", &change.payload)?;
        if let Some(interval_id) = optional_string_payload("interval_id", &change.payload)? {
            ensure_sync_id("interval_id", &interval_id)?;
        }
    }
    Ok(())
}

fn validate_recurrence_entity(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "recurrence_series")?;
    change
        .entity_id
        .parse::<RecurrenceSeriesId>()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change entity_id invalid-id"))?;
    required_workspace_payload(&change.payload)?;
    Ok(())
}

fn recurrence_schedule_payload(payload: &Value) -> Result<RecurrenceSchedule> {
    let frequency = RecurrenceFrequency::parse(&required_string_payload("frequency", payload)?)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let interval = required_i64_payload("interval", payload)?;
    let interval = u32::try_from(interval)
        .context("error invalid-sync-change recurrence-interval-out-of-range")?;
    let weekdays_text = required_string_payload("weekdays", payload)?;
    let weekdays = weekdays_text
        .parse::<WeekdaySet>()
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    if weekdays.to_string() != weekdays_text {
        bail!("error invalid-sync-change recurrence-weekdays-noncanonical");
    }
    let rule = RecurrenceRule::new(frequency, interval, weekdays)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let timezone_text = required_string_payload("timezone", payload)?;
    let timezone = timezone_text
        .parse::<TimeZoneId>()
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    if timezone_text.parse::<chrono_tz::Tz>()?.to_string() != timezone_text {
        bail!("error invalid-sync-change recurrence-timezone-noncanonical");
    }
    let start_on = recurrence_date_payload("start_on", payload)?;
    let available = required_string_payload("available_local_time", payload)?;
    let available_local_time = if available.is_empty() {
        None
    } else {
        Some(validate_local_time(&available)?)
    };
    let due_policy = RecurrenceDuePolicy::parse(&required_string_payload("due_policy", payload)?)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    Ok(RecurrenceSchedule::new(
        rule,
        timezone,
        start_on,
        available_local_time,
        due_policy,
    ))
}

fn recurrence_date_payload(key: &str, payload: &Value) -> Result<NaiveDate> {
    required_string_payload(key, payload)?
        .parse()
        .with_context(|| format!("error invalid-sync-change payload.{key} invalid-date"))
}

fn validate_local_time(value: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M:%S")
        .context("error invalid-sync-change recurrence-local-time")
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{make_change_wire, test_workspace};
    use super::super::validate_pushed_change;
    use crate::change_log::{ChangePayload, op_type};
    use crate::recurrence::{
        RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule, RecurrenceSeriesId,
        derive_occurrence_identity,
    };
    use chrono::NaiveDate;

    #[test]
    fn recurrence_projection_rejects_nondeterministic_change_timestamp() {
        let workspace = test_workspace();
        let series_id: RecurrenceSeriesId = "AAAAAAAAAAAAAAAA".parse().unwrap();
        let schedule = RecurrenceSchedule::new(
            RecurrenceRule::daily(),
            "UTC".parse().unwrap(),
            "2026-07-20".parse().unwrap(),
            None,
            RecurrenceDuePolicy::SameDay,
        );
        let slot_on: NaiveDate = "2026-07-20".parse().unwrap();
        let identity =
            derive_occurrence_identity(&workspace.id, &series_id, &schedule, slot_on).unwrap();
        let payload = ChangePayload::workspace(&workspace)
            .set("series_id", series_id.as_str())
            .set("slot_on", slot_on.to_string())
            .set("task_id", identity.task_id.as_str())
            .set("projected_at", &identity.occurrence_link.projected_at)
            .set("task_change_id", &identity.task_change_id)
            .set("occurrence_change_id", &identity.occurrence_change_id)
            .set(
                "task_field_version_seed",
                &identity.field_version_seeds.task,
            )
            .set(
                "occurrence_field_version_seed",
                &identity.field_version_seeds.occurrence,
            )
            .set("frequency", "daily")
            .set("interval", 1)
            .set("weekdays", "")
            .set("timezone", "UTC")
            .set("start_on", "2026-07-20")
            .set("available_local_time", "")
            .set("due_policy", "same_day")
            .into_value();
        let mut change = make_change_wire(
            op_type::PROJECT_RECURRENCE_OCCURRENCE,
            "recurrence_series",
            series_id.as_str(),
            payload,
        );
        change.change_id = identity.occurrence_change_id;
        change.field = Some("projection".to_string());
        change.created_at = identity.occurrence_link.projected_at;
        validate_pushed_change(&change).unwrap();

        change.created_at = "2026-07-20T00:00:01Z".to_string();
        assert!(
            validate_pushed_change(&change)
                .unwrap_err()
                .to_string()
                .contains("recurrence-deterministic-mismatch")
        );
    }

    fn generated_task(
        title: &str,
        description: &str,
        labels: &[&str],
    ) -> crate::sync::wire::ChangeWire {
        let workspace = test_workspace();
        let series_id: RecurrenceSeriesId = "AAAAAAAAAAAAAAAA".parse().unwrap();
        let schedule = RecurrenceSchedule::new(
            RecurrenceRule::daily(),
            "UTC".parse().unwrap(),
            "2026-07-20".parse().unwrap(),
            None,
            RecurrenceDuePolicy::SameDay,
        );
        let slot_on: NaiveDate = "2026-07-20".parse().unwrap();
        let identity =
            derive_occurrence_identity(&workspace.id, &series_id, &schedule, slot_on).unwrap();
        let slot = crate::recurrence::slot_values(&schedule, slot_on).unwrap();
        let mut payload = ChangePayload::workspace(&workspace)
            .set("task_id", identity.task_id.as_str())
            .set("series_id", series_id.as_str())
            .set("slot_on", slot_on.to_string())
            .set("title", title)
            .set("description", description)
            .set("project_id", "BBBBBBBBBBBBBBBB")
            .set("status", "todo")
            .set("priority", "none")
            .set("available_at", &slot.available_at)
            .set("due_on", slot.due_on.as_deref().unwrap_or(""))
            .set("is_epic", "0")
            .set("labels", labels)
            .set("metadata", Vec::<String>::new())
            .set("created_at", &identity.created_at)
            .set("updated_at", &identity.updated_at)
            .set(
                "occurrence_field_version_seed",
                &identity.field_version_seeds.occurrence,
            )
            .set("frequency", "daily")
            .set("interval", 1)
            .set("weekdays", "")
            .set("timezone", "UTC")
            .set("start_on", "2026-07-20")
            .set("available_local_time", "")
            .set("due_policy", "same_day")
            .into_value();
        let ids = crate::recurrence::derive_proposal_ids(&payload).unwrap();
        payload["task_change_id"] = ids.task_change_id.clone().into();
        payload["occurrence_change_id"] = ids.occurrence_change_id.into();
        payload["task_field_version_seed"] = ids.task_field_version_seed.into();
        let mut change = make_change_wire(
            op_type::CREATE_TASK,
            "task",
            identity.task_id.as_str(),
            payload,
        );
        change.change_id = ids.task_change_id;
        change.created_at = identity.created_at;
        change
    }

    #[test]
    fn generated_task_identity_frames_content_and_rejects_mixed_forms() {
        let proposal = generated_task("ab", "c", &["x", "y"]);
        validate_pushed_change(&proposal).unwrap();
        // Equal content derives equal identities; shifted adjacent strings do not.
        assert_eq!(
            generated_task("ab", "c", &["x", "y"]).change_id,
            proposal.change_id
        );
        assert_ne!(
            generated_task("a", "bc", &["x", "y"]).change_id,
            proposal.change_id
        );
        assert_ne!(
            generated_task("ab", "c", &["xy"]).change_id,
            proposal.change_id
        );

        // The occurrence form stays valid; mixing forms does not.
        let identity = derive_occurrence_identity(
            &test_workspace().id,
            &"AAAAAAAAAAAAAAAA".parse().unwrap(),
            &RecurrenceSchedule::new(
                RecurrenceRule::daily(),
                "UTC".parse().unwrap(),
                "2026-07-20".parse().unwrap(),
                None,
                RecurrenceDuePolicy::SameDay,
            ),
            "2026-07-20".parse().unwrap(),
        )
        .unwrap();
        let mut occurrence = proposal.clone();
        occurrence.change_id = identity.task_change_id.clone();
        occurrence.payload["task_change_id"] = identity.task_change_id.clone().into();
        occurrence.payload["occurrence_change_id"] = identity.occurrence_change_id.clone().into();
        occurrence.payload["task_field_version_seed"] =
            identity.field_version_seeds.task.clone().into();
        validate_pushed_change(&occurrence).unwrap();
        let mut mixed = occurrence.clone();
        mixed.payload["task_field_version_seed"] =
            proposal.payload["task_field_version_seed"].clone();
        assert!(validate_pushed_change(&mixed).is_err());

        // Proposal identities require normalized labels and bound content.
        let mut unsorted = proposal.clone();
        unsorted.payload["labels"] = serde_json::json!(["y", "x"]);
        assert!(validate_pushed_change(&unsorted).is_err());
        let mut edited = proposal;
        edited.payload["title"] = "changed".into();
        assert!(validate_pushed_change(&edited).is_err());
    }
}
