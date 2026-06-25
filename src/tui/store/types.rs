#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MutationMessage {
    pub(crate) message: String,
    pub(crate) selected: Option<usize>,
}

impl MutationMessage {
    pub(crate) fn new(message: impl Into<String>, selected: Option<usize>) -> Self {
        Self {
            message: message.into(),
            selected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConflictTarget {
    pub(crate) task_id: String,
    pub(crate) display_ref: String,
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarTarget {
    All,
    Inbox,
    Active,
    Backlog,
    Todo,
    Done,
    Conflicts,
    Project(String),
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarEntry {
    pub(crate) label: String,
    pub(crate) count: i64,
    pub(crate) target: Option<SidebarTarget>,
    pub(crate) section: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncStatusCheck {
    pub(crate) ok: bool,
    pub(crate) value: String,
}

impl SyncStatusCheck {
    pub(crate) fn new(ok: bool, value: impl Into<String>) -> Self {
        Self {
            ok,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TuiSyncStatus {
    pub(crate) enabled: bool,
    pub(crate) config_error: Option<String>,
    pub(crate) configured_server: Option<SyncStatusCheck>,
    pub(crate) pinned_server: Option<String>,
    pub(crate) server_match: Option<SyncStatusCheck>,
    pub(crate) daemon_server: Option<SyncStatusCheck>,
    pub(crate) auth_token_configured: bool,
    pub(crate) interval_seconds: u64,
    pub(crate) daemon_wake: SyncStatusCheck,
    pub(crate) pending_changes: i64,
    pub(crate) conflicts: i64,
    pub(crate) sync_cursor: Option<String>,
    pub(crate) local_sequence: Option<String>,
}

impl Default for TuiSyncStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            config_error: None,
            configured_server: None,
            pinned_server: None,
            server_match: None,
            daemon_server: None,
            auth_token_configured: false,
            interval_seconds: 30,
            daemon_wake: SyncStatusCheck::new(true, "not checked"),
            pending_changes: 0,
            conflicts: 0,
            sync_cursor: None,
            local_sequence: None,
        }
    }
}

impl TuiSyncStatus {
    pub(crate) fn has_sync_error(&self) -> bool {
        self.config_error.is_some()
            || (self.enabled
                && (!self
                    .configured_server
                    .as_ref()
                    .is_some_and(|check| check.ok)
                    || self.server_match.as_ref().is_some_and(|check| !check.ok)
                    || self.daemon_server.as_ref().is_some_and(|check| !check.ok)
                    || !self.daemon_wake.ok))
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("enabled: {}", yes_no(self.enabled)),
            format!(
                "configured server: {}",
                check_value(self.configured_server.as_ref(), "not configured")
            ),
            format!(
                "pinned server: {}",
                self.pinned_server.as_deref().unwrap_or("none")
            ),
        ];
        if let Some(error) = &self.config_error {
            lines.push(format!("config: {error} (check failed)"));
        }
        if let Some(server_match) = &self.server_match {
            lines.push(format!(
                "server match: {}",
                if server_match.ok {
                    "yes".to_string()
                } else {
                    server_match.value.clone()
                }
            ));
        }
        lines.extend([
            format!(
                "daemon server: {}",
                check_value(self.daemon_server.as_ref(), "not configured")
            ),
            format!(
                "auth token: {}",
                if self.auth_token_configured {
                    "configured"
                } else {
                    "not configured"
                }
            ),
            format!("interval: {} seconds", self.interval_seconds),
            format!("daemon wake: {}", self.daemon_wake.value),
            String::new(),
            format!("pending changes: {}", self.pending_changes),
            format!("conflicts: {}", self.conflicts),
            format!(
                "sync cursor: {}",
                self.sync_cursor.as_deref().unwrap_or("missing")
            ),
            format!(
                "local sequence: {}",
                self.local_sequence.as_deref().unwrap_or("missing")
            ),
        ]);
        lines
    }
}

fn check_value(check: Option<&SyncStatusCheck>, fallback: &str) -> String {
    check
        .map(|check| {
            if check.ok {
                check.value.clone()
            } else {
                format!("{} (check failed)", check.value)
            }
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
