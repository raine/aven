use crossterm::event::KeyCode;

use crate::choices::{TaskPriority, TaskStatus};
use crate::tui::store::{TaskLayout, TaskOrder, TaskQuery};

use super::{Action, BulkSupport};

#[derive(Debug, Clone, Copy)]
pub(crate) struct KeySequence {
    pub(crate) codes: &'static [KeyCode],
    pub(crate) label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailFocusPolicy {
    Global,
    ParentTask,
    EpicChild,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandSpec {
    pub(crate) name: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) description: &'static str,
    pub(crate) section: &'static str,
    list_keys: &'static [KeySequence],
    detail_keys: &'static [KeySequence],
    pub(crate) detail_focus: DetailFocusPolicy,
    pub(crate) action: Action,
}

impl CommandSpec {
    pub(crate) const fn bulk_support(self) -> BulkSupport {
        self.action.bulk_support()
    }

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
            list_keys: keys,
            detail_keys: &[],
            detail_focus: DetailFocusPolicy::ParentTask,
            action,
        }
    }

    pub(crate) const fn implemented_in_detail(
        name: &'static str,
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        action: Action,
    ) -> Self {
        Self::implemented_with_aliases_in_detail(name, &[], description, section, keys, action)
    }

    pub(crate) const fn implemented_with_aliases_in_detail(
        name: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        action: Action,
    ) -> Self {
        Self::implemented_with_detail_bindings(
            name,
            aliases,
            description,
            section,
            keys,
            keys,
            DetailFocusPolicy::ParentTask,
            action,
        )
    }

    pub(crate) const fn implemented_global_in_detail(
        name: &'static str,
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        action: Action,
    ) -> Self {
        Self::implemented_with_detail_bindings(
            name,
            &[],
            description,
            section,
            keys,
            keys,
            DetailFocusPolicy::Global,
            action,
        )
    }

    pub(crate) const fn implemented_for_epic_child(
        name: &'static str,
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        action: Action,
    ) -> Self {
        Self::implemented_with_detail_bindings(
            name,
            &[],
            description,
            section,
            keys,
            keys,
            DetailFocusPolicy::EpicChild,
            action,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn implemented_with_detail_bindings(
        name: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        section: &'static str,
        list_keys: &'static [KeySequence],
        detail_keys: &'static [KeySequence],
        detail_focus: DetailFocusPolicy,
        action: Action,
    ) -> Self {
        Self {
            name,
            aliases,
            description,
            section,
            list_keys,
            detail_keys,
            detail_focus,
            action,
        }
    }

    pub(crate) const fn keys(self, context: CommandContext) -> &'static [KeySequence] {
        match context {
            CommandContext::Normal => self.list_keys,
            CommandContext::Detail => self.detail_keys,
        }
    }

    pub(crate) const fn is_available(self, context: CommandContext) -> bool {
        match context {
            CommandContext::Normal => true,
            CommandContext::Detail => !self.detail_keys.is_empty(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandContext {
    Normal,
    Detail,
}

impl CommandContext {
    pub(crate) fn commands(self) -> impl Iterator<Item = &'static CommandSpec> {
        COMMANDS
            .iter()
            .filter(move |command| command.is_available(self))
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
    "Workspaces",
    "Labels",
    "Views",
    "Filters",
    "Order",
    "Conflicts",
    "Config",
];

pub(crate) const DETAIL_HELP_SECTIONS: &[&str] =
    &["General", "Navigation", "Task detail", "Tasks", "Conflicts"];

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
    CommandSpec::implemented_global_in_detail(
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
        "welcome",
        "show the getting-started guide",
        "General",
        &[],
        Action::ShowWelcome,
    ),
    CommandSpec::implemented_global_in_detail(
        "refresh",
        "reload tasks",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('r')],
            label: "r",
        }],
        Action::Refresh,
    ),
    CommandSpec::implemented_global_in_detail(
        "sync",
        "sync with the remote server",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('S')],
            label: "S",
        }],
        Action::SyncNow,
    ),
    CommandSpec::implemented(
        "update",
        "check for and install an aven update",
        "General",
        &[],
        Action::BeginUpdate,
    ),
    CommandSpec::implemented(
        "changelog",
        "read aven release notes",
        "General",
        &[],
        Action::ShowChangelog,
    ),
    CommandSpec::implemented_for_epic_child(
        "undo",
        "undo last TUI mutation",
        "General",
        &[KeySequence {
            codes: &[KeyCode::Char('u')],
            label: "u",
        }],
        Action::Undo,
    ),
    CommandSpec::implemented_global_in_detail(
        "search",
        "search all tasks",
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
        "move focus right or toggle selected epic",
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
        "move-column-left",
        "move selected or marked tasks to the previous column",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('<')],
            label: "<",
        }],
        Action::MoveColumnLeft,
    ),
    CommandSpec::implemented(
        "move-column-right",
        "move selected or marked tasks to the next column",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('>')],
            label: ">",
        }],
        Action::MoveColumnRight,
    ),
    CommandSpec::implemented(
        "move-to-column",
        "move selected or marked tasks to a column",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('m')],
            label: "m",
        }],
        Action::BeginMoveToColumn,
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
                label: "S-Tab",
            },
        ],
        Action::ToggleFocus,
    ),
    CommandSpec::implemented_global_in_detail(
        "back",
        "return to the previous navigation state",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('[')],
            label: "g [",
        }],
        Action::GoBack,
    ),
    CommandSpec::implemented_global_in_detail(
        "forward",
        "return to the next navigation state",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char(']')],
            label: "g ]",
        }],
        Action::GoForward,
    ),
    CommandSpec::implemented_global_in_detail(
        "return-to-change",
        "select the task most recently changed",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('.')],
            label: "g .",
        }],
        Action::ReturnToLastChange,
    ),
    CommandSpec::implemented(
        "toggle-sidebar",
        "toggle the sidebar",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('s')],
            label: "g s",
        }],
        Action::ToggleSidebar,
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
        "toggle-columns-preview",
        "toggle the selected-task preview in columns view",
        "Navigation",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('d')],
            label: "g d",
        }],
        Action::ToggleColumnsPreview,
    ),
    CommandSpec::implemented_in_detail(
        "delete",
        "confirm deleting selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('D')],
            label: "t D",
        }],
        Action::Delete,
    ),
    CommandSpec::implemented_in_detail(
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
    CommandSpec::implemented_in_detail(
        "restore",
        "restore selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('R')],
            label: "t R",
        }],
        Action::Restore,
    ),
    CommandSpec::implemented_in_detail(
        "status-inbox",
        "set status to inbox",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('i')],
            label: "t i",
        }],
        Action::SetStatus(TaskStatus::Inbox),
    ),
    CommandSpec::implemented_in_detail(
        "status-backlog",
        "set status to backlog",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('b')],
            label: "t b",
        }],
        Action::SetStatus(TaskStatus::Backlog),
    ),
    CommandSpec::implemented_with_aliases_in_detail(
        "status-todo",
        &["todo"],
        "set status to todo",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('t')],
            label: "t t",
        }],
        Action::SetStatus(TaskStatus::Todo),
    ),
    CommandSpec::implemented_in_detail(
        "status-active",
        "set status to active",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('a')],
            label: "t a",
        }],
        Action::SetStatus(TaskStatus::Active),
    ),
    CommandSpec::implemented_in_detail(
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
        Action::SetStatus(TaskStatus::Done),
    ),
    CommandSpec::implemented_in_detail(
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
        Action::SetStatus(TaskStatus::Canceled),
    ),
    CommandSpec::implemented_in_detail(
        "recurrence-skip",
        "skip the current recurring occurrence",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('k')],
            label: "t r k",
        }],
        Action::SkipRecurrence,
    ),
    CommandSpec::implemented_in_detail(
        "recurrence-edit-template",
        "edit the recurring template for future occurrences",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('e')],
            label: "t r e",
        }],
        Action::BeginEditRecurrenceTemplate,
    ),
    CommandSpec::implemented_in_detail(
        "recurrence-pause",
        "pause the selected recurring series",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('p')],
            label: "t r p",
        }],
        Action::PauseRecurrence,
    ),
    CommandSpec::implemented_in_detail(
        "recurrence-resume",
        "resume the selected recurring series",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('r')],
            label: "t r r",
        }],
        Action::ResumeRecurrence,
    ),
    CommandSpec::implemented_in_detail(
        "recurrence-stop",
        "stop future occurrences after the current task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('s')],
            label: "t r s",
        }],
        Action::StopRecurrence,
    ),
    CommandSpec::implemented_in_detail(
        "recurrence-history",
        "show recurring series history",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('h')],
            label: "t r h",
        }],
        Action::ShowRecurrenceHistory,
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
        Action::ShowView(TaskQuery::Queue),
    ),
    CommandSpec::implemented(
        "layout-toggle",
        "switch between list and columns",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('l')],
            label: "v l",
        }],
        Action::ToggleLayout,
    ),
    CommandSpec::implemented(
        "layout-columns",
        "present the active query as columns",
        "Views",
        &[],
        Action::SetLayout(TaskLayout::Columns),
    ),
    CommandSpec::implemented(
        "layout-list",
        "present the active query as a list",
        "Views",
        &[],
        Action::SetLayout(TaskLayout::List),
    ),
    CommandSpec::implemented(
        "view-all",
        "show all available tasks",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('w')],
            label: "v w",
        }],
        Action::ShowView(TaskQuery::All),
    ),
    CommandSpec::implemented(
        "view-open",
        "show open task view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('o')],
            label: "v o",
        }],
        Action::ShowView(TaskQuery::Open),
    ),
    CommandSpec::implemented(
        "view-upcoming",
        "show upcoming task view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('p')],
            label: "v p",
        }],
        Action::ShowView(TaskQuery::Upcoming),
    ),
    CommandSpec::implemented(
        "view-epics",
        "show open epics",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('e')],
            label: "v e",
        }],
        Action::ShowView(TaskQuery::Epics),
    ),
    CommandSpec::implemented(
        "view-recurring",
        "show recurring tasks",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('u')],
            label: "v u",
        }],
        Action::ShowView(TaskQuery::Recurring),
    ),
    CommandSpec::implemented(
        "view-recent",
        "show recent actions",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('r')],
            label: "v r",
        }],
        Action::ShowView(TaskQuery::RecentActions),
    ),
    CommandSpec::implemented(
        "view-inbox",
        "show inbox view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('i')],
            label: "v i",
        }],
        Action::ShowView(TaskQuery::Inbox),
    ),
    CommandSpec::implemented(
        "view-backlog",
        "show backlog view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('b')],
            label: "v b",
        }],
        Action::ShowView(TaskQuery::Backlog),
    ),
    CommandSpec::implemented(
        "view-todo",
        "show todo view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('t')],
            label: "v t",
        }],
        Action::ShowView(TaskQuery::Todo),
    ),
    CommandSpec::implemented(
        "view-active",
        "show active view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('a')],
            label: "v a",
        }],
        Action::ShowView(TaskQuery::Active),
    ),
    CommandSpec::implemented(
        "view-done",
        "show done view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('d')],
            label: "v d",
        }],
        Action::ShowView(TaskQuery::Done),
    ),
    CommandSpec::implemented(
        "view-conflicts",
        "show conflicts view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('c')],
            label: "v c",
        }],
        Action::ShowView(TaskQuery::Conflicts),
    ),
    CommandSpec::implemented(
        "view-search",
        "show search results",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('v'), KeyCode::Char('s')],
            label: "v s",
        }],
        Action::ShowView(TaskQuery::Search),
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
    CommandSpec::implemented(
        "workspace-create",
        "create a workspace",
        "Workspaces",
        &[KeySequence {
            codes: &[KeyCode::Char('W'), KeyCode::Char('n')],
            label: "W n",
        }],
        Action::BeginAddWorkspace,
    ),
    CommandSpec::implemented(
        "workspace-rename",
        "rename a workspace",
        "Workspaces",
        &[KeySequence {
            codes: &[KeyCode::Char('W'), KeyCode::Char('r')],
            label: "W r",
        }],
        Action::BeginRenameWorkspace,
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
    CommandSpec::implemented_in_detail(
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
            codes: &[KeyCode::Char('p'), KeyCode::Char('a')],
            label: "p a",
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
        "browse-labels",
        "browse labels and usage",
        "Labels",
        &[KeySequence {
            codes: &[KeyCode::Char('L'), KeyCode::Char('b')],
            label: "L b",
        }],
        Action::BeginBrowseLabels,
    ),
    CommandSpec::implemented(
        "rename-label",
        "rename a label everywhere it is used",
        "Labels",
        &[KeySequence {
            codes: &[KeyCode::Char('L'), KeyCode::Char('r')],
            label: "L r",
        }],
        Action::BeginRenameLabel,
    ),
    CommandSpec::implemented(
        "delete-label",
        "delete a label everywhere it is used",
        "Labels",
        &[KeySequence {
            codes: &[KeyCode::Char('L'), KeyCode::Char('D')],
            label: "L D",
        }],
        Action::BeginDeleteLabel,
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
    CommandSpec::implemented(
        "add-project-path",
        "add a path to a project",
        "Projects",
        &[KeySequence {
            codes: &[KeyCode::Char('p'), KeyCode::Char('n')],
            label: "p n",
        }],
        Action::BeginAddProjectPath,
    ),
    CommandSpec::implemented(
        "remove-project-path",
        "remove a path from a project",
        "Projects",
        &[KeySequence {
            codes: &[KeyCode::Char('p'), KeyCode::Char('x')],
            label: "p x",
        }],
        Action::BeginRemoveProjectPath,
    ),
    // Edit
    CommandSpec::implemented_in_detail(
        "edit-title",
        "edit selected task title",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('t')],
                label: "e t",
            },
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('t')],
                label: "t e t",
            },
        ],
        Action::BeginEditTitle,
    ),
    CommandSpec::implemented_in_detail(
        "edit-description",
        "edit selected task description",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('d')],
                label: "e d",
            },
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('d')],
                label: "t e d",
            },
        ],
        Action::BeginEditDescription,
    ),
    CommandSpec::implemented_in_detail(
        "edit-project",
        "edit selected task project",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('j')],
                label: "e j",
            },
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('j')],
                label: "t e j",
            },
        ],
        Action::BeginEditProject,
    ),
    CommandSpec::implemented_in_detail(
        "edit-priority",
        "edit selected task priority",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('p')],
                label: "e p",
            },
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('p')],
                label: "t e p",
            },
        ],
        Action::BeginEditPriority,
    ),
    CommandSpec::implemented_in_detail(
        "edit-epic",
        "edit selected task epic container state",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('e')],
                label: "e e",
            },
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('e')],
                label: "t e e",
            },
        ],
        Action::BeginEditEpic,
    ),
    CommandSpec::implemented_in_detail(
        "edit-availability",
        "edit selected task availability",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('a')],
                label: "e a",
            },
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('a')],
                label: "t e a",
            },
        ],
        Action::BeginEditAvailability,
    ),
    CommandSpec::implemented_in_detail(
        "edit-due",
        "edit selected task due date",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('u')],
                label: "e u",
            },
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('u')],
                label: "t e u",
            },
        ],
        Action::BeginEditDue,
    ),
    CommandSpec::implemented_in_detail(
        "edit-labels",
        "edit selected task labels",
        "Tasks",
        &[
            KeySequence {
                codes: &[KeyCode::Char('e'), KeyCode::Char('l')],
                label: "e l",
            },
            KeySequence {
                codes: &[KeyCode::Char('t'), KeyCode::Char('e'), KeyCode::Char('l')],
                label: "t e l",
            },
        ],
        Action::BeginEditLabels,
    ),
    CommandSpec::implemented_in_detail(
        "copy-ref",
        "copy selected task display ref",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('y'), KeyCode::Char('r')],
            label: "y r",
        }],
        Action::CopyShortRef,
    ),
    CommandSpec::implemented_in_detail(
        "copy-id",
        "copy selected task id",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('y'), KeyCode::Char('i')],
            label: "y i",
        }],
        Action::CopyDurableRef,
    ),
    CommandSpec::implemented_in_detail(
        "copy-title",
        "copy selected task title",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('y'), KeyCode::Char('t')],
            label: "y t",
        }],
        Action::CopyTaskTitle,
    ),
    CommandSpec::implemented_in_detail(
        "copy-description",
        "copy selected task description",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('y'), KeyCode::Char('d')],
            label: "y d",
        }],
        Action::CopyTaskDescription,
    ),
    CommandSpec::implemented_in_detail(
        "copy-text",
        "copy selected task title and description",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('y'), KeyCode::Char('a')],
            label: "y a",
        }],
        Action::CopyTaskText,
    ),
    CommandSpec::implemented_in_detail(
        "copy-notes",
        "copy selected task notes",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('y'), KeyCode::Char('n')],
            label: "y n",
        }],
        Action::CopyTaskNotes,
    ),
    CommandSpec::implemented_in_detail(
        "copy-markdown",
        "copy selected task as Markdown",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('y'), KeyCode::Char('m')],
            label: "y m",
        }],
        Action::CopyTaskMarkdown,
    ),
    CommandSpec::implemented_with_detail_bindings(
        "create-gist",
        &["gist"],
        "create a secret GitHub gist from this task",
        "Tasks",
        &[],
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('g')],
            label: "t g",
        }],
        DetailFocusPolicy::ParentTask,
        Action::BeginCreateTaskGist,
    ),
    // Priority
    CommandSpec::implemented_in_detail(
        "priority-none",
        "set priority to none",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('0')],
            label: "t 0",
        }],
        Action::SetPriority(TaskPriority::None),
    ),
    CommandSpec::implemented_in_detail(
        "priority-low",
        "set priority to low",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('l')],
            label: "t l",
        }],
        Action::SetPriority(TaskPriority::Low),
    ),
    CommandSpec::implemented_in_detail(
        "priority-medium",
        "set priority to medium",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('m')],
            label: "t m",
        }],
        Action::SetPriority(TaskPriority::Medium),
    ),
    CommandSpec::implemented_in_detail(
        "priority-high",
        "set priority to high",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('h')],
            label: "t h",
        }],
        Action::SetPriority(TaskPriority::High),
    ),
    CommandSpec::implemented_in_detail(
        "priority-urgent",
        "set priority to urgent",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('u')],
            label: "t u",
        }],
        Action::SetPriority(TaskPriority::Urgent),
    ),
    CommandSpec::implemented(
        "toggle-mark",
        "toggle mark on selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char(' ')],
            label: "Space",
        }],
        Action::ToggleMarkSelected,
    ),
    CommandSpec::implemented(
        "toggle-mark-all",
        "toggle marks on visible tasks",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('V')],
            label: "t V",
        }],
        Action::ToggleMarkAllInView,
    ),
    CommandSpec::implemented(
        "clear-marks",
        "clear task marks",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('C')],
            label: "t C",
        }],
        Action::ClearMarks,
    ),
    // Dependencies
    CommandSpec::implemented_in_detail(
        "add-dependency",
        "add blocker to selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('B')],
            label: "t B",
        }],
        Action::BeginAddDependency,
    ),
    CommandSpec::implemented_in_detail(
        "remove-dependency",
        "remove blocker or unlink focused relationship",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('U')],
            label: "t U",
        }],
        Action::BeginRemoveDependency,
    ),
    // Related tasks
    CommandSpec::implemented_in_detail(
        "add-related",
        "add a related task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('k'), KeyCode::Char('a')],
            label: "t k a",
        }],
        Action::BeginAddRelated,
    ),
    CommandSpec::implemented_in_detail(
        "remove-related",
        "remove a related task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('k'), KeyCode::Char('r')],
            label: "t k r",
        }],
        Action::BeginRemoveRelated,
    ),
    // Epic
    CommandSpec::implemented(
        "task-epic-toggle",
        "toggle epic parent expand/collapse",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('t')],
            label: "t c t",
        }],
        Action::ToggleEpicExpanded,
    ),
    CommandSpec::implemented_for_epic_child(
        "task-child-add",
        "add a child to the selected epic",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')],
            label: "t c a",
        }],
        Action::BeginAddEpicChild,
    ),
    CommandSpec::implemented_for_epic_child(
        "task-child-remove",
        "remove the selected child from its epic",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('r')],
            label: "t c r",
        }],
        Action::RemoveEpicChild,
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
        "filter-recurring-lifecycle",
        "cycle recurring series lifecycle",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('r')],
            label: "f r",
        }],
        Action::CycleRecurringLifecycleFilter,
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
        "filter-closed",
        "cycle closed task visibility",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('d')],
            label: "f d",
        }],
        Action::ToggleClosedFilter,
    ),
    CommandSpec::implemented(
        "filter-deleted",
        "cycle deleted task visibility",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('x')],
            label: "f x",
        }],
        Action::ToggleDeletedFilter,
    ),
    // Order
    CommandSpec::implemented(
        "order-due",
        "sort by due date",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('d')],
            label: "o d",
        }],
        Action::SetOrder(crate::tui::store::TaskOrder::DueOn),
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
    CommandSpec::implemented_global_in_detail(
        "conflict-list",
        "list or filter conflicts",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('l')],
            label: "c l",
        }],
        Action::BeginConflictList,
    ),
    CommandSpec::implemented_in_detail(
        "conflict-show",
        "show conflict details",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('s')],
            label: "c s",
        }],
        Action::ShowConflictDetails,
    ),
    CommandSpec::implemented_global_in_detail(
        "conflict-next",
        "jump to next conflict",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('n')],
            label: "c n",
        }],
        Action::NextConflict,
    ),
    CommandSpec::implemented_global_in_detail(
        "conflict-prev",
        "jump to previous conflict",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('p')],
            label: "c p",
        }],
        Action::PreviousConflict,
    ),
    CommandSpec::implemented_in_detail(
        "conflict-use-local",
        "resolve with local value",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('a')],
            label: "c a",
        }],
        Action::AcceptConflictLocal,
    ),
    CommandSpec::implemented_in_detail(
        "conflict-use-remote",
        "resolve with remote value",
        "Conflicts",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('r')],
            label: "c r",
        }],
        Action::AcceptConflictRemote,
    ),
    CommandSpec::implemented_in_detail(
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
    CommandDomain {
        section: "Workspaces",
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
