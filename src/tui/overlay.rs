mod handlers;
mod multiline;
mod picker;
mod state;
mod text_input;
mod view;

pub(crate) use handlers::{handle_generic_overlay_key, handle_generic_overlay_paste};
#[cfg(test)]
pub(crate) use multiline::edit_multiline_input;
#[cfg(test)]
pub(crate) use picker::{normalize_picker_selection, visible_picker_indices};
pub(crate) use state::{
    AddTaskState, CommandState, ConfirmSubmitRoute, MultilineInputState, MultilineSubmitRoute,
    OverlayOutcome, OverlayRoute, OverlayState, OverlaySubmit, PickerItem, PickerMode,
    PickerSubmitRoute, TextPanelState, TextSubmitRoute,
};
#[cfg(test)]
pub(crate) use state::{ConfirmState, OverlaySubmitKind, PickerState, TextInputState};
pub(crate) use text_input::LineEdit;
pub(crate) use view::{
    AddTaskView, ConfirmView, MultilineInputView, OverlayView, PickerView, TextInputView,
    TextPanelView,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::authoring::AddTaskStep;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn add_task_state(focus: AddTaskStep) -> AddTaskState {
        AddTaskState {
            title: LineEdit::blank(),
            description: MultilineInputState::blank(
                OverlayRoute::AddTaskDescription,
                "Add task: description",
                "",
            ),
            focus,
            project: "aven".to_string(),
            status: "inbox".to_string(),
            priority: "none".to_string(),
        }
    }

    fn line_edit(input: &str, cursor: usize) -> LineEdit {
        LineEdit {
            text: input.to_string(),
            cursor,
        }
    }

    fn handle(key: KeyEvent, overlay: OverlayState) -> OverlayOutcome {
        handle_generic_overlay_key(key, overlay, 100)
    }

    fn handle_with_help_scroll_cap(
        key: KeyEvent,
        overlay: OverlayState,
        help_scroll_cap: u16,
    ) -> OverlayOutcome {
        handle_generic_overlay_key(key, overlay, help_scroll_cap)
    }

    #[test]
    fn text_input_edits_at_cursor() {
        let mut state = line_edit("ab", 1);
        state.handle_key(key(KeyCode::Char('x')));
        assert_eq!(state.text, "axb");
        assert_eq!(state.cursor, 2);
        state.handle_key(key(KeyCode::Backspace));
        assert_eq!(state.text, "ab");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn text_input_supports_emacs_navigation() {
        let mut state = line_edit("abc", 1);
        state.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(state.cursor, 0);
        state.handle_key(ctrl(KeyCode::Char('e')));
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('b')));
        assert_eq!(state.cursor, 2);
        state.handle_key(ctrl(KeyCode::Char('f')));
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn text_input_supports_emacs_deletion() {
        let mut state = line_edit("one two three", 7);
        state.handle_key(ctrl(KeyCode::Char('w')));
        assert_eq!(state.text, "one three");
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('k')));
        assert_eq!(state.text, "one");
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('u')));
        assert_eq!(state.text, "");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn text_input_ignores_control_chars_that_are_not_editing_keys() {
        let mut state = line_edit("ab", 1);
        state.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(state.text, "ab");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn multiline_input_splits_and_merges_lines() {
        let mut state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["ab".to_string()],
            row: 0,
            column: 1,
        };
        edit_multiline_input(&mut state, key(KeyCode::Enter));
        assert_eq!(state.lines, vec!["a".to_string(), "b".to_string()]);
        state.row = 1;
        state.column = 0;
        edit_multiline_input(&mut state, key(KeyCode::Backspace));
        assert_eq!(state.lines, vec!["ab".to_string()]);
    }

    #[test]
    fn multiline_input_supports_emacs_navigation() {
        let mut state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["abc".to_string(), "déf".to_string()],
            row: 0,
            column: 1,
        };
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('e')));
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('b')));
        assert_eq!(state.column, 2);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('f')));
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('n')));
        assert_eq!(state.row, 1);
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('a')));
        assert_eq!(state.column, 0);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('p')));
        assert_eq!(state.row, 0);
        assert_eq!(state.column, 0);
    }

    #[test]
    fn multiline_input_supports_emacs_deletion() {
        let mut state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["one two three".to_string()],
            row: 0,
            column: 7,
        };
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('w')));
        assert_eq!(state.lines, vec!["one three".to_string()]);
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('k')));
        assert_eq!(state.lines, vec!["one".to_string()]);
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('u')));
        assert_eq!(state.lines, vec![String::new()]);
        assert_eq!(state.column, 0);
    }

    #[test]
    fn multiline_ctrl_w_merges_previous_line_at_line_start() {
        let mut state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["one ".to_string(), "two three".to_string()],
            row: 1,
            column: 0,
        };
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('w')));
        assert_eq!(state.lines, vec!["two three".to_string()]);
        assert_eq!(state.row, 0);
        assert_eq!(state.column, 0);
    }

    #[test]
    fn multiline_delete_at_line_end_merges_next_line() {
        let mut state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["one".to_string(), "two".to_string()],
            row: 0,
            column: 3,
        };
        edit_multiline_input(&mut state, key(KeyCode::Delete));
        assert_eq!(state.lines, vec!["onetwo".to_string()]);
        assert_eq!(state.row, 0);
        assert_eq!(state.column, 3);
    }

    #[test]
    fn multiline_ignores_control_chars_that_are_not_editing_keys() {
        let mut state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["ab".to_string()],
            row: 0,
            column: 1,
        };
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('x')));
        assert_eq!(state.lines, vec!["ab".to_string()]);
        assert_eq!(state.column, 1);
    }

    #[test]
    fn multiline_long_line_navigation_keeps_byte_cursor_valid() {
        let mut state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["a".repeat(140), "é".to_string()],
            row: 0,
            column: 139,
        };
        edit_multiline_input(&mut state, key(KeyCode::Right));
        assert_eq!(state.column, 140);
        edit_multiline_input(&mut state, key(KeyCode::Down));
        assert_eq!(state.row, 1);
        assert_eq!(state.column, "é".len());
        edit_multiline_input(&mut state, key(KeyCode::Left));
        assert_eq!(state.column, 0);
        assert!(state.lines[state.row].is_char_boundary(state.column));
    }

    #[test]
    fn multiline_paste_preserves_newlines() {
        let mut state = MultilineInputState::blank(OverlayRoute::MessageOnly, "Notes", "Body");
        state.insert_paste("one\ntwo\r\nthree");
        assert_eq!(
            state.lines,
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
        assert_eq!(state.row, 2);
        assert_eq!(state.column, 5);
    }

    #[test]
    fn add_task_description_paste_preserves_newlines() {
        let outcome = handle_generic_overlay_paste(
            "one\ntwo",
            OverlayState::AddTask(add_task_state(AddTaskStep::Description)),
        );
        let OverlayState::AddTask(state) = outcome else {
            panic!("expected add task state");
        };
        assert_eq!(
            state.description.lines,
            vec!["one".to_string(), "two".to_string()]
        );
        assert_eq!(state.description.row, 1);
        assert_eq!(state.description.column, 3);
    }

    #[test]
    fn add_task_title_paste_flattens_newlines() {
        let outcome = handle_generic_overlay_paste(
            "one\ntwo",
            OverlayState::AddTask(add_task_state(AddTaskStep::Title)),
        );
        let OverlayState::AddTask(state) = outcome else {
            panic!("expected add task state");
        };
        assert_eq!(state.title.text, "one two");
        assert_eq!(state.title.cursor, 7);
    }

    #[test]
    fn multiline_ctrl_s_submits() {
        let state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Notes".to_string(),
            prompt: "Body".to_string(),
            lines: vec!["line".to_string()],
            row: 0,
            column: 4,
        };
        let outcome = handle(
            ctrl(KeyCode::Char('s')),
            OverlayState::MultilineInput(state),
        );
        assert!(matches!(
            outcome,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline { .. })
        ));
    }

    #[test]
    fn picker_filter_and_selection_normalize() {
        let mut state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: LineEdit::blank(),
            items: vec![
                PickerItem {
                    label: "Alpha".to_string(),
                    value: "a".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "Beta".to_string(),
                    value: "b".to_string(),
                    selected: false,
                },
            ],
            selected: 1,
            multi: false,
            mode: PickerMode::Navigate,
        };
        state.filter = LineEdit::new("alp".to_string());
        normalize_picker_selection(&mut state);
        assert_eq!(state.selected, 0);
        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_filter_ignores_dashes_in_labels() {
        let state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Go: project".to_string(),
            filter: LineEdit::new("gitsur".to_string()),
            items: vec![PickerItem {
                label: "GS git-surgeon".to_string(),
                value: "git-surgeon".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            mode: PickerMode::Navigate,
        };

        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_filter_preserves_dash_matching() {
        let state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: LineEdit::new("git-sur".to_string()),
            items: vec![PickerItem {
                label: "GS git-surgeon".to_string(),
                value: "git-surgeon".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            mode: PickerMode::Navigate,
        };

        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_moves_with_j_and_k_in_navigation_mode() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Char('j')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 1);
        assert_eq!(state.filter.as_str(), "");
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Char('k')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 0);
        assert_eq!(state.filter.as_str(), "");
    }

    #[test]
    fn picker_types_j_and_k_in_filter_mode() {
        let mut state = picker_navigation_state();
        state.mode = PickerMode::Filter;
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Char('j')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Char('k')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.filter.as_str(), "jk");
    }

    #[test]
    fn picker_slash_enters_filter_mode_and_esc_returns_to_navigation() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Char('/')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.mode, PickerMode::Filter);
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Esc), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.mode, PickerMode::Navigate);
    }

    #[test]
    fn picker_moves_with_arrow_keys() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Down), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 1);
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Up), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn picker_moves_with_ctrl_n_and_ctrl_p() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(ctrl(KeyCode::Char('n')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 1);
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(ctrl(KeyCode::Char('p')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 0);
    }

    fn picker_navigation_state() -> PickerState {
        PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: LineEdit::blank(),
            items: vec![
                PickerItem {
                    label: "Alpha".to_string(),
                    value: "a".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "Beta".to_string(),
                    value: "b".to_string(),
                    selected: false,
                },
            ],
            selected: 0,
            multi: false,
            mode: PickerMode::Navigate,
        }
    }

    #[test]
    fn text_panel_closes_on_enter_and_esc() {
        let state = TextPanelState {
            title: "Conflicts".to_string(),
            lines: vec!["field=title".to_string()],
            scroll: 0,
        };
        assert!(matches!(
            handle(key(KeyCode::Enter), OverlayState::TextPanel(state.clone())),
            OverlayOutcome::Cancelled
        ));
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::TextPanel(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn text_panel_scrolls_with_navigation_keys() {
        let state = TextPanelState {
            title: "Conflicts".to_string(),
            lines: vec!["one".to_string(), "two".to_string()],
            scroll: 0,
        };
        let OverlayOutcome::None(OverlayState::TextPanel(state)) =
            handle(key(KeyCode::Down), OverlayState::TextPanel(state))
        else {
            panic!("expected scrolled text panel");
        };
        assert_eq!(state.scroll, 1);
        let OverlayOutcome::None(OverlayState::TextPanel(state)) =
            handle(key(KeyCode::Up), OverlayState::TextPanel(state))
        else {
            panic!("expected scrolled text panel");
        };
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn detail_scrolls_with_line_navigation_keys() {
        let OverlayOutcome::None(OverlayState::Detail { scroll }) =
            handle(key(KeyCode::Char('j')), OverlayState::Detail { scroll: 0 })
        else {
            panic!("expected scrolled detail");
        };
        assert_eq!(scroll, 1);
        let OverlayOutcome::None(OverlayState::Detail { scroll }) =
            handle(key(KeyCode::Char('k')), OverlayState::Detail { scroll })
        else {
            panic!("expected scrolled detail");
        };
        assert_eq!(scroll, 0);
    }

    #[test]
    fn esc_cancels_all_generic_overlay_variants() {
        let overlays = vec![
            OverlayState::TextInput(TextInputState::new(
                OverlayRoute::MessageOnly,
                "Title",
                "Prompt",
                "value".to_string(),
            )),
            OverlayState::MultilineInput(MultilineInputState {
                route: OverlayRoute::MessageOnly,
                title: "Body".to_string(),
                prompt: "Prompt".to_string(),
                lines: vec!["value".to_string()],
                row: 0,
                column: 5,
            }),
            OverlayState::Picker(PickerState {
                route: OverlayRoute::MessageOnly,
                title: "Pick".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
                mode: PickerMode::Navigate,
            }),
            OverlayState::Confirm(ConfirmState {
                route: OverlayRoute::MessageOnly,
                title: "Confirm".to_string(),
                prompt: "Continue?".to_string(),
            }),
            OverlayState::TextPanel(TextPanelState {
                title: "Panel".to_string(),
                lines: vec!["line".to_string()],
                scroll: 0,
            }),
        ];

        for overlay in overlays {
            assert!(matches!(
                handle(key(KeyCode::Esc), overlay),
                OverlayOutcome::Cancelled
            ));
        }
    }

    #[test]
    fn help_scroll_stops_at_cap() {
        let OverlayOutcome::None(OverlayState::Help { scroll }) =
            handle_with_help_scroll_cap(key(KeyCode::Down), OverlayState::Help { scroll: 2 }, 2)
        else {
            panic!("expected help overlay state");
        };
        assert_eq!(scroll, 2);
    }

    #[test]
    fn confirm_yes_and_no() {
        let state = ConfirmState {
            route: OverlayRoute::MessageOnly,
            title: "Delete".to_string(),
            prompt: "Sure?".to_string(),
        };
        assert!(matches!(
            handle(
                key(KeyCode::Char('y')),
                OverlayState::Confirm(state.clone())
            ),
            OverlayOutcome::Submitted(OverlaySubmit::Confirm {
                route: OverlayRoute::MessageOnly,
                title,
                ..
            }) if title == "Delete"
        ));
        assert!(matches!(
            handle(key(KeyCode::Char('n')), OverlayState::Confirm(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn generic_submit_variants_propagate_route() {
        let text = handle(
            key(KeyCode::Enter),
            OverlayState::TextInput(TextInputState::new(
                OverlayRoute::AddProject,
                "Add project",
                "name:",
                "app".to_string(),
            )),
        );
        assert!(matches!(
            text,
            OverlayOutcome::Submitted(OverlaySubmit::Text {
                route: OverlayRoute::AddProject,
                ..
            })
        ));

        let multiline = handle(
            ctrl(KeyCode::Char('s')),
            OverlayState::MultilineInput(MultilineInputState {
                route: OverlayRoute::AddNote,
                title: "Add note".to_string(),
                prompt: "body:".to_string(),
                lines: vec!["note".to_string()],
                row: 0,
                column: 4,
            }),
        );
        assert!(matches!(
            multiline,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline {
                route: OverlayRoute::AddNote,
                ..
            })
        ));

        let picker = handle(
            key(KeyCode::Enter),
            OverlayState::Picker(PickerState {
                route: OverlayRoute::EditStatus,
                title: "Edit task: status".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "Todo".to_string(),
                    value: "todo".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
                mode: PickerMode::Navigate,
            }),
        );
        assert!(matches!(
            picker,
            OverlayOutcome::Submitted(OverlaySubmit::Picker {
                route: OverlayRoute::EditStatus,
                ..
            })
        ));
    }

    #[test]
    fn picker_builder_uses_first_selected_item() {
        let state = PickerState::new(
            OverlayRoute::EditLabels,
            "Labels",
            vec![
                PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "Two".to_string(),
                    value: "two".to_string(),
                    selected: true,
                },
            ],
            true,
        );

        assert_eq!(state.selected, 1);
        assert_eq!(state.filter, LineEdit::blank());
        assert_eq!(state.mode, PickerMode::Navigate);
        assert!(state.multi);
    }

    #[test]
    fn project_pickers_open_in_filter_mode() {
        for route in [
            OverlayRoute::AddTaskTitleProject,
            OverlayRoute::EditProject,
            OverlayRoute::ScopeProject,
            OverlayRoute::DeleteProjectPicker,
        ] {
            let state = PickerState::new(
                route,
                "Project",
                vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                false,
            );
            assert_eq!(state.mode, PickerMode::Filter);
        }
    }

    #[test]
    fn picker_builder_defaults_to_first_item() {
        let state = PickerState::new(
            OverlayRoute::EditStatus,
            "Status",
            vec![PickerItem {
                label: "One".to_string(),
                value: "one".to_string(),
                selected: false,
            }],
            false,
        );

        assert_eq!(state.selected, 0);
        assert_eq!(state.filter, LineEdit::blank());
        assert_eq!(state.mode, PickerMode::Navigate);
        assert!(!state.multi);
    }

    #[test]
    fn overlay_builders_preserve_text_multiline_and_confirm_metadata() {
        let OverlayState::TextInput(text) = OverlayState::text_input(
            OverlayRoute::EditTitle,
            "Edit title",
            "title:",
            "old".to_string(),
        ) else {
            panic!("expected text input");
        };
        assert_eq!(text.route, OverlayRoute::EditTitle);
        assert_eq!(text.title, "Edit title");
        assert_eq!(text.prompt, "title:");
        assert_eq!(text.input.as_str(), "old");

        let OverlayState::MultilineInput(multiline) = OverlayState::multiline_input(
            OverlayRoute::EditDescription,
            "Edit description",
            "body:",
            "a\nb".to_string(),
        ) else {
            panic!("expected multiline input");
        };
        assert_eq!(multiline.route, OverlayRoute::EditDescription);
        assert_eq!(multiline.title, "Edit description");
        assert_eq!(multiline.prompt, "body:");
        assert_eq!(multiline.lines, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(multiline.row, 1);
        assert_eq!(multiline.column, 1);

        let OverlayState::Confirm(confirm) =
            OverlayState::confirm(OverlayRoute::DeleteTaskConfirm, "Delete", "Sure?")
        else {
            panic!("expected confirm");
        };
        assert_eq!(confirm.route, OverlayRoute::DeleteTaskConfirm);
        assert_eq!(confirm.title, "Delete");
        assert_eq!(confirm.prompt, "Sure?");
    }

    #[test]
    fn overlay_view_projection_carries_routes() {
        let multiline = OverlayView::from(&OverlayState::MultilineInput(
            MultilineInputState::blank(OverlayRoute::AddNote, "Changed note title", "note body:"),
        ));
        assert!(matches!(
            multiline,
            OverlayView::MultilineInput(MultilineInputView {
                route: OverlayRoute::AddNote,
                ..
            })
        ));

        let picker = OverlayView::from(&OverlayState::Picker(PickerState {
            route: OverlayRoute::DeleteProjectPicker,
            title: "Changed delete title".to_string(),
            filter: LineEdit::blank(),
            items: vec![PickerItem {
                label: "AVN aven".to_string(),
                value: "aven".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            mode: PickerMode::Navigate,
        }));
        assert!(matches!(
            picker,
            OverlayView::Picker(PickerView {
                route: OverlayRoute::DeleteProjectPicker,
                ..
            })
        ));
    }
}
