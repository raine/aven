use super::{SidebarEntry, SidebarEntryTarget, TaskScopeTarget, TaskView, TuiStore};

impl TuiStore {
    pub(super) fn rebuild_sidebar(&mut self) {
        let mut entries = vec![
            SidebarEntry {
                label: "Views".to_string(),
                count: 0,
                target: None,
                section: true,
            },
            view_entry("Queue", self.counts.open, TaskView::Queue),
            view_entry("Open", self.counts.open, TaskView::Open),
            view_entry("Inbox", self.counts.inbox, TaskView::Inbox),
            view_entry("Active", self.counts.active, TaskView::Active),
            view_entry("Backlog", self.counts.backlog, TaskView::Backlog),
            view_entry("Todo", self.counts.todo, TaskView::Todo),
            view_entry("Done", self.counts.done, TaskView::Done),
            view_entry("Conflicts", self.counts.conflicts, TaskView::Conflicts),
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

fn view_entry(label: &str, count: i64, view: TaskView) -> SidebarEntry {
    SidebarEntry {
        label: label.to_string(),
        count,
        target: Some(SidebarEntryTarget::View(view)),
        section: false,
    }
}
