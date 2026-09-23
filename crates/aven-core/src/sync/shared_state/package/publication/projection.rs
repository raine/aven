use std::collections::{BTreeMap, HashMap, HashSet};

use super::catalog::{Images, Parent, Reference};
use super::codec::*;
use super::domain::Mapping;
use super::{Error, Result};
use crate::data_safety::export_types::ExportTables;
use crate::sync::persistence::parent_liveness::ParentState;

pub(super) fn prefix(t: &ExportTables) -> Result<Vec<(u64, String)>> {
    let mut rows = t
        .changes
        .iter()
        .map(|r| {
            let rank =
                u64::try_from(r.server_seq.ok_or(Error::Invalid)?).map_err(|_| Error::Invalid)?;
            Ok((rank, r.change_id.clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    rows.sort();
    Ok(rows)
}

pub(super) fn images(t: &ExportTables, mappings: &[Mapping], mut result: Images) -> Result<Images> {
    bound(number(t.tasks.len())?, RECORD_LIMIT)?;
    bound(number(t.task_attachments.len())?, RECORD_LIMIT)?;
    let mut by_hash = HashMap::new();
    let mut objects = HashSet::new();
    let facts = t
        .blob_inventory
        .iter()
        .map(|r| (r.sha256.as_str(), r))
        .collect::<HashMap<_, _>>();
    valid(facts.len() == t.blob_inventory.len() && mappings.len() == facts.len())?;
    for mapping in mappings {
        valid(by_hash.insert(mapping.sha256.as_str(), mapping).is_none())?;
        let fact = facts.get(mapping.sha256.as_str()).ok_or(Error::Invalid)?;
        match (mapping.classification.as_str(), mapping.object) {
            ("unavailable", None) => {}
            ("current_selected" | "extra_selected", Some(id)) => {
                valid(objects.insert(id))?;
                let object = result
                    .objects
                    .iter()
                    .find(|o| o.id == id)
                    .ok_or(Error::Invalid)?;
                valid(
                    object.artifact.total
                        == u64::try_from(fact.byte_size).map_err(|_| Error::Invalid)?,
                )?;
                valid(
                    object.selection
                        == if mapping.classification == "current_selected" {
                            1
                        } else {
                            2
                        },
                )?;
            }
            _ => return Err(Error::Invalid),
        }
    }
    valid(objects.len() == result.objects.len())?;

    // Missing history, any mismatched deletion base, or a current conflict
    // preserves protection. Final deleted state alone cannot permit cleanup.
    let mut history = t.changes.iter().collect::<Vec<_>>();
    history.sort_by_key(|r| r.server_seq);
    let mut states: HashMap<(String, String), ParentState> = HashMap::new();
    for change in history {
        if change.entity_type != "task"
            || !(change.op_type == "create_task"
                || (matches!(change.op_type.as_str(), "set_field" | "resolve_field")
                    && change.field.as_deref() == Some("deleted")))
        {
            continue;
        }
        let payload: serde_json::Value =
            serde_json::from_str(&change.payload).map_err(|_| Error::Invalid)?;
        let workspace = payload["workspace_id"].as_str().ok_or(Error::Invalid)?;
        let state = states
            .entry((workspace.to_owned(), change.entity_id.clone()))
            .or_default();
        state.apply(
            if change.op_type == "create_task" {
                0
            } else if change.op_type == "resolve_field" {
                2
            } else {
                1
            },
            &change.change_id,
            payload["value"].as_str() == Some("1"),
            if change.op_type == "create_task" {
                Some(
                    payload["task_field_version_seed"]
                        .as_str()
                        .unwrap_or(&change.change_id),
                )
            } else {
                change.base_version.as_deref()
            },
        );
    }
    let versions = t
        .field_versions
        .iter()
        .filter(|r| r.entity_type == "task" && r.field == "deleted")
        .map(|r| {
            (
                (r.workspace_id.as_str(), r.entity_id.as_str()),
                r.version.as_str(),
            )
        })
        .collect::<HashMap<_, _>>();
    let conflicts = t
        .conflicts
        .iter()
        .filter(|r| r.entity_type == "task" && r.field == "deleted" && r.resolved == 0)
        .map(|r| (r.workspace_id.as_str(), r.entity_id.as_str()))
        .collect::<HashSet<_>>();
    let mut parents = BTreeMap::new();
    for task in &t.tasks {
        let identity = (task.workspace_id.to_string(), task.id.to_string());
        let key = (task.workspace_id.as_str(), task.id.as_str());
        let version = versions.get(&key).map(|v| (*v).to_owned());
        let protected = match states.get(&identity) {
            Some(state) => {
                state.protected
                    || state.version.is_none()
                    || state.version != version
                    || state.deleted != (task.deleted != 0)
            }
            None => true,
        } || conflicts.contains(&key);
        parents.insert(
            identity,
            Parent {
                workspace: key.0.to_owned(),
                task: key.1.to_owned(),
                deleted: task.deleted != 0,
                version,
                protected,
            },
        );
    }
    let mut current = HashSet::new();
    for row in &t.task_attachments {
        let parent = parents
            .get(&(row.workspace_id.to_string(), row.task_id.to_string()))
            .ok_or(Error::Invalid)?;
        let mapping = by_hash.get(row.sha256.as_str()).ok_or(Error::Invalid)?;
        if row.deleted == 0 && !parent.deleted {
            current.insert(row.sha256.as_str());
        }
        result.references.push(Reference {
            workspace: row.workspace_id.to_string(),
            task: row.task_id.to_string(),
            reference: row.attachment_id.clone(),
            deleted: row.deleted != 0,
            object: mapping.object,
        });
    }
    for mapping in mappings {
        if mapping.object.is_some() {
            valid(
                (mapping.classification == "current_selected")
                    == current.contains(mapping.sha256.as_str()),
            )?;
        }
    }
    result.parents = parents.into_values().collect();
    result
        .references
        .sort_by(|a, b| (&a.workspace, &a.reference).cmp(&(&b.workspace, &b.reference)));
    result.objects.sort_by_key(|o| o.id);
    Ok(result)
}
