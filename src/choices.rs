use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskStatus {
    Inbox,
    Backlog,
    Todo,
    Active,
    Done,
    Canceled,
}

impl TaskStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::Active => "active",
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "inbox" => Ok(Self::Inbox),
            "backlog" => Ok(Self::Backlog),
            "todo" => Ok(Self::Todo),
            "active" => Ok(Self::Active),
            "done" => Ok(Self::Done),
            "canceled" => Ok(Self::Canceled),
            _ => bail!(
                "error invalid-status input={} choices={}",
                value,
                STATUSES.join(",")
            ),
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for TaskStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

pub(crate) const STATUSES: &[&str] = &[
    TaskStatus::Inbox.as_str(),
    TaskStatus::Backlog.as_str(),
    TaskStatus::Todo.as_str(),
    TaskStatus::Active.as_str(),
    TaskStatus::Done.as_str(),
    TaskStatus::Canceled.as_str(),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskPriority {
    None,
    Low,
    Medium,
    High,
    Urgent,
}

impl TaskPriority {
    pub(crate) const ALL: [Self; 5] = [
        Self::None,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Urgent,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "urgent" => Ok(Self::Urgent),
            _ => bail!(
                "error invalid-priority input={} choices={}",
                value,
                PRIORITIES.join(",")
            ),
        }
    }
}

impl std::fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for TaskPriority {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

pub(crate) const PRIORITIES: &[&str] = &[
    TaskPriority::None.as_str(),
    TaskPriority::Low.as_str(),
    TaskPriority::Medium.as_str(),
    TaskPriority::High.as_str(),
    TaskPriority::Urgent.as_str(),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_priority_parse_display_and_reject_invalid_values() {
        assert_eq!(TaskStatus::parse("active").unwrap(), TaskStatus::Active);
        assert_eq!(TaskStatus::Active.as_str(), "active");
        assert_eq!(TaskStatus::Active.to_string(), "active");
        assert_eq!(
            TaskStatus::parse("blocked").unwrap_err().to_string(),
            "error invalid-status input=blocked choices=inbox,backlog,todo,active,done,canceled"
        );

        assert_eq!(TaskPriority::parse("urgent").unwrap(), TaskPriority::Urgent);
        assert_eq!(TaskPriority::Urgent.as_str(), "urgent");
        assert_eq!(TaskPriority::Urgent.to_string(), "urgent");
        assert_eq!(
            TaskPriority::parse("soon").unwrap_err().to_string(),
            "error invalid-priority input=soon choices=none,low,medium,high,urgent"
        );
    }
}
