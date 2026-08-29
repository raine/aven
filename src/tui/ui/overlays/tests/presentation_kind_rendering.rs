use super::*;
use time::OffsetDateTime;

#[test]
fn overlay_kinds_use_shared_dialog_chrome() {
    let overlays = [
        OverlayView::Search {
            input: "query".to_string(),
            cursor: 5,
            results: &[],
            selected: 0,
            total_matches: 12,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::Navigate,
        },
        add_task_overlay(AddTaskView {
            title: "ship dialogs".to_string(),
            title_cursor: 12,
            priority: "high".to_string(),
            ..add_task_view()
        }),
        OverlayView::TextInput(TextInputView {
            kind: TextInputKind::EditTitle,
            title: "Edit title".to_string(),
            prompt: "New title".to_string(),
            input: "alpha".to_string(),
            cursor: 5,
        }),
        OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Description".to_string(),
            prompt: "Body".to_string(),
            lines: borrow_slice(vec!["line one".to_string()]),
            row: 0,
            column: 4,
            mode: MultilineInputMode::Compose,
        }),
        OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: "app".to_string(),
            filter_cursor: 3,
            items: borrow_slice(vec![picker_item("APP app", "app")]),
            multi: true,
            visible_indices: vec![0],
            ..picker_view()
        }),
        OverlayView::TagCombobox(Box::new(TagComboboxView {
            kind: TagComboboxKind::EditLabels,
            title: "Labels".to_string(),
            input: String::new(),
            input_cursor: 0,
            completion: None,
            options: borrow_slice(vec!["bug".to_string()]),
            selected: &[],
            partial: &[],
            highlighted: 0,
            visible_indices: vec![0],
            visible_start: 0,
        })),
        OverlayView::Confirm(ConfirmView {
            title: "Delete".to_string(),
            prompt: "Delete task?".to_string(),
        }),
        OverlayView::TextPanel(TextPanelView {
            title: "Conflict details".to_string(),
            lines: borrow_slice(vec!["field=title".to_string()]),
            scroll: 0,
        }),
        OverlayView::SyncStatus(Box::new(SyncStatusView {
            state: SyncStatusState {
                details: false,
                scroll: 0,
            },
            status: borrow_value(TuiSyncStatus::default()),
            syncing: false,
            now: OffsetDateTime::UNIX_EPOCH,
        })),
    ];

    for (overlay, title) in overlays.into_iter().zip([
        "Search",
        "Add task",
        "Edit title",
        "Description",
        "Project",
        "Labels",
        "Delete",
        "Conflict details",
        CONFIG_STATUS_TITLE,
    ]) {
        assert_overlay_uses_dialog_chrome(overlay, title);
    }
}

#[test]
fn add_note_kind_uses_specialized_renderer_with_changed_title() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddNote,
        title: "Changed note title".to_string(),
        prompt: "note body:".to_string(),
        lines: borrow_slice(vec![String::new()]),
        row: 0,
        column: 0,
        mode: MultilineInputMode::Compose,
    }));
    assert!(rendered.contains("Changed note title"));
    assert!(rendered.contains("Enter note body here..."));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));
}

#[test]
fn edit_description_kind_uses_specialized_renderer_with_changed_title() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Changed description title".to_string(),
        prompt: String::new(),
        lines: borrow_slice(vec!["a".repeat(160)]),
        row: 0,
        column: 150,
        mode: MultilineInputMode::Compose,
    }));
    assert!(rendered.contains("Changed description title"));
    assert!(rendered.contains("Ctrl+X Ctrl+E editor"));
    assert!(rendered.contains("line 1/1"));
}

#[test]
fn add_task_description_kind_uses_specialized_renderer_with_changed_title() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddTaskDescription,
        title: "Changed add task description".to_string(),
        prompt: String::new(),
        lines: borrow_slice(vec![String::new()]),
        row: 0,
        column: 0,
        mode: MultilineInputMode::Compose,
    }));
    assert!(rendered.contains("Changed add task description"));
    assert!(rendered.contains("Optional details, links, or handoff context..."));
    assert!(rendered.contains("Enter newline"));
}

#[test]
fn project_picker_kinds_control_submit_hints_with_changed_titles() {
    for (kind, title, hint) in [
        (
            PickerKind::ScopeProject,
            "Changed scope title",
            "Enter scope",
        ),
        (
            PickerKind::EditProject,
            "Changed edit title",
            "Enter submit",
        ),
        (
            PickerKind::AddTaskProject,
            "Changed add-task project title",
            "Enter submit",
        ),
        (
            PickerKind::ProjectPathProject,
            "Changed project path title",
            "Enter select",
        ),
        (
            PickerKind::RenameProject,
            "Changed rename title",
            "Enter rename",
        ),
        (
            PickerKind::DeleteProject,
            "Changed delete title",
            "Enter delete",
        ),
    ] {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            kind,
            title: title.to_string(),
            items: borrow_slice(vec![picker_item("AVN aven", "aven")]),
            ..project_picker_view()
        }));
        assert!(rendered.contains(title), "{kind:?}");
        assert!(rendered.contains("PREFIX"), "{kind:?}");
        assert!(rendered.contains(hint), "{kind:?}");
    }
}

#[test]
fn priority_picker_kind_controls_icon_rendering_with_changed_title() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        kind: PickerKind::EditPriority,
        title: "Changed priority title".to_string(),
        items: borrow_slice(vec![picker_item("urgent", "urgent")]),
        visible_indices: vec![0],
        ..picker_view()
    }));
    assert!(rendered.contains("Changed priority title"));
    assert!(rendered.contains(priority_icon("urgent")));
}

#[test]
fn add_task_priority_kind_uses_priority_renderer() {
    let rendered = render_overlay_view(OverlayView::Picker(PickerView {
        kind: PickerKind::AddTaskPriority,
        title: "Changed add task priority".to_string(),
        items: borrow_slice(vec![picker_item("urgent", "urgent")]),
        visible_indices: vec![0],
        ..picker_view()
    }));
    assert!(rendered.contains("Changed add task priority"));
    assert!(rendered.contains(priority_icon("urgent")));
    assert!(rendered.contains("urgent"));
    assert!(rendered.contains("Enter submit"));
}
