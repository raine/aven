use anyhow::Result;

use crate::choices::{TaskPriority, TaskStatus};

pub(super) fn validate_priority(priority: &str) -> Result<()> {
    TaskPriority::parse(priority)
        .map(|_| ())
        .map_err(Into::into)
}

pub(super) fn validate_optional_status(status: Option<&str>) -> Result<()> {
    if let Some(status) = status {
        TaskStatus::parse(status)?;
    }
    Ok(())
}

pub(super) fn validate_optional_priority(priority: Option<&str>) -> Result<()> {
    if let Some(priority) = priority {
        validate_priority(priority)?;
    }
    Ok(())
}
