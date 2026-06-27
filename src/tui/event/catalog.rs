use crossterm::event::KeyCode;

use crate::tui::store::{TaskOrder, TaskView};

use super::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandLifecycle {
    Implemented,
    Planned { reason: &'static str },
    Disabled { reason: &'static str },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct KeySequence {
    pub(crate) codes: &'static [KeyCode],
    pub(crate) label: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandSpec {
    pub(crate) name: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) description: &'static str,
    pub(crate) section: &'static str,
    pub(crate) keys: &'static [KeySequence],
    pub(crate) action: Action,
    pub(crate) lifecycle: CommandLifecycle,
}

pub(crate) const PROJECT_PATH_FLOW_REASON: &str = "requires a multi-step project/path picker flow";
pub(crate) const DUE_SORT_REASON: &str = "tasks do not have due dates";

impl CommandSpec {
    pub(crate) const fn implemented(
        name: &'static str,
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        action: Action,
    ) -> Self {
        Self::implemented_with_aliases(name, &[], description, section, keys, action)
    }

    pub(crate) const fn implemented_with_aliases(
        name: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        action: Action,
    ) -> Self {
        Self {
            name,
            aliases,
            description,
            section,
            keys,
            action,
            lifecycle: CommandLifecycle::Implemented,
        }
    }

    const fn planned(
        name: &'static str,
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        reason: &'static str,
    ) -> Self {
        Self {
            name,
            aliases: &[],
            description,
            section,
            keys,
            action: Action::Planned { name, reason },
            lifecycle: CommandLifecycle::Planned { reason },
        }
    }

    const fn disabled(
        name: &'static str,
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        reason: &'static str,
    ) -> Self {
        Self {
            name,
            aliases: &[],
            description,
            section,
            keys,
            action: Action::Disabled { name, reason },
            lifecycle: CommandLifecycle::Disabled { reason },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandContext {
    Normal,
    Detail,
}

impl CommandContext {
    pub(crate) const fn commands(self) -> &'static [CommandSpec] {
        match self {
            Self::Normal => COMMANDS,
            Self::Detail => DETAIL_COMMANDS,
        }
    }

    pub(crate) const fn sections(self) -> &'static [&'static str] {
        match self {
            Self::Normal => NORMAL_HELP_SECTIONS,
            Self::Detail => DETAIL_HELP_SECTIONS,
        }
    }
}

pub(crate) const NORMAL_HELP_SECTIONS: &[&str] = &[
    "General",
    "Navigation",
    "Tasks",
    "Projects",
    "Labels",
    "Views",
    "Filters",
    "Order",
    "Conflicts",
    "Config",
];

pub(crate) const DETAIL_HELP_SECTIONS: &[&str] = &["General", "Task detail", "Tasks"];

pub(crate) const COMMANDS: &[CommandSpec] = &[
    CommandSpec::implemented(
        "quit",
        "quit the TUI",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('q')],
            label: "q",
        }],
        Action::Quit,
    ),
    CommandSpec::implemented(
        "command",
        "open the command panel",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char(':')],
            label: ":",
        }],
        Action::BeginCommand,
    ),
    CommandSpec::implemented(
        "help",
        "toggle shortcut help",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('?')],
            label: "?",
        }],
        Action::ToggleHelp,
    ),
    CommandSpec::implemented(
        "refresh",
        "reload tasks",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('r')],
            label: "r",
        }],
        Action::Refresh,
    ),
    CommandSpec::implemented(
        "undo",
        "undo last TUI mutation",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('u')],
            label: "u",
        }],
        Action::Undo,
    ),
    CommandSpec::implemented(
        "search",
        "search title and description",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('/')],
            label: "/",
        }],
        Action::BeginSearch,
    ),
    CommandSpec::implemented(
        "move-down",
        "move selection down",
        "Navigation",
        &[
            KeySequence {
                codes: &[KeyCode::Char('j')],
                label: "j",
            },
            KeySequence {
                codes: &[KeyCode::Down],
                label: "Down",
            },
        ],
        Action::MoveDown,
    ),
    CommandSpec::implemented(
        "move-up",
        "move selection up",
        "Navigation",
        &[
            KeySequence {
                codes: &[KeyCode::Char('k')],
                label: "k",
            },
            KeySequence {
                codes: &[KeyCode::Up],
                label: "Up",
            },
        ],
        Action::MoveUp,
    ),
    CommandSpec::implemented(
        "move-left",
        "move focus left",
        "Navigation",
        &[
            KeySequence {
                codes: &[KeyCode::Char('h')],
                label: "h",
            },
            KeySequence {
                codes: &[KeyCode::Left],
                label: "Left",
            },
        ],
        Action::MoveLeft,
    ),
    CommandSpec::implemented(
        "move-right",
        "move focus right",
        "Navigation",
        &[
            KeySequence {
                codes: &[KeyCode::Char('l')],
                label: "l",
            },
            KeySequence {
                codes: &[KeyCode::Right],
                label: "Right",
            },
        ],
        Action::MoveRight,
    ),
    CommandSpec::implemented(
        "previous-item",
        "select previous item in flow",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('[')],
            label: "[",
        }],
        Action::PreviousItem,
    ),
    CommandSpec::implemented(
        "next-item",
        "select next item in flow",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char(']')],
            label: "]",
        }],
        Action::NextItem,
    ),
    CommandSpec::implemented(
        "first",
        "jump to the first item",
        "Navigation",
        &[
            KeySequence {
                codes: &[KeyCode::Char('g'), KeyCode::Char('g')],
                label: "g g",
            },
            KeySequence {
                codes: &[KeyCode::Home],
                label: "Home",
            },
        ],
        Action::First,
    ),
    CommandSpec::implemented(
        "last",
        "jump to the last item",
        "Navigation",
        &[
            KeySequence {
                codes: &[KeyCode::Char('G')],
                label: "G",
            },
            KeySequence {
                codes: &[KeyCode::End],
                label: "End",
            },
        ],
        Action::Last,
    ),
    CommandSpec::implemented(
        "focus",
        "switch between views and tasks",
        "Navigation",
        &[
            KeySequence {
                codes: &[KeyCode::Tab],
                label: "Tab",
            },
            KeySequence {
                codes: &[KeyCode::BackTab],
                label: "Shift+Tab",
            },
        ],
        Action::ToggleFocus,
    ),
    CommandSpec::implemented(
        "detail",
        "select a view or toggle task detail",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Enter],
            label: "Enter",
        }],
        Action::ToggleDetail,
    ),
    CommandSpec::implemented(
        "delete",
        "confirm deleting selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('D')],
            label: "t D",
        }],
        Action::Delete,
    ),
    CommandSpec::implemented(
        "status-picker",
        "open task status picker",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('s')],
                label: "t s",
            },
            KeySequence {
                codes: &[KeyCode::Char('s')],
                label: "s",
            },
        ],
        Action::BeginStatusPicker,
    ),
    CommandSpec::implemented(
        "restore",
        "restore selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('R')],
            label: "t R",
        }],
        Action::Restore,
    ),
    CommandSpec::implemented(
        "status-inbox",
        "set status to inbox",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('i')],
            label: "t i",
        }],
        Action::SetStatus("inbox"),
    ),
    CommandSpec::implemented(
        "status-backlog",
        "set status to backlog",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('b')],
            label: "t b",
        }],
        Action::SetStatus("backlog"),
    ),
    CommandSpec::implemented_with_aliases(
        "status-todo",
        &["todo"],
        "set status to todo",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('t')],
            label: "t t",
        }],
        Action::SetStatus("todo"),
    ),
    CommandSpec::implemented(
        "status-active",
        "set status to active",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('a')],
            label: "t a",
        }],
        Action::SetStatus("active"),
    ),
    CommandSpec::implemented(
        "status-done",
        "set status to done",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('d')],
                label: "t d",
            },
            KeySequence {
                codes: &[KeyCode::Char('d')],
                label: "d",
            },
        ],
        Action::SetStatus("done"),
    ),
    CommandSpec::implemented(
        "status-canceled",
        "set status to canceled",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('x')],
                label: "t x",
            },
            KeySequence {
                codes: &[KeyCode::Char('x')],
                label: "x",
            },
        ],
        Action::SetStatus("canceled"),
    ),
    // Views
    CommandSpec::implemented(
        "view-queue",
        "show queue view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('q')],
            label: "v q",
        }],
        Action::ShowView(TaskView::Queue),
    ),
    CommandSpec::implemented(
        "view-open",
        "show open task view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('o')],
            label: "v o",
        }],
        Action::ShowView(TaskView::Open),
    ),
    CommandSpec::implemented(
        "view-inbox",
        "show inbox view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('i')],
            label: "v i",
        }],
        Action::ShowView(TaskView::Inbox),
    ),
    CommandSpec::implemented(
        "view-backlog",
        "show backlog view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('b')],
            label: "v b",
        }],
        Action::ShowView(TaskView::Backlog),
    ),
    CommandSpec::implemented(
        "view-todo",
        "show todo view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('t')],
            label: "v t",
        }],
        Action::ShowView(TaskView::Todo),
    ),
    CommandSpec::implemented(
        "view-active",
        "show active view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('a')],
            label: "v a",
        }],
        Action::ShowView(TaskView::Active),
    ),
    CommandSpec::implemented(
        "view-done",
        "show done view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('d')],
            label: "v d",
        }],
        Action::ShowView(TaskView::Done),
    ),
    CommandSpec::implemented(
        "view-conflicts",
        "show conflicts view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('c')],
            label: "v c",
        }],
        Action::ShowView(TaskView::Conflicts),
    ),
    CommandSpec::implemented(
        "scope-all",
        "show all projects in current workspace",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('a')],
            label: "g a",
        }],
        Action::ShowWorkspaceScope,
    ),
    CommandSpec::implemented(
        "scope-project",
        "scope to a project",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('p')],
            label: "g p",
        }],
        Action::BeginScopeProject,
    ),
    CommandSpec::implemented(
        "workspace-switch",
        "switch active workspace",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('w')],
            label: "g w",
        }],
        Action::BeginSwitchWorkspace,
    ),
    // Add/Create
    CommandSpec::implemented(
        "add-task",
        "add a new task",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('n')],
                label: "t n",
            },
            KeySequence {
                codes: &[KeyCode::Char('a')],
                label: "a",
            },
        ],
        Action::BeginAddTask,
    ),
    CommandSpec::implemented(
        "add-note",
        "add a note to selected task",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('N')],
                label: "t N",
            },
            KeySequence {
                codes: &[KeyCode::Char('n')],
                label: "n",
            },
        ],
        Action::BeginAddNote,
    ),
    // Metadata
    CommandSpec::implemented(
        "add-project",
        "create a new project",
        "Projects",
        &[KeySequence {
            codes: &[KeyCode::Char('p'), KeyCode::Char('n')],
            label: "p n",
        }],
        Action::BeginAddProject,
    ),
    CommandSpec::implemented(
        "add-label",
        "create a new label",
        "Labels",
        &[KeySequence {
            codes: &[KeyCode::Char('L'), KeyCode::Char('n')],
            label: "L n",
        }],
        Action::BeginAddLabel,
    ),
    CommandSpec::implemented(
        "rename-project",
        "rename a project and display prefix",
        "Projects",
        &[KeySequence {
            codes: &[KeyCode::Char('p'), KeyCode::Char('r')],
            label: "p r",
        }],
        Action::BeginRenameProject,
    ),
    CommandSpec::implemented(
        "delete-project",
        "delete a project",
        "Projects",
        &[KeySequence {
            codes: &[KeyCode::Char('p'), KeyCode::Char('D')],
            label: "p D",
        }],
        Action::BeginDeleteProject,
    ),
    CommandSpec::planned(
        "add-project-path",
        "add a path to a project",
        "Projects",
        &[KeySequence {
            codes: &[KeyCode::Char('p'), KeyCode::Char('a')],
            label: "p a",
        }],
        PROJECT_PATH_FLOW_REASON,
    ),
    CommandSpec::planned(
        "remove-project-path",
        "remove a path from a project",
        "Projects",
        &[KeySequence {
            codes: &[KeyCode::Char('p'), KeyCode::Char('x')],
            label: "p x",
        }],
        PROJECT_PATH_FLOW_REASON,
    ),
    // Edit
    CommandSpec::implemented(
        "edit-title",
        "edit selected task title",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('t')],
            label: "t e t",
        }],
        Action::BeginEditTitle,
    ),
    CommandSpec::implemented(
        "edit-description",
        "edit selected task description",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('d')],
            label: "t e d",
        }],
        Action::BeginEditDescription,
    ),
    CommandSpec::implemented(
        "edit-project",
        "edit selected task project",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('p')],
            label: "t e p",
        }],
        Action::BeginEditProject,
    ),
    CommandSpec::implemented(
        "edit-priority",
        "edit selected task priority",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('p')],
            label: "t p",
        }],
        Action::BeginEditPriority,
    ),
    CommandSpec::implemented(
        "edit-labels",
        "edit selected task labels",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('l')],
            label: "t e l",
        }],
        Action::BeginEditLabels,
    ),
    CommandSpec::implemented(
        "copy-ref",
        "copy selected task display ref",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('y')],
            label: "t y",
        }],
        Action::CopyShortRef,
    ),
    CommandSpec::implemented(
        "copy-id",
        "copy selected task id",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('Y')],
            label: "t Y",
        }],
        Action::CopyDurableRef,
    ),
    // Priority
    CommandSpec::implemented(
        "priority-none",
        "set priority to none",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('0')],
            label: "t 0",
        }],
        Action::SetPriority("none"),
    ),
    CommandSpec::implemented(
        "priority-low",
        "set priority to low",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('l')],
            label: "t l",
        }],
        Action::SetPriority("low"),
    ),
    CommandSpec::implemented(
        "priority-medium",
        "set priority to medium",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('m')],
            label: "t m",
        }],
        Action::SetPriority("medium"),
    ),
    CommandSpec::implemented(
        "priority-high",
        "set priority to high",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('h')],
            label: "t h",
        }],
        Action::SetPriority("high"),
    ),
    CommandSpec::implemented(
        "priority-urgent",
        "set priority to urgent",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('u')],
            label: "t u",
        }],
        Action::SetPriority("urgent"),
    ),
    // Filters
    CommandSpec::implemented(
        "filter-label",
        "filter by label",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('l')],
            label: "f l",
        }],
        Action::BeginFilterLabel,
    ),
    CommandSpec::implemented(
        "filter-priority",
        "filter by priority",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('p')],
            label: "f p",
        }],
        Action::BeginFilterPriority,
    ),
    CommandSpec::implemented(
        "filter-clear",
        "clear all filters",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('c')],
            label: "f c",
        }],
        Action::ClearFilters,
    ),
    CommandSpec::implemented(
        "filter-deleted",
        "filter deleted tasks",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('x')],
            label: "f x",
        }],
        Action::ToggleDeletedFilter,
    ),
    // Order
    CommandSpec::disabled(
        "order-due",
        "sort by due date",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('d')],
            label: "o d",
        }],
        DUE_SORT_REASON,
    ),
    CommandSpec::implemented(
        "order-created",
        "sort by created date",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('c')],
            label: "o c",
        }],
        Action::SetOrder(TaskOrder::Created),
    ),
    CommandSpec::implemented(
        "order-updated",
        "sort by updated date",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('u')],
            label: "o u",
        }],
        Action::SetOrder(TaskOrder::Updated),
    ),
    CommandSpec::implemented(
        "order-priority",
        "sort by priority",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('p')],
            label: "o p",
        }],
        Action::SetOrder(TaskOrder::Priority),
    ),
    CommandSpec::implemented(
        "order-project",
        "sort by project",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('j')],
            label: "o j",
        }],
        Action::SetOrder(TaskOrder::Project),
    ),
    CommandSpec::implemented(
        "order-title",
        "sort by title",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('t')],
            label: "o t",
        }],
        Action::SetOrder(TaskOrder::Title),
    ),
    CommandSpec::implemented(
        "order-reverse",
        "reverse sort direction",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('r')],
            label: "o r",
        }],
        Action::ReverseSort,
    ),
    // Conflict
    CommandSpec::implemented(
        "conflict-list",
        "list or filter conflicts",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('l')],
            label: "c l",
        }],
        Action::BeginConflictList,
    ),
    CommandSpec::implemented(
        "conflict-show",
        "show conflict details",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('s')],
            label: "c s",
        }],
        Action::ShowConflictDetails,
    ),
    CommandSpec::implemented(
        "conflict-next",
        "jump to next conflict",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('n')],
            label: "c n",
        }],
        Action::NextConflict,
    ),
    CommandSpec::implemented(
        "conflict-prev",
        "jump to previous conflict",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('p')],
            label: "c p",
        }],
        Action::PreviousConflict,
    ),
    CommandSpec::implemented(
        "conflict-use-local",
        "resolve with local value",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('a')],
            label: "c a",
        }],
        Action::AcceptConflictLocal,
    ),
    CommandSpec::implemented(
        "conflict-use-remote",
        "resolve with remote value",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('r')],
            label: "c r",
        }],
        Action::AcceptConflictRemote,
    ),
    CommandSpec::implemented(
        "conflict-manual-merge",
        "resolve with manual value",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('m')],
            label: "c m",
        }],
        Action::BeginManualConflictMerge,
    ),
    // Config
    CommandSpec::implemented(
        "config-status",
        "show sync and daemon status",
        "Config",
        &[KeySequence {
            codes: &[KeyCode::Char('C'), KeyCode::Char('s')],
            label: "C s",
        }],
        Action::ShowConfigStatus,
    ),
    CommandSpec::implemented(
        "config-show",
        "show configuration",
        "Config",
        &[KeySequence {
            codes: &[KeyCode::Char('C'), KeyCode::Char('c')],
            label: "C c",
        }],
        Action::ShowConfigInfo,
    ),
    CommandSpec::implemented(
        "config-paths",
        "show data paths",
        "Config",
        &[KeySequence {
            codes: &[KeyCode::Char('C'), KeyCode::Char('d')],
            label: "C d",
        }],
        Action::ShowConfigPaths,
    ),
    CommandSpec::implemented(
        "database-stats",
        "show database statistics",
        "Config",
        &[KeySequence {
            codes: &[KeyCode::Char('C'), KeyCode::Char('D')],
            label: "C D",
        }],
        Action::ShowDatabaseStats,
    ),
    CommandSpec::implemented(
        "config-init",
        "initialize configuration",
        "Config",
        &[KeySequence {
            codes: &[KeyCode::Char('C'), KeyCode::Char('i')],
            label: "C i",
        }],
        Action::BeginConfigInit,
    ),
];

