use super::*;
use crate::sync::wire::ChangeWire;
use anyhow::Context as _;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::{collections::HashSet, fmt};

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Projection {
    None,
    Ref {
        workspace: String,
        task: String,
        reference: String,
        descriptor: Vec<u8>,
        deleted: bool,
        version: Option<String>,
    },
    Unref {
        workspace: String,
        task: String,
        reference: String,
    },
    Parent {
        action: u8,
        workspace: String,
        task: String,
        deleted: bool,
        version: Option<String>,
    },
}
impl Projection {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Ref {
                workspace,
                task,
                reference,
                descriptor,
                deleted,
                version,
            } => {
                let mut out = vec![2];
                for text in [workspace, task, reference] {
                    codec::encode_text(&mut out, text);
                }
                out.extend((descriptor.len() as u32).to_be_bytes());
                out.extend(descriptor);
                out.push(u8::from(*deleted));
                out.push(u8::from(version.is_some()));
                if let Some(v) = version {
                    codec::encode_text(&mut out, v);
                }
                return out;
            }
            Self::Unref {
                workspace,
                task,
                reference,
            } => {
                let mut out = vec![3];
                for text in [workspace, task, reference] {
                    codec::encode_text(&mut out, text);
                }
                return out;
            }
            _ => {}
        }
        let Self::Parent {
            action,
            workspace,
            task,
            deleted,
            version,
        } = self
        else {
            return vec![0];
        };
        let mut out = vec![1, *action];
        codec::encode_text(&mut out, workspace);
        codec::encode_text(&mut out, task);
        out.push(u8::from(*deleted));
        out.push(u8::from(version.is_some()));
        if let Some(v) = version {
            codec::encode_text(&mut out, v);
        }
        out
    }
    pub fn decode(input: &[u8]) -> Result<Self> {
        codec::decode_projection(input)
    }
}

/// Payload keys accepted for each supported operation, or None when unsupported.
pub(super) fn payload_keys(op: &str) -> Option<&'static [&'static str]> {
    Some(match op {
        "create_task" => &[
            "title",
            "description",
            "project_id",
            "project_key",
            "project_name",
            "project_prefix",
            "status",
            "priority",
            "source",
            "available_at",
            "due_on",
            "is_epic",
            "labels",
            "metadata",
            "created_at",
            "task_field_version_seed",
        ],
        "set_field" | "resolve_field" => &[
            "value",
            "project_id",
            "project_key",
            "project_name",
            "project_prefix",
        ],
        "label_add" | "label_remove" => &["label"],
        "note_add" => &["note_id", "body", "created_at"],
        "note_edit" => &["note_id", "body", "edited_at"],
        "note_delete" => &["note_id", "body", "deleted_at"],
        "dependency_add" | "dependency_remove" => &["depends_on_task_id"],
        "related_add" | "related_remove" => &["related_task_id"],
        "epic_link_add" | "epic_link_remove" => &["epic_task_id", "created_at"],
        "create_metadata_field" => &["key", "created_at"],
        "set_metadata_field" => &["key", "conflict_resolution"],
        "set_task_metadata" | "remove_task_metadata" => {
            &["field_id", "key", "value", "conflict_resolution"]
        }
        "create_recurrence_series" => &[
            "series_id",
            "title",
            "description",
            "project_id",
            "project_key",
            "project_name",
            "project_prefix",
            "priority",
            "initial_status",
            "labels",
            "metadata",
            "frequency",
            "interval",
            "weekdays",
            "timezone",
            "start_on",
            "available_local_time",
            "due_policy",
            "state",
            "stopped_at",
            "created_at",
            "updated_at",
        ],
        "update_recurrence_template" => &[
            "fields",
            "base_versions",
            "labels",
            "labels_changed",
            "updated_at",
            "conflict_resolution",
        ],
        "project_recurrence_occurrence" => &[
            "series_id",
            "slot_on",
            "task_id",
            "projected_at",
            "task_change_id",
            "occurrence_change_id",
            "task_field_version_seed",
            "occurrence_field_version_seed",
            "frequency",
            "interval",
            "weekdays",
            "timezone",
            "start_on",
            "available_local_time",
            "due_policy",
        ],
        "resolve_recurrence_occurrence" => &[
            "slot_on",
            "task_id",
            "outcome",
            "resolved_at",
            "task_status",
            "task_status_change_id",
            "successor_task_id",
            "conflict_resolution",
            "frequency",
            "interval",
            "weekdays",
            "timezone",
            "start_on",
            "available_local_time",
            "due_policy",
        ],
        "set_recurrence_state" | "stop_recurrence_series" => {
            &["state", "stopped_at", "changed_at", "conflict_resolution"]
        }
        "open_recurrence_pause" => &[
            "interval_id",
            "paused_at",
            "suspended_slot_on",
            "suspended_task_id",
        ],
        "close_recurrence_pause" => &["interval_id", "paused_at", "resumed_at"],
        "set_recurrence_metadata" | "remove_recurrence_metadata" => {
            &["field_id", "key", "value", "conflict_resolution"]
        }
        "create_project" => &["key", "name", "prefix", "created_at"],
        "set_project_metadata" => &["key", "name", "prefix", "updated_at"],
        "project_delete" => &["deleted_at"],
        "create_label" => &["name", "created_at"],
        "set_label_name" => &["name", "new_name", "renamed_at"],
        "label_delete" => &["name", "deleted_at"],
        "label_restore" => &[
            "name",
            "created_at",
            "task_ids",
            "series_ids",
            "restored_at",
        ],
        "create_workspace" => &["key", "name", "created_at"],
        "set_workspace_field" => &["value"],
        "attachment_add" => &[
            "attachment_id",
            "sha256",
            "byte_size",
            "media_type",
            "filename",
            "alt_text",
            "width",
            "height",
            "created_at",
        ],
        "attachment_delete" => &["attachment_id", "filename", "media_type", "deleted_at"],
        _ => return None,
    })
}

