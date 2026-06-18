use anyhow::{Result, bail};

pub(crate) const STATUSES: &[&str] = &["inbox", "backlog", "todo", "active", "done", "canceled"];
pub(crate) const PRIORITIES: &[&str] = &["none", "low", "medium", "high", "urgent"];

pub(crate) fn validate_choice(name: &str, value: &str, choices: &[&str]) -> Result<()> {
    if choices.contains(&value) {
        Ok(())
    } else {
        bail!(
            "error invalid-{name} input={} choices={}",
            value,
            choices.join(",")
        );
    }
}
