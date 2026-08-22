use super::{SidebarEntry, SidebarEntryTarget, TaskQuery, TaskScopeTarget, TuiStore};

impl TuiStore {
    pub(super) fn rebuild_sidebar(&mut self) {
        let mut entries = vec![
            SidebarEntry {
                label: "Views".to_string(),
                count: 0,
                target: None,
                section: true,
            },
            view_entry("Queue", self.counts.open, TaskQuery::Queue),
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
                label: "Scope".to_string(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: "Workspace".to_string(),
                count: self.workspace_open_count(),
                target: Some(SidebarEntryTarget::Scope(TaskScopeTarget::Workspace)),
                section: false,
            },
            SidebarEntry {
                label: String::new(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: "Projects".to_string(),
                count: 0,
                target: None,
                section: true,
            },
        ];
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
