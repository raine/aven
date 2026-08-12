use super::*;

#[test]
fn picker_navigation_mode_hides_filter_input() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        title: "Project".to_string(),
        filter: "needle".to_string(),
        filter_cursor: 6,
        items: vec![picker_item("APP app", "app")],
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
        items: vec![picker_item("APP app", "app")],
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
fn priority_picker_shows_priority_icons() {
    for (kind, title) in [
        (PickerKind::EditPriority, "Edit task: priority"),
        (PickerKind::AddTaskPriority, "Add task: priority"),
    ] {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            kind,
            title: title.to_string(),
            items: vec![picker_item("urgent", "urgent")],
            visible_indices: vec![0],
            ..picker_view()
        }));
        assert!(rendered.contains(priority_icon("urgent")));
        assert!(rendered.contains("urgent"));
        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("submit"));
    }
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
        items,
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

    let line = super::picker::label_picker_line(&item, false);

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

    let first = super::picker::project_picker_line(&suggestion("M"), false);
    let second = super::picker::project_picker_line(&suggestion("Mobile App"), false);

    assert_eq!(first.spans[1].style.fg, second.spans[1].style.fg);
    assert_eq!(
        first.spans[1].style.fg,
        Some(theme::project_color(
            crate::tui::store::CREATE_PROJECT_PICKER_VALUE_PREFIX
        ))
    );
}

#[test]
fn project_picker_uses_structured_columns() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        filter: "claude".to_string(),
        filter_cursor: 6,
        ..project_picker_view()
    }));
    assert!(rendered.contains("PREFIX"));
    assert!(rendered.contains("PROJECT"));
    assert!(rendered.contains("CC"));
    assert!(rendered.contains("claude-code"));
    assert!(!rendered.contains("/claude"));
    assert!(rendered.contains("Enter scope"));
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
        items: vec![
            picker_item("backend  6 tasks  2 recurring series", "backend"),
            picker_item("bug  1 task  0 recurring series", "bug"),
        ],
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
        options: vec!["bug".to_string(), "feature".to_string()],
        selected: vec!["feature".to_string()],
        partial: Vec::new(),
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
        options: vec!["urgent".to_string()],
        selected: Vec::new(),
        partial: vec!["urgent".to_string()],
        highlighted: 0,
        visible_indices: vec![0],
        visible_start: 0,
    })));

    assert!(rendered.contains("~ urgent"));
}

#[test]
fn edit_project_uses_structured_project_picker() {
    for (kind, title) in [
        (PickerKind::EditProject, "Edit project"),
        (PickerKind::AddTaskProject, "Add task: project"),
    ] {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            kind,
            title: title.to_string(),
            filter: "claude".to_string(),
            filter_cursor: 6,
            ..project_picker_view()
        }));
        assert!(rendered.contains("PREFIX"));
        assert!(rendered.contains("PROJECT"));
        assert!(rendered.contains("CC"));
        assert!(rendered.contains("claude-code"));
        assert!(rendered.contains("Enter submit"));
        assert!(rendered.contains(title));
    }
}

#[test]
fn project_path_project_selection_uses_structured_project_picker() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        kind: PickerKind::ProjectPathProject,
        title: "Add project path".to_string(),
        ..project_picker_view()
    }));

    assert!(rendered.contains("Add project path"));
    assert!(rendered.contains("PREFIX"));
    assert!(rendered.contains("PROJECT"));
    assert!(rendered.contains("CC"));
    assert!(rendered.contains("claude-code"));
    assert!(rendered.contains("Enter select"));
}
