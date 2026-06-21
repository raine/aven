use super::{SidebarEntry, SidebarTarget, TuiStore};

impl TuiStore {
    pub(super) fn rebuild_sidebar(&mut self) {
        let mut entries = vec![
            SidebarEntry {
                label: "Smart Views".to_string(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: "Queue".to_string(),
                count: self.counts.all,
                target: Some(SidebarTarget::All),
                section: false,
            },
            SidebarEntry {
                label: "Inbox".to_string(),
                count: self.counts.inbox,
                target: Some(SidebarTarget::Inbox),
                section: false,
            },
            SidebarEntry {
                label: "Active".to_string(),
                count: self.counts.active,
                target: Some(SidebarTarget::Active),
                section: false,
            },
            SidebarEntry {
                label: "Backlog".to_string(),
                count: self.counts.backlog,
                target: Some(SidebarTarget::Backlog),
                section: false,
            },
            SidebarEntry {
                label: "Todo".to_string(),
                count: self.counts.todo,
                target: Some(SidebarTarget::Todo),
                section: false,
            },
            SidebarEntry {
                label: "Done".to_string(),
                count: self.counts.done,
                target: Some(SidebarTarget::Done),
                section: false,
            },
            SidebarEntry {
                label: "Conflicts".to_string(),
                count: self.counts.conflicts,
                target: Some(SidebarTarget::Conflicts),
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
            target: Some(SidebarTarget::Project(project.key.clone())),
            section: false,
        }));
        self.sidebar_entries = entries;
    }
}
