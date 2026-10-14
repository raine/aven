mod action;
mod catalog;
mod command_catalog;
mod command_query;
mod lookup;
mod session;

pub(crate) use self::action::{
    Action, BulkSupport, CommandScopePolicy, CommandTargetPolicy, RelationshipTargetPolicy,
    SINGLE_TASK_COPY_ACTIONS, SurfaceEffect,
};
#[allow(unused_imports)]
pub(crate) use self::catalog::{
    BuiltInCommand, COMMAND_DOMAINS, COMMANDS, CommandContext, CommandDomain, DetailFocusPolicy,
    KeySequence, detail_focus_for_action,
};
pub(crate) use self::command_catalog::{
    CatalogCommand, CatalogShortcutLookup, CommandCatalog, CommandHandler, RoutingDomain,
    command_match_rank as command_match_rank_for_query, focus_policy_compatible,
    validate_custom_command_keys,
};
pub(crate) use self::command_query::{
    CommandAvailability, CommandCandidate, CommandDisabled, CommandQuery, command_availability,
};
#[allow(unused_imports)]
pub(crate) use self::lookup::{
    ShortcutLookup, key_label, preferred_shortcut_label, prefix_hint_commands,
    resolve_shortcut_for, resolve_shortcut_in_for, shortcut_label,
};
pub(crate) use self::session::{
    CommandSessionSnapshot, CommandSituation, CommandSurfaceSnapshot, CommandWorkspaceSnapshot,
    DetailCommandFocus, ResolvedCommand, ResolvedCommandTarget, SidebarCommandTarget,
    list_situation, recurrence_list_situation,
};

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    fn resolve_shortcut(input: &[KeyCode]) -> ShortcutLookup {
        resolve_shortcut_for(CommandContext::Normal, input)
    }

    use crate::tui::store::TaskQuery;

    use super::*;

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
    fn task_actions_declare_bulk_target_behavior() {
        assert_eq!(Action::BeginEditProject.bulk_support(), BulkSupport::Batch);
        assert_eq!(
            Action::BeginEditTitle.bulk_support(),
            BulkSupport::SingleOnly("title")
        );
        assert_eq!(Action::CopyShortRef.bulk_support(), BulkSupport::Batch);
        assert_eq!(
            Action::CopyTaskMarkdown.bulk_support(),
            BulkSupport::SingleOnly("copy")
        );
        assert_eq!(Action::ClearMarks.bulk_support(), BulkSupport::BulkControl);
        assert_eq!(
            Action::BeginAddRelated.target_policy(),
            CommandTargetPolicy::Single("related link")
        );
        assert_eq!(
            Action::BeginRemoveRelated.target_policy(),
            CommandTargetPolicy::Relationship(RelationshipTargetPolicy::Related)
        );
        assert_eq!(
            Action::ShowView(TaskQuery::Open).bulk_support(),
            BulkSupport::NotTaskScoped
        );
    }

    #[test]
    fn resolves_multi_key_sequences_from_catalog() {
        let commands = [BuiltInCommand::implemented(
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
            resolve_shortcut_in_for(&commands, CommandContext::Normal, &[KeyCode::Char('a')]),
            ShortcutLookup::Prefix
        );
        assert_eq!(
            resolve_shortcut_in_for(
                &commands,
                CommandContext::Normal,
                &[KeyCode::Char('a'), KeyCode::Char('t')]
            ),
            ShortcutLookup::Found(Action::BeginSearch)
        );
    }

    #[test]
    fn context_uses_its_own_command_bindings() {
        let commands = [BuiltInCommand::implemented_with_detail_bindings(
            "contextual",
            &[],
            "contextual command",
            "Test",
            &[KeySequence {
                codes: &[KeyCode::Char('l')],
                label: "l",
            }],
            &[KeySequence {
                codes: &[KeyCode::Char('d')],
                label: "d",
            }],
            DetailFocusPolicy::ParentTask,
            Action::Refresh,
        )];

        assert_eq!(
            resolve_shortcut_in_for(&commands, CommandContext::Normal, &[KeyCode::Char('l')]),
            ShortcutLookup::Found(Action::Refresh)
        );
        assert_eq!(
            resolve_shortcut_in_for(&commands, CommandContext::Detail, &[KeyCode::Char('d')]),
            ShortcutLookup::Found(Action::Refresh)
        );
        assert_eq!(
            resolve_shortcut_in_for(&commands, CommandContext::Detail, &[KeyCode::Char('l')]),
            ShortcutLookup::Missing
        );
    }

    #[test]
    fn resolves_exact_prefix_ambiguity() {
        let commands = [
            BuiltInCommand::implemented(
                "single-g",
                "single g",
                "Test",
                &[KeySequence {
                    codes: &[KeyCode::Char('g')],
                    label: "g",
                }],
                Action::First,
            ),
            BuiltInCommand::implemented(
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
            resolve_shortcut_in_for(&commands, CommandContext::Normal, &[KeyCode::Char('g')]),
            ShortcutLookup::Ambiguous(Action::First)
        );
    }

    #[test]
    fn resolves_duplicate_exact_sequences_as_ambiguous() {
        let commands = [
            BuiltInCommand::implemented(
                "first-q",
                "first q",
                "Test",
                &[KeySequence {
                    codes: &[KeyCode::Char('q')],
                    label: "q",
                }],
                Action::Quit,
            ),
            BuiltInCommand::implemented(
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
            resolve_shortcut_in_for(&commands, CommandContext::Normal, &[KeyCode::Char('q')]),
            ShortcutLookup::Ambiguous(Action::Quit)
        );
    }

    #[test]
    fn resolver_reports_missing_and_empty_inputs() {
        assert_eq!(resolve_shortcut(&[]), ShortcutLookup::Missing);
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('~')]),
            ShortcutLookup::Missing
        );
    }

    #[test]
    fn catalog_includes_add_device_as_a_keyless_list_command() {
        let command = COMMANDS
            .iter()
            .find(|command| command.name == "add-device")
            .expect("add-device command");

        assert!(command.aliases.is_empty());
        assert_eq!(command.action, Action::AddDevice);
        assert_eq!(command.target_policy(), CommandTargetPolicy::None);
        assert_eq!(command.scope_policy(), CommandScopePolicy::ListOnly);
        assert!(command.keys(CommandContext::Normal).is_empty());
        assert!(command.is_available(CommandContext::Normal));
        assert!(!command.is_available(CommandContext::Detail));
    }

    #[test]
    fn layout_shortcut_toggles_while_explicit_commands_have_no_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('v'), KeyCode::Char('l')]),
            ShortcutLookup::Found(Action::ToggleLayout)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('v'), KeyCode::Char('v')]),
            ShortcutLookup::Missing
        );
        for name in ["layout-list", "layout-columns"] {
            let command = COMMANDS
                .iter()
                .find(|command| command.name == name)
                .unwrap();
            assert!(command.keys(CommandContext::Normal).is_empty());
        }
    }

    #[test]
    fn resolves_task_sharing_shortcuts() {
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('y'), KeyCode::Char('m')]),
            ShortcutLookup::Found(Action::CopyTaskMarkdown)
        );
        assert_eq!(
            resolve_shortcut_for(
                CommandContext::Detail,
                &[KeyCode::Char('t'), KeyCode::Char('g')],
            ),
            ShortcutLookup::Found(Action::BeginCreateTaskGist)
        );
        assert_eq!(
            resolve_shortcut(&[KeyCode::Char('t'), KeyCode::Char('g')]),
            ShortcutLookup::Missing
        );
    }

    #[test]
    fn current_single_key_shortcuts_match_catalog() {
        for command in COMMANDS {
            for key in command.keys(CommandContext::Normal) {
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

        assert_eq!(Action::from_normal_key(KeyCode::Down), Action::MoveDown);
        assert_eq!(Action::from_normal_key(KeyCode::Up), Action::MoveUp);
        assert_eq!(Action::from_normal_key(KeyCode::Esc), Action::CancelOverlay);
        assert_eq!(Action::from_normal_key(KeyCode::Char('u')), Action::Undo);
        assert_eq!(Action::from_normal_key(KeyCode::Char('z')), Action::None);
        assert_eq!(Action::from_normal_key(KeyCode::Char('g')), Action::None);
        assert_eq!(Action::from_normal_key(KeyCode::Char('t')), Action::None);
        assert_eq!(Action::from_normal_key(KeyCode::Char('v')), Action::None);
    }

    #[test]
    fn catalog_excludes_overlay_input_and_no_op_actions() {
        for command in COMMANDS {
            assert!(
                !matches!(
                    command.action,
                    Action::AcceptCommand
                        | Action::CancelCommand
                        | Action::BackspaceCommand
                        | Action::CommandChar(_)
                        | Action::AcceptSearch
                        | Action::CancelSearch
                        | Action::BackspaceSearch
                        | Action::SearchChar(_)
                        | Action::None
                ),
                "catalog command :{} uses overlay input or a no-op action",
                command.name
            );
        }
    }

    #[test]
    fn custom_command_namespace_remains_free_of_built_in_bindings() {
        for context in [CommandContext::Normal, CommandContext::Detail] {
            for command in context.commands() {
                for key in command.keys(context) {
                    assert_ne!(
                        key.codes.first(),
                        Some(&KeyCode::Char('z')),
                        ":{} occupies the custom command namespace",
                        command.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_catalog_binding_resolves_to_its_command() {
        for context in [CommandContext::Normal, CommandContext::Detail] {
            for command in context.commands() {
                if context == CommandContext::Detail
                    && matches!(
                        command.action,
                        Action::OpenAttachment | Action::SaveAttachment | Action::DeleteAttachment
                    )
                {
                    continue;
                }
                for key in command.keys(context) {
                    assert_eq!(
                        resolve_shortcut_for(context, key.codes),
                        ShortcutLookup::Found(command.action),
                        "{context:?} shortcut {} for :{}",
                        key.label,
                        command.name
                    );
                    for len in 1..key.codes.len() {
                        assert_eq!(
                            resolve_shortcut_for(context, &key.codes[..len]),
                            ShortcutLookup::Prefix,
                            "{context:?} prefix of {} for :{}",
                            key.label,
                            command.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn command_domains_cover_catalog_sections() {
        let mut covered = Vec::new();
        for domain in catalog::COMMAND_DOMAINS {
            let commands = domain.commands();
            assert!(
                !commands.is_empty(),
                "empty command domain {}",
                domain.section
            );
            assert!(
                commands
                    .iter()
                    .all(|command| command.section == domain.section),
                "domain {} contains another section",
                domain.section
            );
            covered.extend(commands.into_iter().map(|command| command.name));
        }
        for command in COMMANDS {
            assert!(
                covered.contains(&command.name),
                ":{} domain is not registered",
                command.name
            );
        }
    }
}
