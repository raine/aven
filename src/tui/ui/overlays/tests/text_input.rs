use super::*;

#[test]
fn overlay_render_includes_text_input_prompt_and_hints() {
    let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
        kind: TextInputKind::EditTitle,
        title: "Edit title".to_string(),
        prompt: "New title".to_string(),
        input: "alpha".to_string(),
        cursor: 5,
    }));
    assert!(rendered.contains("Edit title"));
    assert!(rendered.contains("New title"));
    assert!(rendered.contains("Enter submit"));
}

#[test]
fn mixed_date_edit_uses_keep_and_clear_hints() {
    let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
        kind: TextInputKind::EditDate,
        title: "Edit due date · 2 marked tasks".to_string(),
        prompt: "Current: varies\nType a date to set it on all tasks".to_string(),
        input: String::new(),
        cursor: 0,
    }));

    assert!(rendered.contains("Type a date to set it on all tasks"));
    assert!(rendered.contains("Enter keep"));
    assert!(rendered.contains("Ctrl+D clear dates"));
    assert!(rendered.contains("Esc cancel"));
}

#[test]
fn overlay_render_omits_empty_text_input_prompt() {
    let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
        kind: TextInputKind::EditTitle,
        title: "Edit title".to_string(),
        prompt: String::new(),
        input: "alpha".to_string(),
        cursor: 5,
    }));
    assert!(rendered.contains("Edit title"));
    assert!(rendered.contains("alpha"));
    assert!(!rendered.contains("title:"));
    assert!(rendered.contains("Enter submit"));
}

#[test]
fn delete_project_name_confirmation_separates_prompt_and_input() {
    let buffer = overlay_buffer(OverlayView::TextInput(TextInputView {
        kind: TextInputKind::ConfirmDeleteProject,
        title: "Delete project".to_string(),
        prompt: "Type blocked-test to delete project:".to_string(),
        input: "blocked-test".to_string(),
        cursor: 12,
    }));
    let prompt_row = (0..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("Type blocked-test"))
        .unwrap();
    let input_row = (prompt_row + 1..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("blocked-test"))
        .unwrap();

    assert_eq!(input_row, prompt_row + 2);
}

#[test]
fn delete_label_name_confirmation_renders_usage_without_clipping() {
    let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
        kind: TextInputKind::ConfirmDeleteLabel,
        title: "Delete label".to_string(),
        prompt: "Type review-needed to delete this label.\nUsed by: 2 tasks, 0 recurring series"
            .to_string(),
        input: String::new(),
        cursor: 0,
    }));

    assert!(rendered.contains("Type review-needed to delete this label."));
    assert!(rendered.contains("Used by: 2 tasks, 0 recurring series"));
}

#[test]
fn placeholder_text_input_kinds_use_placeholder_style() {
    for (kind, title, prompt, placeholder) in [
        (
            TextInputKind::AddProject,
            "Add project",
            "project name:",
            ADD_PROJECT_NAME_PLACEHOLDER,
        ),
        (
            TextInputKind::AddLabel,
            "Add label",
            "label name:",
            ADD_LABEL_NAME_PLACEHOLDER,
        ),
        (
            TextInputKind::RenameLabel,
            "Rename label",
            "new label name:",
            RENAME_LABEL_NAME_PLACEHOLDER,
        ),
        (
            TextInputKind::AddWorkspace,
            "Create workspace",
            "workspace name:",
            ADD_WORKSPACE_NAME_PLACEHOLDER,
        ),
        (
            TextInputKind::RenameWorkspace,
            "Rename workspace",
            "new workspace name:",
            RENAME_WORKSPACE_NAME_PLACEHOLDER,
        ),
        (
            TextInputKind::RenameProject,
            "Rename project",
            "new project name:",
            RENAME_PROJECT_NAME_PLACEHOLDER,
        ),
        (
            TextInputKind::ConflictManual,
            "Resolve manually",
            "manual value for field=title:",
            CONFLICT_MANUAL_VALUE_PLACEHOLDER,
        ),
    ] {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
            kind,
            title: title.to_string(),
            prompt: prompt.to_string(),
            input: String::new(),
            cursor: 0,
        }));
        assert!(rendered.contains(title), "{kind:?}");
        assert!(rendered.contains(placeholder), "{kind:?}");
        assert!(!rendered.contains(prompt), "{kind:?}");
        assert!(rendered.contains("Enter submit"), "{kind:?}");
        if matches!(
            kind,
            TextInputKind::RenameLabel
                | TextInputKind::RenameProject
                | TextInputKind::RenameWorkspace
        ) {
            assert!(rendered.contains("Ctrl+U clear"), "{kind:?}");
        }
    }
}

#[test]
fn project_path_input_marks_truncated_side() {
    let input = "/Users/raine/code/aven/worktree";

    let end = project_path_input_line(input, input.len(), 16);
    assert_eq!(end.spans.first().unwrap().content.as_ref(), "…");
    assert!(end.to_string().ends_with("worktree "));

    let start = project_path_input_line(input, 0, 16);
    assert_eq!(start.spans.last().unwrap().content.as_ref(), "…");
    assert!(start.to_string().starts_with('/'));
}

#[test]
fn project_path_input_uses_wide_labeled_dialog() {
    let input = "/Users/raine/code/aven__worktrees/fix-item-20-project-paths/nested/project-paths";
    let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
        kind: TextInputKind::ProjectPath,
        title: "Add project path".to_string(),
        prompt: "directory path for mobile-app:".to_string(),
        input: input.to_string(),
        cursor: input.len(),
    }));

    assert!(rendered.contains("Add project path"));
    assert!(rendered.contains("directory path for mobile-app:"));
    assert!(rendered.contains('…'));
    assert!(rendered.contains("fix-item-20-project-paths"));
}

#[test]
fn empty_placeholder_text_input_shows_placeholder() {
    let line = placeholder_text_input_line("", 0, 20, ADD_PROJECT_NAME_PLACEHOLDER);
    assert_eq!(line.spans[0].content.as_ref(), "E");
    assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
    assert_eq!(line.spans[0].style.bg, Some(FG));
    assert_eq!(line.spans[1].content.as_ref(), "nter project name here...");
    assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    assert_eq!(line.to_string(), ADD_PROJECT_NAME_PLACEHOLDER);
}
