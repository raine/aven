use super::*;

#[test]
fn picker_navigation_mode_hides_filter_input() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        title: "Project".to_string(),
        filter: "needle".to_string(),
        filter_cursor: 6,
        items: borrow_slice(vec![picker_item("APP app", "app")]),
        multi: true,
        visible_indices: vec![0],
        ..picker_view()
    }));
    assert!(rendered.contains("Project"));
    assert!(!rendered.contains("needle"));
    assert!(rendered.contains("j/k"));
    assert!(rendered.contains("/ filter"));
    assert!(rendered.contains("Space"));
    assert!(rendered.contains("toggle"));
}

#[test]
fn picker_filter_mode_hints_show_text_entry() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        title: "Project".to_string(),
        filter: "app".to_string(),
        filter_cursor: 3,
        items: borrow_slice(vec![picker_item("APP app", "app")]),
        mode: PickerMode::Filter,
        visible_indices: vec![0],
        ..picker_view()
    }));
    assert!(rendered.contains("/app"));
    assert!(rendered.contains("type filter"));
    assert!(rendered.contains("Esc normal"));
}

#[test]
fn workspace_picker_shows_direct_filter_hints_and_no_match_state() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        kind: PickerKind::SwitchWorkspace,
        title: "Switch workspace".to_string(),
        filter: "missing".to_string(),
        filter_cursor: 7,
        mode: PickerMode::Filter,
        ..picker_view()
    }));

    assert!(rendered.contains("/missing"));
    assert!(rendered.contains("no matching workspaces"));
    assert!(rendered.contains("type filter"));
    assert!(rendered.contains("Up/Down move"));
    assert!(rendered.contains("Esc cancel"));
    assert!(rendered.contains("Enter switch"));
}

#[test]
fn blocker_picker_styles_task_reference_and_title() {
    let item = PickerItem {
        label: "docs  DCS-5283  BNV Multi Blocker Alpha".to_string(),
        value: "blocker-id".to_string(),
        selected: false,
    };

    let line = super::picker::blocker_picker_line(&Theme::dark(), &item, false, 8);

    assert_eq!(line.spans[1].content, "DCS");
    assert_eq!(line.spans[1].style.fg, Some(Theme::dark().project("docs")));
    assert_eq!(line.spans[2].content, "-");
    assert_eq!(line.spans[2].style.fg, Some(Theme::dark().fg_dim));
    assert_eq!(line.spans[3].content, "5283");
    assert_eq!(line.spans[3].style.fg, Some(Theme::dark().fg_dim));
    assert_eq!(line.spans[5].content, "BNV Multi Blocker Alpha");
    assert_eq!(line.spans[5].style.fg, Some(Theme::dark().fg_muted));

    let selected = super::picker::blocker_picker_line(&Theme::dark(), &item, true, 8);
    assert_eq!(selected.spans[0].content, "▸ ");
    assert_eq!(selected.spans[5].style.fg, Some(Theme::dark().fg));
}

#[test]
fn blocker_picker_aligns_titles_after_different_ref_widths() {
    let docs = PickerItem {
        label: "docs  DCS-NA0G  Docs blocker".to_string(),
        value: "docs-blocker".to_string(),
        selected: false,
    };
    let mobile = PickerItem {
        label: "mobile-app  MA-964N  Mobile blocker".to_string(),
        value: "mobile-blocker".to_string(),
        selected: false,
    };

    let docs_line = super::picker::blocker_picker_line(&Theme::dark(), &docs, false, 8);
    let mobile_line = super::picker::blocker_picker_line(&Theme::dark(), &mobile, false, 8);
    let title_column = |line: &Line<'_>| {
        line.spans[..line.spans.len() - 1]
            .iter()
            .map(|span| span.width())
            .sum::<usize>()
    };

    assert_eq!(title_column(&docs_line), title_column(&mobile_line));
    assert_eq!(title_column(&docs_line), 12);
}

#[test]
fn blocker_picker_kind_uses_structured_task_renderer() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        kind: PickerKind::GoToBlocker,
        title: "Go to blocker".to_string(),
        items: borrow_slice(vec![picker_item(
            "docs  DCS-5283  BNV Multi Blocker Alpha",
            "blocker-id",
        )]),
        visible_indices: vec![0],
        ..picker_view()
    }));

    assert!(rendered.contains("DCS-5283  BNV Multi Blocker Alpha"));
    assert!(!rendered.contains("docs  DCS"));
}

