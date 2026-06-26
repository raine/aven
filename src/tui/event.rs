mod action;
mod catalog;
mod lookup;

pub(crate) use self::action::Action;
#[allow(unused_imports)]
pub(crate) use self::catalog::{
    COMMAND_DOMAINS, COMMANDS, CommandContext, CommandDomain, CommandLifecycle, CommandSpec,
    DETAIL_COMMANDS, KeySequence,
};
#[cfg(test)]
pub(crate) use self::catalog::{DUE_SORT_REASON, PROJECT_PATH_FLOW_REASON};
#[allow(unused_imports)]
pub(crate) use self::lookup::{
    CommandCompletion, CommandLookup, ShortcutLookup, command_cycle_options, complete_command,
    key_label, lookup_command, matching_commands, prefix_hint_commands, resolve_shortcut,
    resolve_shortcut_for, resolve_shortcut_in, shortcut_label,
};

#[cfg(test)]
fn implemented_action_is_handled(action: Action) -> bool {
    matches!(
        action,
        Action::Quit
            | Action::MoveDown
            | Action::MoveUp
            | Action::MoveLeft
            | Action::MoveRight
            | Action::PreviousItem
            | Action::NextItem
            | Action::First
            | Action::Last
            | Action::ToggleFocus
            | Action::ToggleDetail
            | Action::ToggleHelp
            | Action::BeginSearch
            | Action::BeginCommand
            | Action::Refresh
            | Action::SetOrder(_)
            | Action::ReverseSort
            | Action::SetStatus(_)
            | Action::SetPriority(_)
            | Action::CyclePriority(_)
            | Action::CopyShortRef
            | Action::CopyDurableRef
            | Action::BeginEditTitle
            | Action::BeginEditDescription
            | Action::BeginEditProject
            | Action::BeginEditPriority
            | Action::BeginEditLabels
            | Action::Delete
            | Action::Restore
            | Action::BeginStatusPicker
            | Action::BeginRenameProject
            | Action::BeginDeleteProject
            | Action::BeginAddTask
            | Action::BeginAddNote
            | Action::BeginAddProject
            | Action::BeginAddLabel
            | Action::BeginFilterLabel
            | Action::BeginFilterPriority
            | Action::BeginScopeProject
            | Action::BeginSwitchWorkspace
            | Action::ClearFilters
            | Action::ToggleDeletedFilter
            | Action::ShowView(_)
            | Action::ShowWorkspaceScope
            | Action::BeginConflictList
            | Action::ShowConflictDetails
            | Action::NextConflict
            | Action::PreviousConflict
            | Action::AcceptConflictLocal
            | Action::AcceptConflictRemote
            | Action::BeginManualConflictMerge
            | Action::ShowConfigStatus
            | Action::ShowConfigInfo
            | Action::ShowConfigPaths
            | Action::ShowDatabaseStats
            | Action::BeginConfigInit
            | Action::Undo
    )
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

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
    fn lookup_command_finds_prefixed_suffix() {
        assert_eq!(
            lookup_command(":todo"),
            CommandLookup::Found(Action::SetStatus("todo"))
        );
    }

    #[test]
    fn lookup_command_ignores_dashes() {
        assert_eq!(
            lookup_command(":statusin"),
            CommandLookup::Found(Action::SetStatus("inbox"))
        );
    }

    #[test]
    fn lookup_command_preserves_suffix_ambiguity() {
        assert_eq!(lookup_command(":done"), CommandLookup::Ambiguous);
    }

    #[test]
    fn complete_command_fills_unique_match() {
        assert_eq!(
            complete_command(":todo"),
            CommandCompletion::Completed("status-todo".to_string())
        );
    }

    #[test]
    fn complete_command_fills_dashless_match() {
        assert_eq!(
            complete_command(":statusin"),
            CommandCompletion::Completed("status-inbox".to_string())
        );
    }

    #[test]
    fn complete_command_reports_unchanged_for_ambiguous_match() {
        assert_eq!(complete_command("stat"), CommandCompletion::Unchanged);
    }

    #[test]
    fn command_cycle_options_keeps_lower_ranked_visible_matches() {
        assert_eq!(
            command_cycle_options("r"),
            vec![
                "refresh",
                "restore",
                "rename-project",
                "remove-project-path",
                "move-right",
                "copy-ref",
                "order-reverse",
                "conflict-use-remote"
            ]
        );
    }

    #[test]
    fn command_cycle_options_limits_exact_match_to_exact_commands() {
        assert_eq!(command_cycle_options("todo"), vec!["status-todo"]);
    }

    #[test]
    fn complete_command_reports_unchanged_when_fully_extended() {
        assert_eq!(complete_command("status-"), CommandCompletion::Unchanged);
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
            resolve_shortcut(&[KeyCode::Char('!')]),
            ShortcutLookup::Missing
        );
    }

    #[test]
    fn resolves_undo_shortcut() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('u')]),
            ShortcutLookup::Found(Action::Undo)
        );
        assert_eq!(lookup_command("undo"), CommandLookup::Found(Action::Undo));
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
    fn current_single_key_shortcuts_match_catalog() {
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
        assert_eq!(Action::from_normal_key(KeyCode::Char('u')), Action::Undo);
        assert_eq!(Action::from_normal_key(KeyCode::Char('z')), Action::None);
        assert_eq!(Action::from_normal_key(KeyCode::Char('g')), Action::None);
        assert_eq!(Action::from_normal_key(KeyCode::Char('v')), Action::None);
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
                (
                    CommandLifecycle::Planned { reason },
                    Action::Planned {
                        name,
                        reason: action_reason,
                    },
                ) => {
                    assert_eq!(name, command.name);
                    assert_eq!(reason, action_reason);
                    assert!(
                        !reason.trim().is_empty(),
                        ":{} planned reason is empty",
                        command.name
                    );
                }
                (
                    CommandLifecycle::Disabled { reason },
                    Action::Disabled {
                        name,
                        reason: action_reason,
                    },
                ) => {
                    assert_eq!(name, command.name);
                    assert_eq!(reason, action_reason);
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
    fn implemented_detail_actions_are_handled() {
        for command in CommandContext::Detail.commands() {
            assert!(
                implemented_action_is_handled(command.action),
                "implemented detail command :{} is not handled",
                command.name
            );
        }
    }

    #[test]
    fn command_contexts_reject_duplicate_exact_shortcuts() {
        for context in [CommandContext::Normal, CommandContext::Detail] {
            let mut seen: Vec<(&[KeyCode], &str, &str)> = Vec::new();
            for command in context.commands() {
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
    }

    #[test]
    fn detail_context_resolves_detail_shortcuts() {
        assert_eq!(
            resolve_shortcut_for(CommandContext::Detail, &[KeyCode::Char('e')]),
            ShortcutLookup::Prefix
        );
        assert_eq!(
            resolve_shortcut_for(
                CommandContext::Detail,
                &[KeyCode::Char('e'), KeyCode::Char('t')]
            ),
            ShortcutLookup::Found(Action::BeginEditTitle)
        );
        assert_eq!(
            resolve_shortcut_for(CommandContext::Detail, &[KeyCode::Char('l')]),
            ShortcutLookup::Found(Action::BeginEditLabels)
        );
    }

    #[test]
    fn command_domains_cover_catalog_sections() {
        let mut offset = 0;
        for domain in catalog::COMMAND_DOMAINS {
            let commands = domain.commands();
            assert!(
                !commands.is_empty(),
                "empty command domain {}",
                domain.section
            );
            assert_eq!(domain.start, offset);
            assert_eq!(domain.end, offset + commands.len());
            assert!(
                commands
                    .iter()
                    .all(|command| command.section == domain.section),
                "domain {} contains another section",
                domain.section
            );
            offset = domain.end;
        }
        assert_eq!(offset, COMMANDS.len());
    }

    #[test]
    fn resolves_metadata_shortcuts() {
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('A')]),
            ShortcutLookup::Prefix
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('A'), KeyCode::Char('p')]),
            ShortcutLookup::Found(Action::BeginAddProject)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('A'), KeyCode::Char('l')]),
            ShortcutLookup::Found(Action::BeginAddLabel)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('A'), KeyCode::Char('e')]),
            ShortcutLookup::Found(Action::BeginRenameProject)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('A'), KeyCode::Char('d')]),
            ShortcutLookup::Found(Action::BeginDeleteProject)
        ));
    }

    #[test]
    fn resolves_authoring_shortcuts() {
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('a')]),
            ShortcutLookup::Found(Action::BeginAddTask)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('A'), KeyCode::Char('t')]),
            ShortcutLookup::Found(Action::BeginAddTask)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('n')]),
            ShortcutLookup::Found(Action::BeginAddNote)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('A'), KeyCode::Char('n')]),
            ShortcutLookup::Found(Action::BeginAddNote)
        ));
    }

    #[test]
    fn project_path_commands_remain_planned_with_explicit_reason() {
        for name in ["add-project-path", "remove-project-path"] {
            let command = COMMANDS
                .iter()
                .find(|command| command.name == name)
                .unwrap_or_else(|| panic!("missing command :{name}"));
            assert_eq!(
                command.lifecycle,
                CommandLifecycle::Planned {
                    reason: PROJECT_PATH_FLOW_REASON
                }
            );
        }
    }

    #[test]
    fn required_action_families_are_present() {
        for name in [
            "add-task",
            "edit-title",
            "status-picker",
            "copy-ref",
            "copy-id",
            "status-active",
            "scope-project",
            "scope-all",
            "order-due",
            "order-priority",
            "order-reverse",
            "conflict-list",
            "add-project",
            "rename-project",
            "delete-project",
            "config-show",
            "config-status",
            "config-paths",
            "database-stats",
            "config-init",
        ] {
            assert!(
                COMMANDS.iter().any(|command| command.name == name),
                "missing required command :{name}"
            );
        }
    }

    #[test]
    fn resolves_task_editing_shortcuts() {
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('E')]),
            ShortcutLookup::Prefix
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('E'), KeyCode::Char('t')]),
            ShortcutLookup::Found(Action::BeginEditTitle)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('E'), KeyCode::Char('d')]),
            ShortcutLookup::Found(Action::BeginEditDescription)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('E'), KeyCode::Char('p')]),
            ShortcutLookup::Found(Action::BeginEditProject)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('e'), KeyCode::Char('t')]),
            ShortcutLookup::Found(Action::BeginEditTitle)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('e'), KeyCode::Char('d')]),
            ShortcutLookup::Found(Action::BeginEditDescription)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('e'), KeyCode::Char('p')]),
            ShortcutLookup::Found(Action::BeginEditProject)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('p')]),
            ShortcutLookup::Found(Action::BeginEditPriority)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('e'), KeyCode::Char('r')]),
            ShortcutLookup::Found(Action::BeginEditPriority)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('e'), KeyCode::Char('l')]),
            ShortcutLookup::Found(Action::BeginEditLabels)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('y')]),
            ShortcutLookup::Found(Action::CopyShortRef)
        ));
        assert!(matches!(
            resolve_shortcut(&[KeyCode::Char('Y')]),
            ShortcutLookup::Found(Action::CopyDurableRef)
        ));
    }

    #[test]
    fn resolves_exact_priority_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m'), KeyCode::Char('0')]),
            ShortcutLookup::Found(Action::SetPriority("none"))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m'), KeyCode::Char('l')]),
            ShortcutLookup::Found(Action::SetPriority("low"))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m'), KeyCode::Char('m')]),
            ShortcutLookup::Found(Action::SetPriority("medium"))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m'), KeyCode::Char('h')]),
            ShortcutLookup::Found(Action::SetPriority("high"))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m'), KeyCode::Char('u')]),
            ShortcutLookup::Found(Action::SetPriority("urgent"))
        );
    }

    #[test]
    fn status_shortcuts_resolve_through_mark_prefix() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('s')]),
            ShortcutLookup::Found(Action::BeginStatusPicker)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('d')]),
            ShortcutLookup::Found(Action::SetStatus("done"))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('x')]),
            ShortcutLookup::Found(Action::SetStatus("canceled"))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m'), KeyCode::Char('i')]),
            ShortcutLookup::Found(Action::SetStatus("inbox"))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('m'), KeyCode::Char('d')]),
            ShortcutLookup::Found(Action::SetStatus("done"))
        );
    }

    #[test]
    fn resolves_filter_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('f'), KeyCode::Char('l')]),
            ShortcutLookup::Found(Action::BeginFilterLabel)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('f'), KeyCode::Char('r')]),
            ShortcutLookup::Found(Action::BeginFilterPriority)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('f'), KeyCode::Char('c')]),
            ShortcutLookup::Found(Action::ClearFilters)
        );
    }

    #[test]
    fn resolves_view_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('v')]),
            ShortcutLookup::Prefix
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('v'), KeyCode::Char('q')]),
            ShortcutLookup::Found(Action::ShowView(crate::tui::store::TaskView::Queue))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('v'), KeyCode::Char('c')]),
            ShortcutLookup::Found(Action::ShowView(crate::tui::store::TaskView::Conflicts))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('g'), KeyCode::Char('p')]),
            ShortcutLookup::Found(Action::BeginScopeProject)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('g'), KeyCode::Char('s')]),
            ShortcutLookup::Found(Action::ShowWorkspaceScope)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('g'), KeyCode::Char('w')]),
            ShortcutLookup::Found(Action::BeginSwitchWorkspace)
        );
    }

    #[test]
    fn resolves_conflict_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('c'), KeyCode::Char('l')]),
            ShortcutLookup::Found(Action::BeginConflictList)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('c'), KeyCode::Char('s')]),
            ShortcutLookup::Found(Action::ShowConflictDetails)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('c'), KeyCode::Char('n')]),
            ShortcutLookup::Found(Action::NextConflict)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('c'), KeyCode::Char('p')]),
            ShortcutLookup::Found(Action::PreviousConflict)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('c'), KeyCode::Char('a')]),
            ShortcutLookup::Found(Action::AcceptConflictLocal)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('c'), KeyCode::Char('r')]),
            ShortcutLookup::Found(Action::AcceptConflictRemote)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('c'), KeyCode::Char('m')]),
            ShortcutLookup::Found(Action::BeginManualConflictMerge)
        );
    }

    #[test]
    fn resolves_config_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('?')]),
            ShortcutLookup::Found(Action::ToggleHelp)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('h')]),
            ShortcutLookup::Found(Action::MoveLeft)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('C')]),
            ShortcutLookup::Prefix
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('C'), KeyCode::Char('s')]),
            ShortcutLookup::Found(Action::ShowConfigStatus)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('C'), KeyCode::Char('c')]),
            ShortcutLookup::Found(Action::ShowConfigInfo)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('C'), KeyCode::Char('d')]),
            ShortcutLookup::Found(Action::ShowConfigPaths)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('C'), KeyCode::Char('D')]),
            ShortcutLookup::Found(Action::ShowDatabaseStats)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('C'), KeyCode::Char('i')]),
            ShortcutLookup::Found(Action::BeginConfigInit)
        );
    }

    #[test]
    fn non_executing_lifecycle_shortcuts_resolve_to_catalog_action() {
        for command in COMMANDS {
            if !matches!(command.lifecycle, CommandLifecycle::Implemented) {
                for key in command.keys {
                    assert_eq!(
                        resolve_shortcut(key.codes),
                        ShortcutLookup::Found(command.action),
                        "shortcut {} for :{} resolved incorrectly",
                        key.label,
                        command.name
                    );
                }
            }
        }
    }

    #[test]
    fn non_executing_lifecycle_commands_resolve_to_catalog_action() {
        for command in COMMANDS {
            if !matches!(command.lifecycle, CommandLifecycle::Implemented) {
                assert_eq!(
                    lookup_command(command.name),
                    CommandLookup::Found(command.action)
                );
            }
        }
    }

    #[test]
    fn resolves_order_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('o'), KeyCode::Char('d')]),
            ShortcutLookup::Found(Action::Disabled {
                name: "order-due",
                reason: DUE_SORT_REASON,
            })
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('o'), KeyCode::Char('c')]),
            ShortcutLookup::Found(Action::SetOrder(crate::tui::store::TaskOrder::Created))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('o'), KeyCode::Char('p')]),
            ShortcutLookup::Found(Action::SetOrder(crate::tui::store::TaskOrder::Priority))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('o'), KeyCode::Char('u')]),
            ShortcutLookup::Found(Action::SetOrder(crate::tui::store::TaskOrder::Updated))
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('o'), KeyCode::Char('r')]),
            ShortcutLookup::Found(Action::ReverseSort)
        );
    }
}