pub(crate) const DETAIL_COMMANDS: &[CommandSpec] = &[
    CommandSpec::implemented(
        "detail-edit-title",
        "edit selected task title",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('t')],
            label: "t e t",
        }],
        Action::BeginEditTitle,
    ),
    CommandSpec::implemented(
        "detail-edit-description",
        "edit selected task description",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('d')],
            label: "t e d",
        }],
        Action::BeginEditDescription,
    ),
    CommandSpec::implemented(
        "detail-edit-project",
        "edit selected task project",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('p')],
            label: "t e p",
        }],
        Action::BeginEditProject,
    ),
    CommandSpec::implemented(
        "detail-edit-labels",
        "edit selected task labels",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('l')],
            label: "t e l",
        }],
        Action::BeginEditLabels,
    ),
    CommandSpec::implemented(
        "detail-add-note",
        "add a note to selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('N')],
            label: "t N",
        }],
        Action::BeginAddNote,
    ),
    CommandSpec::implemented(
        "detail-status-picker",
        "open task status picker",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('s')],
            label: "t s",
        }],
        Action::BeginStatusPicker,
    ),
    CommandSpec::implemented(
        "detail-status-done",
        "set status to done",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('d')],
            label: "t d",
        }],
        Action::SetStatus("done"),
    ),
    CommandSpec::implemented(
        "detail-edit-priority",
        "edit selected task priority",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('P')],
            label: "t P",
        }],
        Action::BeginEditPriority,
    ),
    CommandSpec::implemented(
        "detail-delete",
        "confirm deleting selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('D')],
            label: "t D",
        }],
        Action::Delete,
    ),
    CommandSpec::implemented(
        "detail-copy-ref",
        "copy selected task display ref",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('y')],
            label: "t y",
        }],
        Action::CopyShortRef,
    ),
    CommandSpec::implemented(
        "detail-copy-id",
        "copy selected task id",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('Y')],
            label: "t Y",
        }],
        Action::CopyDurableRef,
    ),
    CommandSpec::implemented(
        "detail-undo",
        "undo last TUI mutation",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('u')],
            label: "u",
        }],
        Action::Undo,
    ),
];

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandDomain {
    pub(crate) section: &'static str,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CommandDomain {
    pub(crate) fn commands(self) -> Vec<&'static CommandSpec> {
        COMMANDS
            .iter()
            .filter(|command| command.section == self.section)
            .collect()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const COMMAND_DOMAINS: &[CommandDomain] = &[
    CommandDomain { section: "General" },
    CommandDomain {
        section: "Navigation",
    },
    CommandDomain { section: "Tasks" },
    CommandDomain {
        section: "Projects",
    },
    CommandDomain { section: "Labels" },
    CommandDomain { section: "Views" },
    CommandDomain { section: "Filters" },
    CommandDomain { section: "Order" },
    CommandDomain {
        section: "Conflicts",
    },
    CommandDomain { section: "Config" },
];