#[test]
fn picker_viewport_uses_scroll_position() {
    let items = (0..12)
        .map(|index| PickerItem {
            label: format!("Item {index}"),
            value: index.to_string(),
            selected: false,
        })
        .collect::<Vec<_>>();
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        title: "Project".to_string(),
        items: borrow_slice(items),
        selected: 10,
        scroll: 3,
        visible_indices: (0..12).collect(),
        ..picker_view()
    }));
    assert!(rendered.contains("▸ Item 10"));
    assert!(rendered.contains("Item 3"));
    assert!(!rendered.contains("Item 0"));
}

#[test]
fn label_picker_aligns_wide_labels_by_cells() {
    let item = PickerItem {
        label: "한글  2 tasks  3 recurring series".to_string(),
        value: "한글".to_string(),
        selected: false,
    };

    let line = super::picker::label_picker_line(&Theme::dark(), &item, false);

    assert_eq!(line.spans[1].width(), 30);
    assert_eq!(line.width(), 58);
}

#[test]
fn project_creation_suggestion_keeps_stable_prefix_color() {
    let suggestion = |name: &str| PickerItem {
        label: format!("+ Create project \"{name}\""),
        value: format!(
            "{}{}",
            crate::tui::store::CREATE_PROJECT_PICKER_VALUE_PREFIX,
            name
        ),
        selected: false,
    };

    let first = super::picker::project_picker_line(&Theme::dark(), &suggestion("M"), false);
    let second =
        super::picker::project_picker_line(&Theme::dark(), &suggestion("Mobile App"), false);

    assert_eq!(first.spans[1].style.fg, second.spans[1].style.fg);
    assert_eq!(
        first.spans[1].style.fg,
        Some(Theme::dark().project(crate::tui::store::CREATE_PROJECT_PICKER_VALUE_PREFIX))
    );
}

#[test]
fn project_scope_picker_shows_single_escape_cancellation() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        mode: PickerMode::Filter,
        ..project_picker_view()
    }));

    assert!(rendered.contains("type filter"));
    assert!(rendered.contains("Esc cancel"));
    assert!(!rendered.contains("Esc normal"));
}

#[test]
fn label_picker_uses_structured_usage_columns() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        kind: PickerKind::LabelAdministration,
        title: "Labels".to_string(),
        items: borrow_slice(vec![
            picker_item("backend  6 tasks  2 recurring series", "backend"),
            picker_item("bug  1 task  0 recurring series", "bug"),
        ]),
        visible_indices: vec![0, 1],
        mode: PickerMode::Filter,
        ..picker_view()
    }));

    assert!(rendered.contains("LABEL"));
    assert!(rendered.contains("TASKS"));
    assert!(rendered.contains("RECURRING SERIES"));
    assert!(rendered.contains("backend"));
    assert!(rendered.contains("▸ backend"));
    assert!(!rendered.contains("6 tasks"));
    assert!(!rendered.contains("2 recurring series"));
    assert!(rendered.contains("Enter choose"));
}

#[test]
fn tag_combobox_shows_selected_labels_input_completion_and_matches() {
    let rendered = render_overlay_view(OverlayView::TagCombobox(Box::new(TagComboboxView {
        kind: TagComboboxKind::EditLabels,
        title: "Edit task: labels".to_string(),
        input: "bu".to_string(),
        input_cursor: 2,
        completion: Some("g".to_string()),
        options: borrow_slice(vec!["bug".to_string(), "feature".to_string()]),
        selected: borrow_slice(vec!["feature".to_string()]),
        partial: &[],
        highlighted: 0,
        visible_indices: vec![0],
        visible_start: 0,
    })));

    assert!(rendered.contains("Edit task: labels"));
    assert!(rendered.contains("feature"));
    assert!(rendered.contains("bu"));
    assert!(rendered.contains("bug"));
    assert!(rendered.contains("Tab/Space add"));
    assert!(rendered.contains("Enter add/save"));
    assert!(rendered.contains("^S save"));
}

#[test]
fn tag_combobox_shows_partial_label_membership() {
    let rendered = render_overlay_view(OverlayView::TagCombobox(Box::new(TagComboboxView {
        kind: TagComboboxKind::EditLabelsMulti,
        title: "Edit labels · 2 marked tasks".to_string(),
        input: String::new(),
        input_cursor: 0,
        completion: None,
        options: borrow_slice(vec!["urgent".to_string()]),
        selected: &[],
        partial: borrow_slice(vec!["urgent".to_string()]),
        highlighted: 0,
        visible_indices: vec![0],
        visible_start: 0,
    })));

    assert!(rendered.contains("~ urgent"));
}

