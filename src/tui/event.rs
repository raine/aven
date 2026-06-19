use crossterm::event::KeyCode;

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
    None,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct KeyBinding {
    pub(crate) code: KeyCode,
    pub(crate) label: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandSpec {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) section: &'static str,
    pub(crate) keys: &'static [KeyBinding],
    pub(crate) action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandLookup {
    Empty,
    Found(Action),
    Ambiguous,
    Missing,
}

pub(crate) const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "quit",
        description: "quit the TUI",
        section: "General",
        keys: &[KeyBinding {
            code: KeyCode::Char('q'),
            label: "q",
        }],
        action: Action::Quit,
    },
    CommandSpec {
        name: "command",
        description: "open the command panel",
        section: "General",
        keys: &[KeyBinding {
            code: KeyCode::Char(':'),
            label: ":",
        }],
        action: Action::BeginCommand,
    },
    CommandSpec {
        name: "help",
        description: "toggle shortcut help",
        section: "General",
        keys: &[KeyBinding {
            code: KeyCode::Char('?'),
            label: "?",
        }],
        action: Action::ToggleHelp,
    },
    CommandSpec {
        name: "refresh",
        description: "reload tasks",
        section: "General",
        keys: &[KeyBinding {
            code: KeyCode::Char('r'),
            label: "r",
        }],
        action: Action::Refresh,
    },
    CommandSpec {
        name: "search",
        description: "search title and description",
        section: "General",
        keys: &[KeyBinding {
            code: KeyCode::Char('/'),
            label: "/",
        }],
        action: Action::BeginSearch,
    },
    CommandSpec {
        name: "move-down",
        description: "move selection down",
        section: "Navigation",
        keys: &[
            KeyBinding {
                code: KeyCode::Char('j'),
                label: "j",
            },
            KeyBinding {
                code: KeyCode::Down,
                label: "Down",
            },
        ],
        action: Action::MoveDown,
    },
    CommandSpec {
        name: "move-up",
        description: "move selection up",
        section: "Navigation",
        keys: &[
            KeyBinding {
                code: KeyCode::Char('k'),
                label: "k",
            },
            KeyBinding {
                code: KeyCode::Up,
                label: "Up",
            },
        ],
        action: Action::MoveUp,
    },
    CommandSpec {
        name: "first",
        description: "jump to the first item",
        section: "Navigation",
        keys: &[
            KeyBinding {
                code: KeyCode::Char('g'),
                label: "g",
            },
            KeyBinding {
                code: KeyCode::Home,
                label: "Home",
            },
        ],
        action: Action::First,
    },
    CommandSpec {
        name: "last",
        description: "jump to the last item",
        section: "Navigation",
        keys: &[
            KeyBinding {
                code: KeyCode::Char('G'),
                label: "G",
            },
            KeyBinding {
                code: KeyCode::End,
                label: "End",
            },
        ],
        action: Action::Last,
    },
    CommandSpec {
        name: "focus",
        description: "switch between views and tasks",
        section: "Navigation",
        keys: &[
            KeyBinding {
                code: KeyCode::Tab,
                label: "Tab",
            },
            KeyBinding {
                code: KeyCode::BackTab,
                label: "Shift+Tab",
            },
        ],
        action: Action::ToggleFocus,
    },
    CommandSpec {
        name: "detail",
        description: "select a view or toggle task detail",
        section: "Navigation",
        keys: &[
            KeyBinding {
                code: KeyCode::Enter,
                label: "Enter",
            },
            KeyBinding {
                code: KeyCode::Char('l'),
                label: "l",
            },
        ],
        action: Action::ToggleDetail,
    },
    CommandSpec {
        name: "sort",
        description: "cycle sort order",
        section: "Tasks",
        keys: &[KeyBinding {
            code: KeyCode::Char('s'),
            label: "s",
        }],
        action: Action::CycleSort,
    },
    CommandSpec {
        name: "priority-next",
        description: "cycle priority forward",
        section: "Tasks",
        keys: &[KeyBinding {
            code: KeyCode::Char('p'),
            label: "p",
        }],
        action: Action::CyclePriority(false),
    },
    CommandSpec {
        name: "priority-prev",
        description: "cycle priority backward",
        section: "Tasks",
        keys: &[KeyBinding {
            code: KeyCode::Char('P'),
            label: "P",
        }],
        action: Action::CyclePriority(true),
    },
    CommandSpec {
        name: "delete",
        description: "delete selected task",
        section: "Tasks",
        keys: &[KeyBinding {
            code: KeyCode::Char('d'),
            label: "d",
        }],
        action: Action::Delete,
    },
    CommandSpec {
        name: "restore",
        description: "restore selected task",
        section: "Tasks",
        keys: &[KeyBinding {
            code: KeyCode::Char('u'),
            label: "u",
        }],
        action: Action::Restore,
    },
    CommandSpec {
        name: "status-inbox",
        description: "set status to inbox",
        section: "Status",
        keys: &[KeyBinding {
            code: KeyCode::Char('1'),
            label: "1",
        }],
        action: Action::SetStatus("inbox"),
    },
    CommandSpec {
        name: "status-backlog",
        description: "set status to backlog",
        section: "Status",
        keys: &[KeyBinding {
            code: KeyCode::Char('2'),
            label: "2",
        }],
        action: Action::SetStatus("backlog"),
    },
    CommandSpec {
        name: "status-todo",
        description: "set status to todo",
        section: "Status",
        keys: &[KeyBinding {
            code: KeyCode::Char('3'),
            label: "3",
        }],
        action: Action::SetStatus("todo"),
    },
    CommandSpec {
        name: "status-active",
        description: "set status to active",
        section: "Status",
        keys: &[KeyBinding {
            code: KeyCode::Char('4'),
            label: "4",
        }],
        action: Action::SetStatus("active"),
    },
    CommandSpec {
        name: "status-done",
        description: "set status to done",
        section: "Status",
        keys: &[KeyBinding {
            code: KeyCode::Char('5'),
            label: "5",
        }],
        action: Action::SetStatus("done"),
    },
    CommandSpec {
        name: "status-canceled",
        description: "set status to canceled",
        section: "Status",
        keys: &[KeyBinding {
            code: KeyCode::Char('6'),
            label: "6",
        }],
        action: Action::SetStatus("canceled"),
    },
];

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
    pub(crate) fn from_search_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Esc => Self::CancelSearch,
            KeyCode::Enter => Self::AcceptSearch,
            KeyCode::Backspace => Self::BackspaceSearch,
            KeyCode::Char(ch) => Self::SearchChar(ch),
            _ => Self::None,
        }
    }

    pub(crate) fn from_command_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Esc => Self::CancelCommand,
            KeyCode::Enter => Self::AcceptCommand,
            KeyCode::Backspace => Self::BackspaceCommand,
            KeyCode::Char(ch) => Self::CommandChar(ch),
            _ => Self::None,
        }
    }

    pub(crate) fn from_normal_key(code: KeyCode) -> Self {
        if code == KeyCode::Esc {
            return Self::CancelOverlay;
        }

        COMMANDS
            .iter()
            .find_map(|command| {
                command
                    .keys
                    .iter()
                    .any(|key| key.code == code)
                    .then_some(command.action)
            })
            .unwrap_or(Self::None)
    }
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
    fn preserves_existing_shortcuts() {
        assert_eq!(Action::from_normal_key(KeyCode::Char('q')), Action::Quit);
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('?')),
            Action::ToggleHelp
        );
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('/')),
            Action::BeginSearch
        );
        assert_eq!(Action::from_normal_key(KeyCode::Char('r')), Action::Refresh);
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('s')),
            Action::CycleSort
        );
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('p')),
            Action::CyclePriority(false)
        );
        assert_eq!(
            Action::from_normal_key(KeyCode::Char('P')),
            Action::CyclePriority(true)
        );
        assert_eq!(Action::from_normal_key(KeyCode::Char('d')), Action::Delete);
        assert_eq!(Action::from_normal_key(KeyCode::Char('u')), Action::Restore);
        assert_eq!(Action::from_normal_key(KeyCode::Tab), Action::ToggleFocus);
        assert_eq!(
            Action::from_normal_key(KeyCode::Enter),
            Action::ToggleDetail
        );
    }
}
