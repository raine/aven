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

const PROPOSAL_DOMAIN: &[u8] = b"aven recurrence proposal v1\0";
const PROPOSAL_TASK_CHANGE_DOMAIN: &[u8] = b"aven recurrence proposal task change v1";
const PROPOSAL_OCCURRENCE_CHANGE_DOMAIN: &[u8] = b"aven recurrence proposal occurrence change v1";
const PROPOSAL_TASK_FIELD_VERSION_DOMAIN: &[u8] = b"aven recurrence proposal task field version v1";

/// Operation identities of one generation proposal. Replicas that generate an
/// occurrence from equal template contents and schedule context derive equal
/// identities; any difference yields distinct operations for the same task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceProposalIds {
    pub task_change_id: String,
    pub occurrence_change_id: String,
    pub task_field_version_seed: String,
}

/// Derives proposal identities from a generated `create_task` payload. Labels must
/// be strictly ascending and metadata strictly ascending by key with unique field
/// IDs, so equal proposals always have equal payload bytes. The digest input is a
/// JSON array of strings and arrays only, which frames every value unambiguously.
pub fn derive_proposal_ids(payload: &serde_json::Value) -> anyhow::Result<RecurrenceProposalIds> {
    use anyhow::{Context, ensure};
    let text = |key: &str| {
        payload[key]
            .as_str()
            .with_context(|| format!("error invalid-sync-change recurrence-proposal field={key}"))
    };
    let labels = payload["labels"]
        .as_array()
        .context("error invalid-sync-change recurrence-proposal field=labels")?
        .iter()
        .map(|label| {
            label
                .as_str()
                .context("error invalid-sync-change recurrence-proposal field=labels")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    ensure!(
        labels.windows(2).all(|pair| pair[0] < pair[1]),
        "error invalid-sync-change recurrence-proposal field=labels"
    );
    let metadata = payload["metadata"]
        .as_array()
        .context("error invalid-sync-change recurrence-proposal field=metadata")?
        .iter()
        .map(|entry| {
            let value = |key: &str| {
                entry[key]
                    .as_str()
                    .context("error invalid-sync-change recurrence-proposal field=metadata")
            };
            Ok([value("field_id")?, value("key")?, value("value")?])
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut field_ids = metadata.iter().map(|entry| entry[0]).collect::<Vec<_>>();
    field_ids.sort_unstable();
    field_ids.dedup();
    ensure!(
        metadata.windows(2).all(|pair| pair[0][1] < pair[1][1])
            && field_ids.len() == metadata.len(),
        "error invalid-sync-change recurrence-proposal field=metadata"
    );
    let input = serde_json::json!([
        text("workspace_id")?,
        text("series_id")?,
        text("slot_on")?,
        text("title")?,
        text("description")?,
        text("project_id")?,
        text("status")?,
        text("priority")?,
        labels,
        metadata,
        text("available_local_time")?,
        text("due_policy")?,
    ]);
    let mut hasher = Sha256::new();
    hasher.update(PROPOSAL_DOMAIN);
    hasher.update(serde_json::to_vec(&input)?);
    let digest = hex::encode(hasher.finalize());
    Ok(proposal_ids_for_task_change(derive_id(
        PROPOSAL_TASK_CHANGE_DOMAIN,
        &[digest.as_str()],
    )))
}

/// Derives the projection and field-version identities bound to a proposal's
/// task change, which is all a projection record can check on its own.
pub fn proposal_ids_for_task_change(task_change_id: String) -> RecurrenceProposalIds {
    RecurrenceProposalIds {
        occurrence_change_id: derive_id(PROPOSAL_OCCURRENCE_CHANGE_DOMAIN, &[&task_change_id]),
        task_field_version_seed: derive_id(PROPOSAL_TASK_FIELD_VERSION_DOMAIN, &[&task_change_id]),
        task_change_id,
    }
}

impl RecurrenceOccurrenceIdentity {
    /// Whether a generated record carries proposal-form rather than occurrence-form
    /// identities.
    pub fn is_proposal(&self, task_change_id: &str) -> bool {
        task_change_id != self.task_change_id
    }

    /// Expected identities of a generated `create_task` payload in its own form.
    pub fn generated_task_ids(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<RecurrenceProposalIds> {
        match payload["task_change_id"].as_str() {
            Some(id) if !self.is_proposal(id) => Ok(self.projection_ids(id)),
            _ => derive_proposal_ids(payload),
        }
    }

    /// Expected identities of a projection referencing `task_change_id`.
    pub fn projection_ids(&self, task_change_id: &str) -> RecurrenceProposalIds {
        if self.is_proposal(task_change_id) {
            return proposal_ids_for_task_change(task_change_id.to_owned());
        }
        RecurrenceProposalIds {
            task_change_id: self.task_change_id.clone(),
            occurrence_change_id: self.occurrence_change_id.clone(),
            task_field_version_seed: self.field_version_seeds.task.clone(),
        }
    }

    /// Identities of a stored generated `create_task` or projection record, when it
    /// is a valid generation of this occurrence in its own derivation form.
    pub fn stored_generation_ids(
        &self,
        payload: &serde_json::Value,
        slot_on: NaiveDate,
        projection: bool,
    ) -> Option<RecurrenceProposalIds> {
        let ids = if projection {
            self.projection_ids(payload["task_change_id"].as_str()?)
        } else {
            self.generated_task_ids(payload).ok()?
        };
        let slot = slot_on.format("%Y-%m-%d").to_string();
        let matches = [
            ("task_id", self.task_id.as_str()),
            ("series_id", self.occurrence_link.series_id.as_str()),
            ("slot_on", slot.as_str()),
            ("task_change_id", ids.task_change_id.as_str()),
            ("occurrence_change_id", ids.occurrence_change_id.as_str()),
            (
                "task_field_version_seed",
                ids.task_field_version_seed.as_str(),
            ),
            (
                "occurrence_field_version_seed",
                self.field_version_seeds.occurrence.as_str(),
            ),
        ]
        .into_iter()
        .all(|(key, expected)| payload[key].as_str() == Some(expected))
            && (!projection
                || payload["projected_at"].as_str()
                    == Some(self.occurrence_link.projected_at.as_str()));
        matches.then_some(ids)
    }
}
