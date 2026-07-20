use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{TaskId, WorkspaceId, encode_crockford};

use super::schedule::{RecurrenceScheduleError, slot_values};
use super::{RecurrenceSchedule, RecurrenceSeriesId};

const TASK_ID_DOMAIN: &[u8] = b"aven recurrence task v1";
const TASK_CHANGE_DOMAIN: &[u8] = b"aven recurrence task change v1";
const OCCURRENCE_CHANGE_DOMAIN: &[u8] = b"aven recurrence occurrence change v1";
const TASK_FIELD_VERSION_DOMAIN: &[u8] = b"aven recurrence task field version v1";
const OCCURRENCE_FIELD_VERSION_DOMAIN: &[u8] = b"aven recurrence occurrence field version v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceOccurrenceIdentity {
    pub task_id: TaskId,
    pub task_change_id: String,
    pub occurrence_change_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub occurrence_link: RecurrenceOccurrenceLink,
    pub field_version_seeds: RecurrenceFieldVersionSeeds,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceOccurrenceLink {
    pub workspace_id: WorkspaceId,
    pub series_id: RecurrenceSeriesId,
    pub slot_on: NaiveDate,
    pub task_id: TaskId,
    pub projected_at: String,
    pub change_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceFieldVersionSeeds {
    pub task: String,
    pub occurrence: String,
}

pub fn derive_occurrence_identity(
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    schedule: &RecurrenceSchedule,
    slot_on: NaiveDate,
) -> Result<RecurrenceOccurrenceIdentity, RecurrenceScheduleError> {
    let slot = slot_values(schedule, slot_on)?;
    let slot_text = slot_on.format("%Y-%m-%d").to_string();
    let components = [
        workspace_id.as_str(),
        series_id.as_str(),
        slot_text.as_str(),
    ];
    let task_id = derive_id(TASK_ID_DOMAIN, &components)
        .parse::<TaskId>()
        .expect("derived recurrence task ID is valid");
    let task_change_id = derive_id(TASK_CHANGE_DOMAIN, &components);
    let occurrence_change_id = derive_id(OCCURRENCE_CHANGE_DOMAIN, &components);
    let field_version_seeds = RecurrenceFieldVersionSeeds {
        task: derive_id(TASK_FIELD_VERSION_DOMAIN, &components),
        occurrence: derive_id(OCCURRENCE_FIELD_VERSION_DOMAIN, &components),
    };
    let occurrence_link = RecurrenceOccurrenceLink {
        workspace_id: workspace_id.clone(),
        series_id: series_id.clone(),
        slot_on,
        task_id: task_id.clone(),
        projected_at: slot.boundary_at.clone(),
        change_id: occurrence_change_id.clone(),
    };
    Ok(RecurrenceOccurrenceIdentity {
        task_id,
        task_change_id,
        occurrence_change_id,
        created_at: slot.boundary_at.clone(),
        updated_at: slot.boundary_at,
        occurrence_link,
        field_version_seeds,
    })
}

fn derive_id(domain: &[u8], components: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for component in components {
        hasher.update(component.as_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 10];
    bytes.copy_from_slice(&digest[..10]);
    encode_crockford(&bytes)
}
