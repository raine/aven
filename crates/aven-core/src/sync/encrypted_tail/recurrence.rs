use crate::{ids::WorkspaceId, recurrence::RecurrenceSeriesId, sync::wire::ChangeWire};
use anyhow::{Context, Result};

pub(super) fn is_deterministic(change: &ChangeWire) -> bool {
    change.op_type == "project_recurrence_occurrence"
        || (change.op_type == "create_task" && change.payload["series_id"].is_string())
}

pub(super) fn affected_series(
    change: &ChangeWire,
) -> Result<Option<(WorkspaceId, RecurrenceSeriesId)>> {
    let series = if change.entity_type == "recurrence_series" {
        Some(change.entity_id.as_str())
    } else {
        change.payload["series_id"].as_str()
    };
    series
        .map(|series| {
            Ok((
                change.payload["workspace_id"]
                    .as_str()
                    .context("error encrypted-tail-workspace")?
                    .parse()?,
                series.parse()?,
            ))
        })
        .transpose()
}
