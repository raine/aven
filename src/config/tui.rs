use std::collections::BTreeSet;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::custom_commands::CustomTuiCommandConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default)]
    pub table: TaskTableConfig,
    #[serde(default)]
    pub sidebar: SidebarConfig,
    #[serde(default = "default_task_columns")]
    pub columns: Vec<TaskColumnConfig>,
    #[serde(default)]
    pub commands: Vec<CustomTuiCommandConfig>,
    #[serde(default)]
    pub appearance: TuiAppearance,
}

/// Which theme the TUI draws with. `Auto` matches the terminal background.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TuiAppearance {
    #[default]
    Auto,
    Dark,
    Light,
    Terminal,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            table: TaskTableConfig::default(),
            sidebar: SidebarConfig::default(),
            columns: default_task_columns(),
            commands: Vec::new(),
            appearance: TuiAppearance::Auto,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTableConfig {
    #[serde(default = "default_table_columns")]
    pub columns: Vec<TableColumn>,
    #[serde(default)]
    pub compact_status: bool,
}

impl Default for TaskTableConfig {
    fn default() -> Self {
        Self {
            columns: default_table_columns(),
            compact_status: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableColumn {
    Ref,
    Title,
    Labels,
    Metadata,
    Project,
    Status,
    Priority,
    Time,
    Due,
}

impl TableColumn {
    /// Every column, in semantic index order. Indexes geometry and cell arrays.
    pub const ALL: [Self; 9] = [
        Self::Ref,
        Self::Title,
        Self::Labels,
        Self::Metadata,
        Self::Project,
        Self::Status,
        Self::Priority,
        Self::Time,
        Self::Due,
    ];

    /// Columns shown when `tui.table.columns` is omitted, in display order.
    pub const DEFAULT: [Self; 8] = [
        Self::Ref,
        Self::Title,
        Self::Labels,
        Self::Metadata,
        Self::Project,
        Self::Status,
        Self::Priority,
        Self::Time,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Ref => "ref",
            Self::Title => "title",
            Self::Labels => "labels",
            Self::Metadata => "metadata",
            Self::Project => "project",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::Time => "time",
            Self::Due => "due",
        }
    }
}

pub(super) fn default_table_columns() -> Vec<TableColumn> {
    TableColumn::DEFAULT.to_vec()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidebarConfig {
    #[serde(default = "default_sidebar_views")]
    pub views: Vec<SidebarView>,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            views: default_sidebar_views(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarView {
    Queue,
    Ready,
    Blocked,
    Overdue,
    All,
    Open,
    Inbox,
    Active,
    Backlog,
    Todo,
    Upcoming,
    Done,
    Conflicts,
    Epics,
    Recurring,
    RecentActions,
    Search,
}

fn default_sidebar_views() -> Vec<SidebarView> {
    vec![
        SidebarView::Queue,
        SidebarView::Ready,
        SidebarView::Blocked,
        SidebarView::Overdue,
        SidebarView::All,
        SidebarView::Open,
        SidebarView::Inbox,
        SidebarView::Active,
        SidebarView::Backlog,
        SidebarView::Todo,
        SidebarView::Upcoming,
        SidebarView::Done,
        SidebarView::Conflicts,
        SidebarView::Epics,
        SidebarView::Recurring,
        SidebarView::RecentActions,
        SidebarView::Search,
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskColumnConfig {
    pub name: String,
    pub statuses: Vec<String>,
}

impl TaskColumnConfig {
    fn new(name: &str, statuses: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            statuses: statuses.iter().map(|status| status.to_string()).collect(),
        }
    }
}

pub(super) fn default_task_columns() -> Vec<TaskColumnConfig> {
    vec![
        TaskColumnConfig::new("Inbox", &["inbox"]),
        TaskColumnConfig::new("Backlog", &["backlog"]),
        TaskColumnConfig::new("Todo", &["todo"]),
        TaskColumnConfig::new("Active", &["active"]),
        TaskColumnConfig::new("Done", &["done", "canceled"]),
    ]
}

pub(super) fn validate(config: &TuiConfig) -> Result<()> {
    if config.table.columns.is_empty() {
        bail!("tui.table.columns must include at least one column");
    }
    let mut table_columns = BTreeSet::new();
    for column in &config.table.columns {
        if !table_columns.insert(*column) {
            bail!(
                "tui.table.columns contains duplicate column {}",
                column.name()
            );
        }
    }

    let mut sidebar_views = BTreeSet::new();
    for view in &config.sidebar.views {
        if !sidebar_views.insert(view) {
            bail!("tui.sidebar.views contains duplicate view {view:?}");
        }
    }

    if config.columns.is_empty() {
        bail!("column view requires at least one column");
    }
    let valid = crate::choices::STATUSES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut assigned = BTreeSet::new();
    for column in &config.columns {
        if column.name.trim().is_empty() {
            bail!("column name must not be blank");
        }
        if column.statuses.is_empty() {
            bail!("column {} must include at least one status", column.name);
        }
        for status in &column.statuses {
            if !valid.contains(status.as_str()) {
                bail!("unknown column status {status}");
            }
            if !assigned.insert(status.as_str()) {
                bail!("duplicate column status {status}");
            }
        }
    }
    let missing = valid.difference(&assigned).copied().collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!("missing column statuses {}", missing.join(","));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::AppConfig;
    use super::super::test_support::load_config;
    use super::*;

    #[test]
    fn default_columns_cover_every_status_once() {
        let config = AppConfig::default();

        config.validate().unwrap();
        assert_eq!(
            config
                .tui
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["Inbox", "Backlog", "Todo", "Active", "Done"]
        );
        assert_eq!(load_config("{}\n").unwrap().tui.columns, config.tui.columns);
    }

    #[test]
    fn omitted_sidebar_views_load_the_default() {
        let expected = AppConfig::default().tui.sidebar.views;

        assert_eq!(load_config("{}\n").unwrap().tui.sidebar.views, expected);
        assert_eq!(
            load_config("tui:\n  sidebar: {}\n")
                .unwrap()
                .tui
                .sidebar
                .views,
            expected
        );
    }

    #[test]
    fn sidebar_views_load_in_configured_order_and_support_empty_lists() {
        let configured =
            load_config("tui:\n  sidebar:\n    views: [search, recent_actions, queue]\n").unwrap();
        assert_eq!(
            configured.tui.sidebar.views,
            [
                SidebarView::Search,
                SidebarView::RecentActions,
                SidebarView::Queue,
            ]
        );

        let empty = load_config("tui:\n  sidebar:\n    views: []\n").unwrap();
        assert!(empty.tui.sidebar.views.is_empty());
    }

    #[test]
    fn sidebar_views_reject_duplicates() {
        let error = load_config("tui:\n  sidebar:\n    views: [queue, done, queue]\n").unwrap_err();

        assert!(format!("{error:#}").contains("tui.sidebar.views contains duplicate view Queue"));
    }

    #[test]
    fn sidebar_views_reject_unknown_names() {
        let error = load_config("tui:\n  sidebar:\n    views: [queue, someday]\n").unwrap_err();

        assert!(format!("{error:#}").contains("unknown variant `someday`"));
    }

    #[test]
    fn table_columns_default_and_round_trip() {
        for yaml in ["{}", "tui: {}", "tui:\n  table: {}"] {
            assert_eq!(
                load_config(yaml).unwrap().tui.table.columns,
                TableColumn::DEFAULT
            );
        }
        let config = load_config("tui:\n  table:\n    columns: [status, priority, ref]").unwrap();
        assert_eq!(
            config.tui.table.columns,
            [TableColumn::Status, TableColumn::Priority, TableColumn::Ref]
        );
        let text = serde_saphyr::to_string(&config).unwrap();
        assert_eq!(
            load_config(&text).unwrap().tui.table.columns,
            config.tui.table.columns
        );
        assert_eq!(config.tui.columns, default_task_columns());
    }

    #[test]
    fn compact_status_defaults_off_and_loads_under_the_table_settings() {
        for yaml in ["{}", "tui: {}", "tui:\n  table: {}"] {
            assert!(!load_config(yaml).unwrap().tui.table.compact_status);
        }
        assert!(!TaskTableConfig::default().compact_status);

        let config = load_config("tui:\n  table:\n    compact_status: true\n").unwrap();
        assert!(config.tui.table.compact_status);
        assert_eq!(config.tui.table.columns, default_table_columns());

        let text = serde_saphyr::to_string(&config).unwrap();
        assert!(load_config(&text).unwrap().tui.table.compact_status);
    }

    #[test]
    fn appearance_defaults_to_auto_and_loads_explicit_themes() {
        for yaml in ["{}", "tui: {}"] {
            assert_eq!(
                load_config(yaml).unwrap().tui.appearance,
                TuiAppearance::Auto
            );
        }

        let config = load_config("tui:\n  appearance: light\n").unwrap();
        assert_eq!(config.tui.appearance, TuiAppearance::Light);
        let text = serde_saphyr::to_string(&config).unwrap();
        assert_eq!(
            load_config(&text).unwrap().tui.appearance,
            TuiAppearance::Light
        );

        let config = load_config("tui:\n  appearance: terminal\n").unwrap();
        assert_eq!(config.tui.appearance, TuiAppearance::Terminal);
        let text = serde_saphyr::to_string(&config).unwrap();
        assert_eq!(
            load_config(&text).unwrap().tui.appearance,
            TuiAppearance::Terminal
        );
        assert!(load_config("tui:\n  appearance: sepia\n").is_err());
    }

    #[test]
    fn due_column_is_opt_in_and_independent_of_the_time_column() {
        assert!(!default_table_columns().contains(&TableColumn::Due));

        let config = load_config("tui:\n  table:\n    columns: [title, due]").unwrap();
        assert_eq!(
            config.tui.table.columns,
            [TableColumn::Title, TableColumn::Due]
        );
        assert_eq!(TableColumn::Due.name(), "due");

        let text = serde_saphyr::to_string(&config).unwrap();
        assert_eq!(
            load_config(&text).unwrap().tui.table.columns,
            config.tui.table.columns
        );

        let with_time = load_config("tui:\n  table:\n    columns: [title, due, time]").unwrap();
        assert_eq!(
            with_time.tui.table.columns,
            [TableColumn::Title, TableColumn::Due, TableColumn::Time]
        );
    }

    #[test]
    fn table_columns_reject_empty_duplicate_and_unknown_columns() {
        for (columns, expected) in [
            ("[]", "tui.table.columns must include at least one column"),
            (
                "[ref, title, labels, metadata, project, status, priority, ref]",
                "tui.table.columns contains duplicate column ref",
            ),
            (
                "[title, due, due]",
                "tui.table.columns contains duplicate column due",
            ),
            ("[robot]", "unknown variant `robot`"),
        ] {
            let error =
                load_config(&format!("tui:\n  table:\n    columns: {columns}")).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn table_columns_reject_legacy_column_order_instead_of_ignoring_it() {
        let error = load_config(
                "tui:\n  table:\n    column_order: [ref, title, labels, metadata, project, status, priority, time]",
            )
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("unknown field `column_order`"),
            "{message}"
        );
        assert!(message.contains("columns"), "{message}");
    }

    #[test]
    fn custom_columns_load_in_configured_order() {
        let config = load_config(
                "tui:\n  columns:\n    - name: Work\n      statuses: [active, todo]\n    - name: Later\n      statuses: [inbox, backlog]\n    - name: Closed\n      statuses: [done, canceled]\n",
            )
            .unwrap();

        assert_eq!(
            config.tui.columns,
            [
                TaskColumnConfig::new("Work", &["active", "todo"]),
                TaskColumnConfig::new("Later", &["inbox", "backlog"]),
                TaskColumnConfig::new("Closed", &["done", "canceled"]),
            ]
        );
    }

    #[test]
    fn column_config_rejects_missing_statuses() {
        let error =
            load_config("tui:\n  columns:\n    - name: Current\n      statuses: [active, todo]\n")
                .unwrap_err();

        assert!(format!("{error:#}").contains("missing column statuses"));
    }

    #[test]
    fn column_config_rejects_duplicate_statuses() {
        let error = load_config(
                "tui:\n  columns:\n    - name: One\n      statuses: [inbox, backlog, todo, active, done, canceled]\n    - name: Two\n      statuses: [active]\n",
            )
            .unwrap_err();

        assert!(format!("{error:#}").contains("duplicate column status active"));
    }

    #[test]
    fn column_config_rejects_unknown_statuses() {
        let error = load_config(
                "tui:\n  columns:\n    - name: One\n      statuses: [inbox, backlog, todo, active, done, canceled, parked]\n",
            )
            .unwrap_err();

        assert!(format!("{error:#}").contains("unknown column status parked"));
    }

    #[test]
    fn column_config_rejects_empty_columns_and_names() {
        let empty = load_config("tui:\n  columns: []\n").unwrap_err();
        assert!(format!("{empty:#}").contains("requires at least one column"));

        let unnamed = load_config(
                "tui:\n  columns:\n    - name: '  '\n      statuses: [inbox, backlog, todo, active, done, canceled]\n",
            )
            .unwrap_err();
        assert!(format!("{unnamed:#}").contains("column name must not be blank"));

        let no_statuses =
            load_config("tui:\n  columns:\n    - name: Empty\n      statuses: []\n").unwrap_err();
        assert!(format!("{no_statuses:#}").contains("must include at least one status"));
    }
}