pub(super) fn validate(c: &ChangeWire) -> Result<Projection> {
    valid(c.server_seq.is_none() && c.change_id.len() <= 256)?;
    let keys = payload_keys(&c.op_type).context("error encrypted-tail-operation-unsupported")?;
    crate::sync::wire::validate_local_change_shape(c)
        .and_then(|()| crate::sync::protocol::validate_change(18, c))
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-domain"))?;
    let p = c
        .payload
        .as_object()
        .context("error encrypted-tail-payload")?;
    let occurrence_keys = [
        "task_id",
        "series_id",
        "slot_on",
        "updated_at",
        "task_change_id",
        "occurrence_change_id",
        "occurrence_field_version_seed",
        "frequency",
        "interval",
        "weekdays",
        "timezone",
        "start_on",
        "available_local_time",
        "due_policy",
    ];
    valid(p.keys().all(|k| {
        keys.contains(&k.as_str())
            || matches!(k.as_str(), "workspace_id" | "workspace_key")
            || (c.op_type == "create_task"
                && p.get("series_id").is_some_and(Value::is_string)
                && occurrence_keys.contains(&k.as_str()))
    }))?;
    if let Some(flag) = p.get("conflict_resolution") {
        valid(flag.is_boolean())?;
    }
    let workspace_op = matches!(
        c.op_type.as_str(),
        "create_workspace" | "set_workspace_field"
    );
    if workspace_op
        || matches!(
            c.op_type.as_str(),
            "set_project_metadata"
                | "project_delete"
                | "set_label_name"
                | "label_delete"
                | "label_restore"
        )
    {
        // These reducers ignore the columns, so accepting them would be ambiguous.
        valid(
            c.base_version.is_none() && c.field.is_none() == (c.op_type != "set_workspace_field"),
        )?;
    }
    if c.op_type == "set_label_name" {
        valid(p.get("new_name") != p.get("name"))?;
    }
    // Workspace operations are database-wide and identify the workspace as their entity.
    let workspace = if workspace_op {
        valid(!p.contains_key("workspace_id") && !p.contains_key("workspace_key"))?;
        c.entity_id.as_str()
    } else {
        p.get("workspace_id")
            .and_then(Value::as_str)
            .context("error encrypted-tail-workspace")?
    };
    workspace
        .parse::<crate::ids::WorkspaceId>()
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-workspace"))?;
    if c.op_type == "create_task" {
        valid(c.field.is_none() && c.base_version.is_none())?;
        let seed = match p.get("task_field_version_seed") {
            None => &c.change_id,
            Some(v) => v.as_str().context("error encrypted-tail-seed")?,
        };
        valid(!seed.is_empty() && seed.len() <= 256)?;
        seed.parse::<crate::ids::TaskId>()
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-seed"))?;
        return Ok(Projection::Parent {
            action: 0,
            workspace: workspace.into(),
            task: c.entity_id.clone(),
            deleted: false,
            version: Some(seed.into()),
        });
    }
    if matches!(c.op_type.as_str(), "set_field" | "resolve_field")
        && c.field.as_deref() == Some("deleted")
    {
        valid(
            c.base_version
                .as_ref()
                .is_none_or(|v| !v.is_empty() && v.len() <= 256),
        )?;
        return Ok(Projection::Parent {
            action: if c.op_type == "set_field" { 1 } else { 2 },
            workspace: workspace.into(),
            task: c.entity_id.clone(),
            deleted: p["value"] == "1",
            version: c.base_version.clone(),
        });
    }
    if matches!(c.op_type.as_str(), "attachment_add" | "attachment_delete") {
        valid(c.field.as_deref() == Some("attachments") && c.base_version.is_none())?;
    }
    if c.op_type == "attachment_delete" {
        return Ok(Projection::Unref {
            workspace: workspace.into(),
            task: c.entity_id.clone(),
            reference: p["attachment_id"]
                .as_str()
                .context("error encrypted-image-reference")?
                .into(),
        });
    }
    Ok(Projection::None)
}

