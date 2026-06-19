use crossterm::event::KeyCode;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Quit,
    MoveDown,
    MoveUp,
    First,
    Last,
    ToggleFocus,
    ToggleDetail,
    ToggleHelp,
    BeginSearch,
    BeginCommand,
    AcceptSearch,
    AcceptCommand,
    CancelOverlay,
    CancelSearch,
    CancelCommand,
    BackspaceSearch,
    BackspaceCommand,
    SearchChar(char),
    CommandChar(char),
    Refresh,
    CycleSort,
    SetStatus(&'static str),
    CyclePriority(bool),
    Delete,
    Restore,
    Planned(&'static str),
    Disabled(&'static str),
    None,
}

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
    pub(crate) description: &'static str,
    pub(crate) section: &'static str,
    pub(crate) keys: &'static [KeySequence],
    pub(crate) action: Action,
    pub(crate) lifecycle: CommandLifecycle,
}

const PLANNED_FLOW_REASON: &str = "not yet implemented";
const CONFLICT_OVERLAY_REASON: &str = "requires the conflict overlay";

impl CommandSpec {
    const fn implemented(
        name: &'static str,
        description: &'static str,
        section: &'static str,
        keys: &'static [KeySequence],
        action: Action,
    ) -> Self {
        Self {
            name,
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
            description,
            section,
            keys,
            action: Action::Planned(name),
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
            description,
            section,
            keys,
            action: Action::Disabled(name),
            lifecycle: CommandLifecycle::Disabled { reason },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandLookup {
    Empty,
    Found(Action),
    Ambiguous,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutLookup {
    Found(Action),
    Prefix,
    Ambiguous(Action),
    Missing,
}

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
        &[
            KeySequence {
                codes: &[KeyCode::Enter],
                label: "Enter",
            },
            KeySequence {
                codes: &[KeyCode::Char('l')],
                label: "l",
            },
        ],
        Action::ToggleDetail,
    ),
    CommandSpec::implemented(
        "sort",
        "cycle sort order",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('s')],
            label: "s",
        }],
        Action::CycleSort,
    ),
    CommandSpec::implemented(
        "priority-next",
        "cycle priority forward",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('p')],
            label: "p",
        }],
        Action::CyclePriority(false),
    ),
    CommandSpec::implemented(
        "priority-prev",
        "cycle priority backward",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('P')],
            label: "P",
        }],
        Action::CyclePriority(true),
    ),
    CommandSpec::implemented(
        "delete",
        "delete selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('d')],
            label: "d",
        }],
        Action::Delete,
    ),
    CommandSpec::implemented(
        "restore",
        "restore selected task",
        "Tasks",
        &[KeySequence {
            codes: &[KeyCode::Char('u')],
            label: "u",
        }],
        Action::Restore,
    ),
    CommandSpec::implemented(
        "status-inbox",
        "set status to inbox",
        "Status",
        &[
            KeySequence {
                codes: &[KeyCode::Char('1')],
                label: "1",
            },
            KeySequence {
                codes: &[KeyCode::Char('m'), KeyCode::Char('i')],
                label: "m i",
            },
        ],
        Action::SetStatus("inbox"),
    ),
    CommandSpec::implemented(
        "status-backlog",
        "set status to backlog",
        "Status",
        &[
            KeySequence {
                codes: &[KeyCode::Char('2')],
                label: "2",
            },
            KeySequence {
                codes: &[KeyCode::Char('m'), KeyCode::Char('b')],
                label: "m b",
            },
        ],
        Action::SetStatus("backlog"),
    ),
    CommandSpec::implemented(
        "status-todo",
        "set status to todo",
        "Status",
        &[
            KeySequence {
                codes: &[KeyCode::Char('3')],
                label: "3",
            },
            KeySequence {
                codes: &[KeyCode::Char('m'), KeyCode::Char('t')],
                label: "m t",
            },
        ],
        Action::SetStatus("todo"),
    ),
    CommandSpec::implemented(
        "status-active",
        "set status to active",
        "Status",
        &[
            KeySequence {
                codes: &[KeyCode::Char('4')],
                label: "4",
            },
            KeySequence {
                codes: &[KeyCode::Char('m'), KeyCode::Char('a')],
                label: "m a",
            },
        ],
        Action::SetStatus("active"),
    ),
    CommandSpec::implemented(
        "status-done",
        "set status to done",
        "Status",
        &[
            KeySequence {
                codes: &[KeyCode::Char('5')],
                label: "5",
            },
            KeySequence {
                codes: &[KeyCode::Char('m'), KeyCode::Char('d')],
                label: "m d",
            },
        ],
        Action::SetStatus("done"),
    ),
    CommandSpec::implemented(
        "status-canceled",
        "set status to canceled",
        "Status",
        &[
            KeySequence {
                codes: &[KeyCode::Char('6')],
                label: "6",
            },
            KeySequence {
                codes: &[KeyCode::Char('m'), KeyCode::Char('x')],
                label: "m x",
            },
        ],
        Action::SetStatus("canceled"),
    ),
    // Views
    CommandSpec::planned(
        "view-all",
        "show all tasks",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('a')],
            label: "g a",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "view-inbox",
        "show inbox view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('i')],
            label: "g i",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "view-backlog",
        "show backlog view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('b')],
            label: "g b",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "view-todo",
        "show todo view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('t')],
            label: "g t",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "view-active",
        "show active view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('v')],
            label: "g v",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "view-project",
        "show project view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('p')],
            label: "g p",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "view-conflicts",
        "show conflicts view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('c')],
            label: "g c",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "view-deleted",
        "show deleted tasks view",
        "Views",
        &[KeySequence {
            codes: &[KeyCode::Char('g'), KeyCode::Char('x')],
            label: "g x",
        }],
        PLANNED_FLOW_REASON,
    ),
    // Add/Create
    CommandSpec::planned(
        "add-task",
        "add a new task",
        "Add/Create",
        &[KeySequence {
            codes: &[KeyCode::Char('a'), KeyCode::Char('t')],
            label: "a t",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "add-note",
        "add a note to selected task",
        "Add/Create",
        &[KeySequence {
            codes: &[KeyCode::Char('a'), KeyCode::Char('n')],
            label: "a n",
        }],
        PLANNED_FLOW_REASON,
    ),
    // Metadata
    CommandSpec::planned(
        "add-project",
        "create a new project",
        "Metadata",
        &[KeySequence {
            codes: &[KeyCode::Char('a'), KeyCode::Char('p')],
            label: "a p",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "add-label",
        "create a new label",
        "Metadata",
        &[KeySequence {
            codes: &[KeyCode::Char('a'), KeyCode::Char('l')],
            label: "a l",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "add-project-path",
        "add a path to a project",
        "Metadata",
        &[KeySequence {
            codes: &[KeyCode::Char('a'), KeyCode::Char('P')],
            label: "a P",
        }],
        PLANNED_FLOW_REASON,
    ),
    // Edit
    CommandSpec::planned(
        "edit-title",
        "edit selected task title",
        "Edit",
        &[KeySequence {
            codes: &[KeyCode::Char('e'), KeyCode::Char('t')],
            label: "e t",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "edit-description",
        "edit selected task description",
        "Edit",
        &[KeySequence {
            codes: &[KeyCode::Char('e'), KeyCode::Char('d')],
            label: "e d",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "edit-project",
        "edit selected task project",
        "Edit",
        &[KeySequence {
            codes: &[KeyCode::Char('e'), KeyCode::Char('p')],
            label: "e p",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "edit-priority",
        "edit selected task priority",
        "Edit",
        &[KeySequence {
            codes: &[KeyCode::Char('e'), KeyCode::Char('r')],
            label: "e r",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "edit-labels",
        "edit selected task labels",
        "Edit",
        &[KeySequence {
            codes: &[KeyCode::Char('e'), KeyCode::Char('l')],
            label: "e l",
        }],
        PLANNED_FLOW_REASON,
    ),
    // Priority
    CommandSpec::planned(
        "priority-none",
        "set priority to none",
        "Priority",
        &[KeySequence {
            codes: &[KeyCode::Char('m'), KeyCode::Char('0')],
            label: "m 0",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "priority-low",
        "set priority to low",
        "Priority",
        &[KeySequence {
            codes: &[KeyCode::Char('m'), KeyCode::Char('l')],
            label: "m l",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "priority-medium",
        "set priority to medium",
        "Priority",
        &[KeySequence {
            codes: &[KeyCode::Char('m'), KeyCode::Char('m')],
            label: "m m",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "priority-high",
        "set priority to high",
        "Priority",
        &[KeySequence {
            codes: &[KeyCode::Char('m'), KeyCode::Char('h')],
            label: "m h",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "priority-urgent",
        "set priority to urgent",
        "Priority",
        &[KeySequence {
            codes: &[KeyCode::Char('m'), KeyCode::Char('u')],
            label: "m u",
        }],
        PLANNED_FLOW_REASON,
    ),
    // Filters
    CommandSpec::planned(
        "filter-project",
        "filter by project",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('p')],
            label: "f p",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "filter-label",
        "filter by label",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('l')],
            label: "f l",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "filter-status",
        "filter by status",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('s')],
            label: "f s",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "filter-priority",
        "filter by priority",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('r')],
            label: "f r",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "filter-clear",
        "clear all filters",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('c')],
            label: "f c",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "filter-deleted",
        "filter deleted tasks",
        "Filters",
        &[KeySequence {
            codes: &[KeyCode::Char('f'), KeyCode::Char('x')],
            label: "f x",
        }],
        PLANNED_FLOW_REASON,
    ),
    // Order
    CommandSpec::planned(
        "order-queue",
        "sort by queue order",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('q')],
            label: "o q",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "order-created",
        "sort by created date",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('c')],
            label: "o c",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "order-updated",
        "sort by updated date",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('u')],
            label: "o u",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "order-project",
        "sort by project",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('p')],
            label: "o p",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "order-title",
        "sort by title",
        "Order",
        &[KeySequence {
            codes: &[KeyCode::Char('o'), KeyCode::Char('t')],
            label: "o t",
        }],
        PLANNED_FLOW_REASON,
    ),
    // Conflict
    CommandSpec::planned(
        "conflict-list",
        "list or filter conflicts",
        "Conflict",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('l')],
            label: "c l",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "conflict-show",
        "show conflict details",
        "Conflict",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('s')],
            label: "c s",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "conflict-next",
        "jump to next conflict",
        "Conflict",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('n')],
            label: "c n",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "conflict-resolve",
        "resolve selected conflict",
        "Conflict",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('r')],
            label: "c r",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::disabled(
        "conflict-use-a",
        "resolve with variant A",
        "Conflict",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('a')],
            label: "c a",
        }],
        CONFLICT_OVERLAY_REASON,
    ),
    CommandSpec::disabled(
        "conflict-use-b",
        "resolve with variant B",
        "Conflict",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('b')],
            label: "c b",
        }],
        CONFLICT_OVERLAY_REASON,
    ),
    CommandSpec::disabled(
        "conflict-edit-value",
        "resolve with manual edit",
        "Conflict",
        &[KeySequence {
            codes: &[KeyCode::Char('c'), KeyCode::Char('e')],
            label: "c e",
        }],
        CONFLICT_OVERLAY_REASON,
    ),
    // Config
    CommandSpec::planned(
        "config-show",
        "show configuration",
        "Config",
        &[KeySequence {
            codes: &[KeyCode::Char('C'), KeyCode::Char('s')],
            label: "C s",
        }],
        PLANNED_FLOW_REASON,
    ),
    CommandSpec::planned(
        "config-init",
        "initialize configuration",
        "Config",
        &[KeySequence {
            codes: &[KeyCode::Char('C'), KeyCode::Char('i')],
            label: "C i",
        }],
        PLANNED_FLOW_REASON,
    ),
];

pub(crate) fn key_label(code: KeyCode) -> String {
    match code {
        KeyCode::Char(ch) => ch.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        _ => format!("{code:?}"),
    }
}

pub(crate) fn shortcut_label(codes: &[KeyCode]) -> String {
    codes
        .iter()
        .map(|code| key_label(*code))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn resolve_shortcut(input: &[KeyCode]) -> ShortcutLookup {
    resolve_shortcut_in(COMMANDS, input)
}

pub(crate) fn resolve_shortcut_in(commands: &[CommandSpec], input: &[KeyCode]) -> ShortcutLookup {
    if input.is_empty() {
        return ShortcutLookup::Missing;
    }

    let mut exact = Vec::new();
    let mut prefix = false;

    for command in commands {
        for key in command.keys {
            if key.codes == input {
                exact.push(command.action);
            } else if key.codes.starts_with(input) {
                prefix = true;
            }
        }
    }

    match (exact.as_slice(), prefix) {
        ([action], false) => ShortcutLookup::Found(*action),
        ([action], true) => ShortcutLookup::Ambiguous(*action),
        ([action, ..], _) => ShortcutLookup::Ambiguous(*action),
        ([], true) => ShortcutLookup::Prefix,
        ([], false) => ShortcutLookup::Missing,
    }
}

pub(crate) fn matching_commands(input: &str) -> Vec<&'static CommandSpec> {
    let input = input.trim();
    if input.is_empty() {
        return COMMANDS.iter().collect();
    }
    COMMANDS
        .iter()
        .filter(|command| command.name == input || command.name.starts_with(input))
        .collect()
}

pub(crate) fn lookup_command(input: &str) -> CommandLookup {
    let input = input.trim();
    if input.is_empty() {
        return CommandLookup::Empty;
    }
    let matches = matching_commands(input);
    match matches.as_slice() {
        [command] => CommandLookup::Found(command.action),
        [] => CommandLookup::Missing,
        _ => CommandLookup::Ambiguous,
    }
}

impl Action {
    #[allow(dead_code)]
    pub(crate) fn from_search_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Esc => Self::CancelSearch,
            KeyCode::Enter => Self::AcceptSearch,
            KeyCode::Backspace => Self::BackspaceSearch,
            KeyCode::Char(ch) => Self::SearchChar(ch),
            _ => Self::None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn from_command_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Esc => Self::CancelCommand,
            KeyCode::Enter => Self::AcceptCommand,
            KeyCode::Backspace => Self::BackspaceCommand,
            KeyCode::Char(ch) => Self::CommandChar(ch),
            _ => Self::None,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_normal_key(code: KeyCode) -> Self {
        if code == KeyCode::Esc {
            return Self::CancelOverlay;
        }

        match resolve_shortcut(&[code]) {
            ShortcutLookup::Found(action) | ShortcutLookup::Ambiguous(action) => action,
            ShortcutLookup::Prefix | ShortcutLookup::Missing => Self::None,
        }
    }
}

#[cfg(test)]
fn implemented_action_is_handled(action: Action) -> bool {
    matches!(
        action,
        Action::Quit
            | Action::MoveDown
            | Action::MoveUp
            | Action::First
            | Action::Last
            | Action::ToggleFocus
            | Action::ToggleDetail
            | Action::ToggleHelp
            | Action::BeginSearch
            | Action::BeginCommand
            | Action::Refresh
            | Action::CycleSort
            | Action::SetStatus(_)
            | Action::CyclePriority(_)
            | Action::Delete
            | Action::Restore
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_navigation_keys() {
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('j')),
            Action::MoveDown
        );
        assert_eq!(Action::from_normal_key(KeyCode::Down), Action::MoveDown);
        assert_eq!(Action::from_normal_key(KeyCode::Char('k')), Action::MoveUp);
        assert_eq!(Action::from_normal_key(KeyCode::Up), Action::MoveUp);
    }

    #[test]
    fn maps_status_keys() {
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('1')),
            Action::SetStatus("inbox")
        );
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('4')),
            Action::SetStatus("active")
        );
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('6')),
            Action::SetStatus("canceled")
        );
    }

    #[test]
    fn search_mode_captures_text_keys() {
        assert_eq!(
            Action::from_search_key(KeyCode::Char('q')),
            Action::SearchChar('q')
        );
        assert_eq!(
            Action::from_search_key(KeyCode::Enter),
            Action::AcceptSearch
        );
        assert_eq!(Action::from_search_key(KeyCode::Esc), Action::CancelSearch);
    }

    #[test]
    fn normal_escape_cancels_overlay() {
        assert_eq!(Action::from_normal_key(KeyCode::Esc), Action::CancelOverlay);
    }

    #[test]
    fn maps_command_panel_key() {
        assert_eq!(
            Action::from_normal_key(KeyCode::Char(':')),
            Action::BeginCommand
        );
    }

    #[test]
    fn command_mode_captures_text_keys() {
        assert_eq!(
            Action::from_command_key(KeyCode::Char('q')),
            Action::CommandChar('q')
        );
        assert_eq!(
            Action::from_command_key(KeyCode::Enter),
            Action::AcceptCommand
        );
        assert_eq!(
            Action::from_command_key(KeyCode::Esc),
            Action::CancelCommand
        );
        assert_eq!(
            Action::from_command_key(KeyCode::Backspace),
            Action::BackspaceCommand
        );
    }

    #[test]
    fn lookup_command_finds_exact_name() {
        assert_eq!(lookup_command("quit"), CommandLookup::Found(Action::Quit));
    }

    #[test]
    fn lookup_command_finds_unique_prefix() {
        assert_eq!(lookup_command("ref"), CommandLookup::Found(Action::Refresh));
    }

    #[test]
    fn lookup_command_reports_ambiguous_prefix() {
        assert_eq!(lookup_command("s"), CommandLookup::Ambiguous);
    }

    #[test]
    fn lookup_command_reports_empty_input() {
        assert_eq!(lookup_command(""), CommandLookup::Empty);
        assert_eq!(lookup_command("   "), CommandLookup::Empty);
    }

    #[test]
    fn lookup_command_reports_missing_input() {
        assert_eq!(lookup_command("zzzz"), CommandLookup::Missing);
    }

    #[test]
    fn resolves_single_key_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('q')]),
            ShortcutLookup::Found(Action::Quit)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('j')]),
            ShortcutLookup::Found(Action::MoveDown)
        );
    }

    #[test]
    fn resolves_multi_key_sequences_from_catalog() {
        let commands = [CommandSpec::implemented(
            "test-sequence",
            "test sequence",
            "Test",
            &[KeySequence {
                codes: &[KeyCode::Char('a'), KeyCode::Char('t')],
                label: "a t",
            }],
            Action::BeginSearch,
        )];

        assert_eq!(
            resolve_shortcut_in(&commands, &[KeyCode::Char('a')]),
            ShortcutLookup::Prefix
        );
        assert_eq!(
            resolve_shortcut_in(&commands, &[KeyCode::Char('a'), KeyCode::Char('t')]),
            ShortcutLookup::Found(Action::BeginSearch)
        );
    }

    #[test]
    fn resolves_exact_prefix_ambiguity() {
        let commands = [
            CommandSpec::implemented(
                "single-g",
                "single g",
                "Test",
                &[KeySequence {
                    codes: &[KeyCode::Char('g')],
                    label: "g",
                }],
                Action::First,
            ),
            CommandSpec::implemented(
                "double-g",
                "double g",
                "Test",
                &[KeySequence {
                    codes: &[KeyCode::Char('g'), KeyCode::Char('g')],
                    label: "g g",
                }],
                Action::Last,
            ),
        ];

        assert_eq!(
            resolve_shortcut_in(&commands, &[KeyCode::Char('g')]),
            ShortcutLookup::Ambiguous(Action::First)
        );
    }

    #[test]
    fn resolves_duplicate_exact_sequences_as_ambiguous() {
        let commands = [
            CommandSpec::implemented(
                "first-q",
                "first q",
                "Test",
                &[KeySequence {
                    codes: &[KeyCode::Char('q')],
                    label: "q",
                }],
                Action::Quit,
            ),
            CommandSpec::implemented(
                "second-q",
                "second q",
                "Test",
                &[KeySequence {
                    codes: &[KeyCode::Char('q')],
                    label: "q",
                }],
                Action::Refresh,
            ),
        ];

        assert_eq!(
            resolve_shortcut_in(&commands, &[KeyCode::Char('q')]),
            ShortcutLookup::Ambiguous(Action::Quit)
        );
    }

    #[test]
    fn resolver_reports_missing_and_empty_inputs() {
        assert_eq!(resolve_shortcut(&[]), ShortcutLookup::Missing);
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('z')]),
            ShortcutLookup::Missing
        );
    }

    #[test]
    fn resolves_phase_prefix_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('g')]),
            ShortcutLookup::Prefix
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('g'), KeyCode::Char('g')]),
            ShortcutLookup::Found(Action::First)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m')]),
            ShortcutLookup::Prefix
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m'), KeyCode::Char('a')]),
            ShortcutLookup::Found(Action::SetStatus("active"))
        );
    }

    #[test]
    fn formats_shortcut_labels() {
        assert_eq!(
            shortcut_label(&[KeyCode::Char('m'), KeyCode::Char('a')]),
            "m a"
        );
        assert_eq!(shortcut_label(&[KeyCode::Home]), "Home");
    }

    #[test]
    fn preserves_existing_shortcuts() {
        for command in COMMANDS {
            for key in command.keys {
                if key.codes.len() != 1 {
                    continue;
                }
                assert_eq!(
                    Action::from_normal_key(key.codes[0]),
                    command.action,
                    "shortcut {} for :{} resolved incorrectly",
                    key.label,
                    command.name
                );
            }
        }

        assert_eq!(Action::from_normal_key(KeyCode::Esc), Action::CancelOverlay);
        assert_eq!(Action::from_normal_key(KeyCode::Char('z')), Action::None);
        assert_eq!(Action::from_normal_key(KeyCode::Char('g')), Action::None);
    }

    #[test]
    fn production_sequences_are_not_ambiguous() {
        for command in COMMANDS {
            for key in command.keys {
                assert_ne!(
                    resolve_shortcut(key.codes),
                    ShortcutLookup::Ambiguous(command.action),
                    "shortcut {} for :{} should not be ambiguous",
                    key.label,
                    command.name
                );
            }
        }
    }

    #[test]
    fn catalog_lifecycle_matches_action_state() {
        for command in COMMANDS {
            match (command.lifecycle, command.action) {
                (CommandLifecycle::Implemented, action) => {
                    assert!(
                        implemented_action_is_handled(action),
                        "implemented :{} is not handled",
                        command.name
                    );
                }
                (CommandLifecycle::Planned { reason }, Action::Planned(name)) => {
                    assert_eq!(name, command.name);
                    assert!(
                        !reason.trim().is_empty(),
                        ":{} planned reason is empty",
                        command.name
                    );
                }
                (CommandLifecycle::Disabled { reason }, Action::Disabled(name)) => {
                    assert_eq!(name, command.name);
                    assert!(
                        !reason.trim().is_empty(),
                        ":{} disabled reason is empty",
                        command.name
                    );
                }
                _ => panic!("lifecycle/action mismatch for :{}", command.name),
            }
        }
    }

    #[test]
    fn catalog_rejects_duplicate_exact_shortcuts() {
        let mut seen: Vec<(&[KeyCode], &str, &str)> = Vec::new();
        for command in COMMANDS {
            for key in command.keys {
                if let Some((_, other_command, other_label)) =
                    seen.iter().find(|(codes, _, _)| *codes == key.codes)
                {
                    panic!(
                        "duplicate shortcut {} for :{} conflicts with {} for :{}",
                        key.label, command.name, other_label, other_command
                    );
                }
                seen.push((key.codes, command.name, key.label));
            }
        }
    }

    #[test]
    fn required_action_families_are_present() {
        for name in [
            "add-task",
            "edit-title",
            "status-active",
            "filter-project",
            "order-queue",
            "conflict-list",
            "add-project",
            "config-show",
        ] {
            assert!(
                COMMANDS.iter().any(|command| command.name == name),
                "missing required command :{name}"
            );
        }
    }
}
