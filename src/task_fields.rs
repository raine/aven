use anyhow::Result;

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
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

    pub(crate) fn current_value(self, task: &Task) -> String {
        match self {
            Self::Title => task.title.clone(),
            Self::Description => task.description.clone(),
            Self::Project => task.project_key.clone(),
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
}