pub(super) fn validate_projection(c: &ChangeWire, projection: &Projection) -> Result<()> {
    let expected = validate(c)?;
    if c.op_type == "attachment_add" {
        let Projection::Ref {
            workspace,
            task,
            reference,
            descriptor,
            ..
        } = projection
        else {
            anyhow::bail!("error encrypted-image-projection")
        };
        let d = super::attachments::codec::Descriptor::decode(descriptor)?;
        valid(
            c.payload["workspace_id"].as_str() == Some(workspace)
                && c.entity_id == *task
                && c.payload["attachment_id"].as_str() == Some(reference)
                && c.payload["byte_size"].as_u64() == Some(d.artifact.total),
        )
    } else {
        valid(expected == *projection)
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<ChangeWire> {
    valid(bytes.len() <= 131072)?;
    let v = strict_value(bytes)?;
    let keys = [
        "change_id",
        "client_id",
        "local_seq",
        "entity_type",
        "entity_id",
        "field",
        "op_type",
        "payload",
        "base_version",
        "created_at",
        "server_seq",
    ];
    let o = v.as_object().context("error encrypted-tail-json")?;
    valid(o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))?;
    let c: ChangeWire =
        serde_json::from_value(v).map_err(|_| anyhow::anyhow!("error encrypted-tail-json"))?;
    validate(&c)?;
    Ok(c)
}

/// Check integer token range before serde_json can round overflow into f64.
pub(super) fn strict_value(bytes: &[u8]) -> Result<Value> {
    let mut string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if string {
            if escaped {
                escaped = false
            } else if b == b'\\' {
                escaped = true
            } else if b == b'"' {
                string = false
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            string = true;
            i += 1;
            continue;
        }
        if b == b'-' || b.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || b".eE+-".contains(&bytes[i])) {
                i += 1
            }
            let token = std::str::from_utf8(&bytes[start..i])?;
            if !token.contains(['.', 'e', 'E']) {
                valid(if token.starts_with('-') {
                    token.parse::<i64>().is_ok()
                } else {
                    token.parse::<u64>().is_ok()
                })?;
            }
        } else {
            i += 1
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = Strict(0)
        .deserialize(&mut decoder)
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-json"))?;
    decoder
        .end()
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-json"))?;
    Ok(value)
}
struct Strict(usize);
impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> std::result::Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Strict {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Value, A::Error> {
        if self.0 >= 64 {
            return Err(de::Error::custom("depth"));
        }
        let mut result = Vec::new();
        while let Some(v) = a.next_element_seed(Strict(self.0 + 1))? {
            result.push(v)
        }
        Ok(Value::Array(result))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<Value, A::Error> {
        if self.0 >= 64 {
            return Err(de::Error::custom("depth"));
        }
        let mut seen = HashSet::new();
        let mut result = serde_json::Map::new();
        while let Some(k) = a.next_key::<String>()? {
            if !seen.insert(k.clone()) {
                return Err(de::Error::custom("duplicate"));
            }
            result.insert(k, a.next_value_seed(Strict(self.0 + 1))?);
        }
        Ok(Value::Object(result))
    }
}
