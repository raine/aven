use super::{
    SidebarEntry, SidebarEntryTarget, SidebarSection, TaskQuery, TaskScopeTarget, TuiProjection,
    TuiStore,
};
use crate::config::AppConfig;

impl TuiStore {
    pub(crate) async fn collapsed_sidebar_sections(
        &self,
    ) -> anyhow::Result<std::collections::BTreeSet<SidebarSection>> {
        self.database.collapsed_sidebar_sections().await
    }

    pub(crate) async fn set_sidebar_section_collapsed(
        &self,
        section: SidebarSection,
        collapsed: bool,
    ) -> anyhow::Result<()> {
        self.database
            .set_sidebar_section_collapsed(section, collapsed)
            .await
    }
}

impl TuiProjection {
    pub(super) fn rebuild_sidebar(&mut self, app_config: &AppConfig) {
        let mut entries = vec![
            SidebarEntry {
                label: SidebarSection::Views.label().to_string(),
                count: 0,
                target: Some(SidebarEntryTarget::Section(SidebarSection::Views)),
                section: true,
            },
            view_entry("Queue", self.counts.open, TaskQuery::Queue),
            view_entry("Ready", self.counts.ready, TaskQuery::Ready),
            view_entry("Blocked", self.counts.blocked, TaskQuery::Blocked),
            view_entry("Overdue", self.counts.overdue, TaskQuery::Overdue),
            view_entry("All", self.counts.open + self.counts.done, TaskQuery::All),
            view_entry("Open", self.counts.open, TaskQuery::Open),
            view_entry("Inbox", self.counts.inbox, TaskQuery::Inbox),
            view_entry("Active", self.counts.active, TaskQuery::Active),
            view_entry("Backlog", self.counts.backlog, TaskQuery::Backlog),
            view_entry("Todo", self.counts.todo, TaskQuery::Todo),
            view_entry("Upcoming", self.counts.upcoming, TaskQuery::Upcoming),
            view_entry("Done", self.counts.done, TaskQuery::Done),
            view_entry("Conflicts", self.counts.conflicts, TaskQuery::Conflicts),
            view_entry("Epics", self.counts.epics, TaskQuery::Epics),
            view_entry(
                "Recurring Tasks",
                self.counts.recurring,
                TaskQuery::Recurring,
            ),
            view_entry(
                "Recent actions",
                self.recent_actions.len() as i64,
                TaskQuery::RecentActions,
            ),
            view_entry(
                "Search",
                self.view_state
                    .projection_origin
                    .match_count()
                    .unwrap_or_default() as i64,
                TaskQuery::Search,
            ),
            SidebarEntry {
                label: String::new(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: SidebarSection::Workspaces.label().to_string(),
                count: 0,
                target: Some(SidebarEntryTarget::Section(SidebarSection::Workspaces)),
                section: true,
            },
            SidebarEntry {
                label: String::new(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: SidebarSection::Projects.label().to_string(),
                count: 0,
                target: Some(SidebarEntryTarget::Section(SidebarSection::Projects)),
                section: true,
            },
        ];
        let views: Vec<_> = app_config
            .tui
            .sidebar
            .views
            .iter()
            .map(|view| {
                let query = sidebar_query(*view);
                entries
                    .iter()
                    .find(|entry| entry.target == Some(SidebarEntryTarget::View(query)))
                    .expect("every configured sidebar view has an entry")
                    .clone()
            })
            .collect();
        entries.retain(|entry| !matches!(entry.target, Some(SidebarEntryTarget::View(_))));
        entries.splice(1..1, views);
        let workspace_index = entries
            .iter()
            .position(|entry| {
                entry.target == Some(SidebarEntryTarget::Section(SidebarSection::Workspaces))
            })
            .expect("workspace section exists")
            + 1;
        entries.splice(
            workspace_index..workspace_index,
            self.workspaces.iter().map(|workspace| SidebarEntry {
                label: workspace.key.clone(),
                count: if workspace.id == self.active_workspace.id {
                    self.workspace_open_count()
                } else {
                    self.workspace_open_counts
                        .get(&workspace.id)
                        .copied()
                        .unwrap_or_default()
                },
                target: Some(SidebarEntryTarget::Workspace(workspace.key.clone())),
                section: false,
            }),
        );
        entries.extend(self.projects.iter().map(|project| SidebarEntry {
            label: if project.inbox_count > 0 {
                format!("{} {}*", project.prefix, project.name)
            } else {
                format!("{} {}", project.prefix, project.name)
            },
            count: project.open_count,
            target: Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                project.key.clone(),
            ))),
            section: false,
        }));
        self.sidebar_entries = entries;
    }

    fn workspace_open_count(&self) -> i64 {
        self.projects.iter().map(|project| project.open_count).sum()
    }
}

fn view_entry(label: &str, count: i64, view: TaskQuery) -> SidebarEntry {
    SidebarEntry {
        label: label.to_string(),
        count,
        target: Some(SidebarEntryTarget::View(view)),
        section: false,
    }
}

fn sidebar_query(view: crate::config::SidebarView) -> TaskQuery {
    use crate::config::SidebarView;
    match view {
        SidebarView::Queue => TaskQuery::Queue,
        SidebarView::Ready => TaskQuery::Ready,
        SidebarView::Blocked => TaskQuery::Blocked,
        SidebarView::Overdue => TaskQuery::Overdue,
        SidebarView::All => TaskQuery::All,
        SidebarView::Open => TaskQuery::Open,
        SidebarView::Inbox => TaskQuery::Inbox,
        SidebarView::Active => TaskQuery::Active,
        SidebarView::Backlog => TaskQuery::Backlog,
        SidebarView::Todo => TaskQuery::Todo,
        SidebarView::Upcoming => TaskQuery::Upcoming,
        SidebarView::Done => TaskQuery::Done,
        SidebarView::Conflicts => TaskQuery::Conflicts,
        SidebarView::Epics => TaskQuery::Epics,
        SidebarView::Recurring => TaskQuery::Recurring,
        SidebarView::RecentActions => TaskQuery::RecentActions,
        SidebarView::Search => TaskQuery::Search,
    }
}
