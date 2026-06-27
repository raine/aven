use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::types::Project;
use crate::types::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskField {
    Title,
    Description,
    Project,
    Status,
    Priority,
    Deleted,
}

impl TaskField {
    pub(crate) const VERSIONED: [TaskField; 6] = [
        TaskField::Title,
        TaskField::Description,
        TaskField::Project,
        TaskField::Status,
        TaskField::Priority,
        TaskField::Deleted,
    ];

    pub(crate) fn parse(field: &str) -> Option<Self> {
        match field {
            "title" => Some(Self::Title),
            "description" => Some(Self::Description),
            "project" => Some(Self::Project),
            "status" => Some(Self::Status),
            "priority" => Some(Self::Priority),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }

    pub(crate) fn is_project(self) -> bool {
        matches!(self, Self::Project)
    }

    pub(crate) fn is_scalar(self) -> bool {
        !self.is_project()
    }

    pub(crate) fn parse_or_unknown(field: &str) -> Result<Self> {
        Self::parse(field).ok_or_else(|| anyhow!("error unknown-field field={field}"))
    }

    pub(crate) fn parse_for_sync(field: &str) -> Result<Self> {
        Self::parse(field).ok_or_else(|| anyhow!("error invalid-sync-change field={field}"))
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::Project => "project",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::Deleted => "deleted",
        }
    }

    pub(crate) fn validate_value(self, value: &str) -> Result<()> {
        match self {
            Self::Status => validate_choice("status", value, STATUSES),
            Self::Priority => validate_choice("priority", value, PRIORITIES),
            Self::Deleted if matches!(value, "0" | "1") => Ok(()),
            Self::Deleted => anyhow::bail!("error invalid-deleted value={value}"),
            _ => Ok(()),
        }
    }

    pub(crate) fn updates_queue_activity(self) -> bool {
        matches!(self, Self::Status | Self::Priority)
    }

    pub(crate) fn scalar_payload(
        self,
        workspace_id: &str,
        workspace_key: &str,
        value: &str,
    ) -> Result<Value> {
        ensure!(self.is_scalar(), "error project-update-requires-project-id");
        Ok(json!({
            "workspace_id": workspace_id,
            "workspace_key": workspace_key,
            "value": value,
        }))
    }

    pub(crate) fn project_payload(
        workspace_id: &str,
        workspace_key: &str,
        project: &Project,
    ) -> Value {
        json!({
            "workspace_id": workspace_id,
            "workspace_key": workspace_key,
            "value": &project.id,
            "project_id": &project.id,
            "project_key": &project.key,
            "project_name": &project.name,
            "project_prefix": &project.prefix,
        })
    }

    pub(crate) fn current_value(self, task: &Task) -> String {
        match self {
            Self::Title => task.title.clone(),
            Self::Description => task.description.clone(),
            Self::Project => task.project_id.clone(),
            Self::Status => task.status.clone(),
            Self::Priority => task.priority.clone(),
            Self::Deleted => {
                if task.deleted {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskField;
    use crate::types::Project;

    #[test]
    fn versioned_fields_keep_protocol_order() {
        let fields: Vec<_> = TaskField::VERSIONED
            .into_iter()
            .map(TaskField::as_str)
            .collect();
        assert_eq!(
            fields,
            [
                "title",
                "description",
                "project",
                "status",
                "priority",
                "deleted"
            ]
        );
    }

    #[test]
    fn project_fields_classify_projection() {
        assert!(TaskField::Project.is_project());
        assert!(TaskField::Title.is_scalar());
        assert!(TaskField::Description.is_scalar());
        assert!(TaskField::Status.is_scalar());
        assert!(TaskField::Priority.is_scalar());
        assert!(TaskField::Deleted.is_scalar());
    }

    #[test]
    fn parse_unknown_field_errors_are_canonicalized() {
        assert_eq!(
            TaskField::parse_or_unknown("missing")
                .unwrap_err()
                .to_string(),
            "error unknown-field field=missing"
        );
        assert_eq!(
            TaskField::parse_for_sync("missing")
                .unwrap_err()
                .to_string(),
            "error invalid-sync-change field=missing"
        );
    }

    #[test]
    fn deleted_validation() {
        assert!(TaskField::Deleted.validate_value("0").is_ok());
        assert!(TaskField::Deleted.validate_value("1").is_ok());
        assert_eq!(
            TaskField::Deleted
                .validate_value("x")
                .unwrap_err()
                .to_string(),
            "error invalid-deleted value=x"
        );
    }

    #[test]
    fn scalar_payload_shape() {
        let payload = TaskField::Title
            .scalar_payload("wk", "key", "v")
            .expect("valid scalar field");
        assert_eq!(
            payload,
            serde_json::json!({
                "workspace_id": "wk",
                "workspace_key": "key",
                "value": "v",
            })
        );
        assert!(TaskField::Project.scalar_payload("wk", "key", "v").is_err());
    }

    #[test]
    fn project_payload_shape() {
        let project = Project {
            id: "prj1".to_string(),
            workspace_id: "wk".to_string(),
            key: "proj".to_string(),
            name: "Project".to_string(),
            prefix: "pp".to_string(),
        };
        assert_eq!(
            TaskField::project_payload("wk", "key", &project),
            serde_json::json!({
                "workspace_id": "wk",
                "workspace_key": "key",
                "value": "prj1",
                "project_id": "prj1",
                "project_key": "proj",
                "project_name": "Project",
                "project_prefix": "pp",
            })
        );
    }
}