#[test]
fn picker_rendered_rows_are_mouse_targets() {
    use crate::tui::overlay::{OverlayOutcome, handle_generic_overlay_mouse, picker_layout};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    for mode in [PickerMode::Navigate, PickerMode::Filter] {
        for (count, filter) in [(0usize, ""), (1, ""), (3, ""), (12, ""), (12, "Item 1")] {
            for (width, height) in [(80, 24), (24, 9)] {
                let state = PickerState {
                    intent: PickerIntent::FilterLabel,
                    title: "Pick".to_string(),
                    filter: LineEdit::new(filter.to_string()),
                    items: (0..count)
                        .map(|i| picker_item(&format!("Item {i:02}"), &i.to_string()))
                        .collect(),
                    selected: count.saturating_sub(1),
                    scroll: count.saturating_sub(8),
                    multi: true,
                    mode,
                };
                let view = PickerView::from_state(&state);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| render_picker(frame, &Theme::dark(), &view))
                    .unwrap();
                let size = ratatui::layout::Size::new(width, height);
                let layout = picker_layout(&view, size);
                let click = |column, row| MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                };
                for row in layout.list.y..layout.list.bottom() {
                    let text = (layout.inner.x..layout.inner.right())
                        .map(|x| terminal.backend().buffer()[(x, row)].symbol())
                        .collect::<String>();
                    if let Some(index) = layout.item_at(layout.list.x, row, &view.visible_indices) {
                        assert!(text.contains(&format!("Item {index:02}")), "{text}");
                        let outcome = handle_generic_overlay_mouse(
                            OverlayState::Picker(state.clone()),
                            click(layout.list.x, row),
                            size,
                        );
                        let OverlayOutcome::None(OverlayState::Picker(clicked)) = outcome else {
                            panic!("expected retained multi picker");
                        };
                        assert_eq!(clicked.selected, index);
                        assert!(clicked.items[index].selected);
                    } else {
                        assert!(!text.contains("Item"));
                    }
                }
                assert_eq!(
                    handle_generic_overlay_mouse(OverlayState::Picker(state), click(0, 0), size),
                    OverlayOutcome::Cancelled
                );
            }
        }
    }
}

#[test]
fn priority_pickers_share_icons_colors_highlight_and_typing_hint() {
    for intent in [
        crate::tui::overlay::PickerIntent::FilterPriority,
        crate::tui::overlay::PickerIntent::AddTaskPriority,
    ] {
        let kind = PickerKind::from(&intent);
        assert_eq!(kind, PickerKind::Priority);
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            kind,
            title: "Priority".to_string(),
            items: borrow_slice(vec![picker_item("urgent", "urgent")]),
            visible_indices: vec![0],
            ..picker_view()
        }));
        assert!(rendered.contains("▲ urgent"));
        assert!(rendered.contains("type filter"));
        assert!(!rendered.contains("/ filter"));
    }
    for theme in [Theme::dark(), Theme::light(), Theme::terminal()] {
        for value in ["none", "low", "medium", "high", "urgent"] {
            let item = picker_item(value, value);
            let selected = super::picker::priority_picker_line(&theme, &item, true);
            assert_eq!(selected.style, theme.selected());
            assert_eq!(
                selected.spans[1].content,
                format!("{} ", priority_icon(value))
            );
            assert_eq!(selected.spans[1].style.fg, theme.priority(value).fg);
            assert_eq!(selected.spans[2].style.fg, Some(theme.fg));
            let plain = super::picker::priority_picker_line(&theme, &item, false);
            assert_ne!(plain.style, selected.style);
        }
    }
}

#[test]
fn priority_highlight_fills_row_and_empty_message_keeps_footer() {
    let theme = Theme::dark();
    let mut state = PickerView {
        kind: PickerKind::Priority,
        items: borrow_slice(vec![picker_item("none", "none"), picker_item("low", "low")]),
        visible_indices: vec![0, 1],
        ..picker_view()
    };
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let layout = crate::tui::overlay::picker_layout(&state, ratatui::layout::Size::new(100, 30));
    terminal
        .draw(|frame| render_picker(frame, &theme, &state))
        .unwrap();
    let buffer = terminal.backend().buffer();
    for x in layout.list.x..layout.list.right() {
        assert_eq!(buffer[(x, layout.list.y)].bg, theme.bg_selected);
        assert_ne!(buffer[(x, layout.list.y + 1)].bg, theme.bg_selected);
    }
    state.mode = PickerMode::Filter;
    state.filter = "invalid".to_string();
    state.filter_cursor = 7;
    state.visible_indices.clear();
    terminal
        .draw(|frame| render_picker(frame, &theme, &state))
        .unwrap();
    let rendered = buffer_text(terminal.backend());
    assert!(rendered.contains("no matching priorities"));
    assert!(rendered.contains("Enter submit"));
}
