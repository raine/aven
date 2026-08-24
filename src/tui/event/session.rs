use crate::ids::{TaskId, WorkspaceId};
use crate::tui::app::DetailSection;
use crate::tui::store::{TaskQuery, TaskScopeTarget};
use aven_core::recurrence::RecurrenceSeriesId;

use super::Action;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandWorkspaceSnapshot {
    pub(crate) id: WorkspaceId,
    pub(crate) key: String,
    pub(crate) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarCommandTarget {
    View(TaskQuery),
    Project(String),
    Workspace,
}

impl SidebarCommandTarget {
    pub(crate) fn from_scope(target: &TaskScopeTarget) -> Self {
        match target {
            TaskScopeTarget::Workspace => Self::Workspace,
            TaskScopeTarget::Project(project) => Self::Project(project.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DetailCommandFocus {
    Relationship {
        section: DetailSection,
        task_id: TaskId,
    },
    Note,
    Attachment {
        attachment_id: String,
        bytes_present: bool,
    },
    Disclosure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandSurfaceSnapshot {
    List {
        primary_task_id: Option<TaskId>,
        marked_task_ids: Vec<TaskId>,
        visible_task_ids: Vec<TaskId>,
        focused_sidebar: Option<SidebarCommandTarget>,
        is_empty: bool,
        empty_preferred_action: Option<Action>,
    },
    Detail {
        parent_task_id: TaskId,
        marked_task_ids: Vec<TaskId>,
        focus: Option<DetailCommandFocus>,
        scroll: u16,
    },
    RecurrenceList {
        focused_sidebar: Option<SidebarCommandTarget>,
        is_empty: bool,
        empty_preferred_action: Option<Action>,
    },
    AddTaskOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandSessionSnapshot {
    pub(crate) workspace: CommandWorkspaceSnapshot,
    pub(crate) surface: CommandSurfaceSnapshot,
    pub(crate) recurrence_series_id: Option<RecurrenceSeriesId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandSituation {
    ParentDetail,
    Relationship { section: DetailSection },
    Note,
    Attachment,
    Disclosure,
    MarkedSelection { count: usize },
    SidebarProject { project: String },
    SidebarView { view: TaskQuery },
    SidebarWorkspace,
    EmptyView { preferred_action: Option<Action> },
    SelectedTask,
    RecurringRow,
    AddTaskOnly,
    Neutral,
}

impl CommandSituation {
    pub(crate) const fn routing_domain(&self) -> super::RoutingDomain {
        use super::RoutingDomain;
        match self {
            Self::ParentDetail => RoutingDomain::DetailParent,
            Self::Relationship { .. } => RoutingDomain::DetailRelated,
            Self::Note | Self::Disclosure => RoutingDomain::DetailPassive,
            Self::Attachment => RoutingDomain::DetailAttachment,
            _ => RoutingDomain::Normal,
        }
    }
}

impl CommandSessionSnapshot {
    pub(crate) fn routing_domain(&self) -> super::RoutingDomain {
        self.situation().routing_domain()
    }

    pub(crate) fn situation(&self) -> CommandSituation {
        match &self.surface {
            CommandSurfaceSnapshot::Detail { focus, .. } => match focus {
                None => CommandSituation::ParentDetail,
                Some(DetailCommandFocus::Relationship { section, .. }) => {
                    CommandSituation::Relationship { section: *section }
                }
                Some(DetailCommandFocus::Note) => CommandSituation::Note,
                Some(DetailCommandFocus::Attachment { .. }) => CommandSituation::Attachment,
                Some(DetailCommandFocus::Disclosure) => CommandSituation::Disclosure,
            },
            CommandSurfaceSnapshot::List {
                marked_task_ids,
                focused_sidebar,
                is_empty,
                empty_preferred_action,
                primary_task_id,
                ..
            } => list_situation(
                marked_task_ids.len(),
                primary_task_id.is_some(),
                focused_sidebar,
                *is_empty,
                *empty_preferred_action,
            ),
            CommandSurfaceSnapshot::RecurrenceList {
                focused_sidebar,
                is_empty,
                empty_preferred_action,
            } => recurrence_list_situation(
                self.recurrence_series_id.is_some(),
                focused_sidebar,
                *is_empty,
                *empty_preferred_action,
            ),
            CommandSurfaceSnapshot::AddTaskOnly => CommandSituation::AddTaskOnly,
        }
    }

    pub(crate) fn primary_task_id(&self) -> Option<&TaskId> {
        match &self.surface {
            CommandSurfaceSnapshot::List {
                primary_task_id, ..
            } => primary_task_id.as_ref(),
            CommandSurfaceSnapshot::Detail { parent_task_id, .. } => Some(parent_task_id),
            CommandSurfaceSnapshot::RecurrenceList { .. } | CommandSurfaceSnapshot::AddTaskOnly => {
                None
            }
        }
    }

    pub(crate) fn marked_task_ids(&self) -> &[TaskId] {
        match &self.surface {
            CommandSurfaceSnapshot::List {
                marked_task_ids, ..
            }
            | CommandSurfaceSnapshot::Detail {
                marked_task_ids, ..
            } => marked_task_ids,
            _ => &[],
        }
    }

    pub(crate) fn visible_task_ids(&self) -> &[TaskId] {
        match &self.surface {
            CommandSurfaceSnapshot::List {
                visible_task_ids, ..
            } => visible_task_ids,
            _ => &[],
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        match &self.surface {
            CommandSurfaceSnapshot::List { is_empty, .. }
            | CommandSurfaceSnapshot::RecurrenceList { is_empty, .. } => *is_empty,
            CommandSurfaceSnapshot::Detail { .. } => false,
            CommandSurfaceSnapshot::AddTaskOnly => true,
        }
    }

    pub(crate) fn detail_focus(&self) -> Option<&DetailCommandFocus> {
        match &self.surface {
            CommandSurfaceSnapshot::Detail { focus, .. } => focus.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn detail_scroll(&self) -> Option<u16> {
        match &self.surface {
            CommandSurfaceSnapshot::Detail { scroll, .. } => Some(*scroll),
            _ => None,
        }
    }

    pub(crate) fn sidebar_target(&self) -> Option<&SidebarCommandTarget> {
        match &self.surface {
            CommandSurfaceSnapshot::List {
                focused_sidebar, ..
            }
            | CommandSurfaceSnapshot::RecurrenceList {
                focused_sidebar, ..
            } => focused_sidebar.as_ref(),
            CommandSurfaceSnapshot::Detail { .. } | CommandSurfaceSnapshot::AddTaskOnly => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ResolvedCommandTarget {
    None,
    Marks(Vec<TaskId>),
    Tasks(crate::tui::task_selection::TaskSelection),
    Relationship {
        parent: TaskId,
        related: TaskId,
        section: DetailSection,
    },
    Sidebar(SidebarCommandTarget),
    SidebarProject(String),
    Attachment {
        owner: TaskId,
        attachment_id: String,
    },
    Recurrence(RecurrenceSeriesId),
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedCommand {
    pub(crate) action: Action,
    pub(crate) target: ResolvedCommandTarget,
    pub(crate) effect: super::SurfaceEffect,
}

pub(crate) fn list_situation(
    marked_count: usize,
    has_primary: bool,
    focused_sidebar: &Option<SidebarCommandTarget>,
    is_empty: bool,
    empty_preferred_action: Option<Action>,
) -> CommandSituation {
    if let Some(target) = focused_sidebar.as_ref() {
        return sidebar_situation(target);
    }
    if marked_count > 0 {
        CommandSituation::MarkedSelection {
            count: marked_count,
        }
    } else if has_primary {
        CommandSituation::SelectedTask
    } else if is_empty {
        CommandSituation::EmptyView {
            preferred_action: empty_preferred_action,
        }
    } else {
        CommandSituation::Neutral
    }
}

pub(crate) fn recurrence_list_situation(
    has_series: bool,
    focused_sidebar: &Option<SidebarCommandTarget>,
    is_empty: bool,
    empty_preferred_action: Option<Action>,
) -> CommandSituation {
    if let Some(target) = focused_sidebar.as_ref() {
        sidebar_situation(target)
    } else if has_series {
        CommandSituation::RecurringRow
    } else if is_empty {
        CommandSituation::EmptyView {
            preferred_action: empty_preferred_action,
        }
    } else {
        CommandSituation::Neutral
    }
}

fn sidebar_situation(target: &SidebarCommandTarget) -> CommandSituation {
    match target {
        SidebarCommandTarget::Project(project) => CommandSituation::SidebarProject {
            project: project.clone(),
        },
        SidebarCommandTarget::View(view) => CommandSituation::SidebarView { view: *view },
        SidebarCommandTarget::Workspace => CommandSituation::SidebarWorkspace,
    }
}
