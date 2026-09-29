use super::*;

#[test]
fn detail_document_projections_share_semantic_body_geometry() {
    let mut item = detail_test_epic_item();
    item.epic_parent = Some(crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: crate::test_support::task_id("epic-parent-id"),
        display_ref: "APP-EPIC".to_string(),
        title: "Parent epic".to_string(),
        status: "active".to_string(),
        priority: "high".to_string(),
        unresolved: true,
    });
    item.task.description =
        "A wrapped description with enough words to occupy several rows.".to_string();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    let images = DetailInlineImageContext::default();
    let children = detail_epic_children(&item, None);
    let body =
        build_detail_body_document(&item, &children, 42, &BTreeSet::new(), Some(&images), &[]);
    let selectable = detail_selectable_document_from_body(&item, 42, true, &body);
    let model = project_detail_content_model(Vec::new(), &body, usize::MAX, 0);

    assert_eq!(model.content_height, body.lines.len());
    assert_eq!(model.interactive_rows, body.interactive_rows);
    assert_eq!(model.image_placements.len(), body.image_placements.len());
    for line in &selectable.description {
        let body_index = line.body_index.expect("description body index");
        assert_eq!(
            body.lines[body_index].to_string(),
            format!("│ {}", line.text)
        );
    }
    for row in body.interactive_rows.iter() {
        assert!(row.line_index < body.lines.len());
        assert!(row.line_index + row.height <= body.lines.len());
    }
    let section_lines = body
        .section_body_indices
        .iter()
        .map(|index| body.lines[*index].to_string())
        .collect::<Vec<_>>();
    assert_eq!(section_lines[0], "EPIC PARENT");
    assert!(section_lines[1].starts_with("│ A wrapped description"));
    assert_eq!(section_lines[2], "NOTES (n add · e edit · D delete)");
    assert_eq!(section_lines[3], "WHY BLOCKED open=1 total=1");
}

#[test]
fn detail_selection_maps_and_highlights_across_wrapped_title_lines() {
    let mut item = detail_test_item();
    item.task.title = "first wrapped second line".to_string();
    let expanded_sections = BTreeSet::new();
    let context = detail_query_context(20, 24, 0, &expanded_sections, None);
    let document = DetailDocument::build(&item, &context);
    let layout = detail_content_layout(context.terminal_area);
    let first = document
        .text_cell_at_position(layout.content_area.x, layout.content_area.y)
        .unwrap();
    let focus = document
        .text_cell_at_position(
            layout.content_area.x + "second line".width() as u16 - 1,
            layout.content_area.y + 1,
        )
        .unwrap();
    let mut selection = DetailTextSelection::new(item.task.id.clone(), 20, first);
    selection.focus = focus;

    assert_eq!(document.sticky_height(), 5);
    assert_eq!(
        document.selected_text(&selection).as_deref(),
        Some(item.task.title.as_str())
    );

    let model = build_detail_content_model(
        &item,
        layout.content_area,
        0,
        None,
        None,
        &expanded_sections,
        Some(&selection),
        None,
    );
    for line in &model.sticky_lines[..2] {
        assert!(
            line.spans
                .iter()
                .any(|span| span.style.bg == Some(theme::accent()))
        );
    }
}

#[test]
fn detail_content_layout_matches_wide_and_narrow_metadata_rules() {
    let wide = detail_content_layout(Rect::new(0, 0, 120, 30));

    assert_eq!(wide.body_area, Rect::new(0, 2, 120, 26));
    assert!(wide.metadata_area.width > 0);
    assert_eq!(wide.content_area.y, 3);

    let narrow = detail_content_layout(Rect::new(0, 0, 80, 30));

    assert_eq!(narrow.metadata_area, Rect::default());
    assert_eq!(narrow.content_area.x, 2);
}

#[test]
fn detail_section_targets_cycle_through_notes_dependencies_and_activity() {
    let mut item = detail_test_item();
    item.task.description = (0..20)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let layout = detail_content_layout(Rect::new(0, 0, 80, 10));
    let indices = detail_section_body_indices(&item, layout.content_area.width as usize, None);

    let notes = detail_section_scroll_target(&item, 0, 80, 10, false);
    let dependencies = detail_section_scroll_target(&item, notes, 80, 10, false);
    let activity = detail_section_scroll_target(&item, dependencies, 80, 10, false);

    assert_eq!(notes, indices[1] as u16);
    assert_eq!(dependencies, indices[2] as u16);
    assert_eq!(activity, indices[3] as u16);
    assert_eq!(
        detail_section_scroll_target(&item, activity, 80, 10, false),
        0
    );
    assert_eq!(
        detail_section_scroll_target(&item, 0, 80, 10, true),
        activity
    );
}

#[test]
fn detail_content_model_prepares_render_lines_and_scrollbar() {
    let mut item = detail_test_item();
    item.task.description = (0..20)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    let model = build_detail_content_model(
        &item,
        Rect::new(0, 0, 60, 5),
        4,
        None,
        None,
        &BTreeSet::new(),
        None,
        None,
    );

    assert_eq!(
        model.content_height,
        detail_body_lines(&item, 60, None).len()
    );
    assert_eq!(
        model.sticky_lines.len(),
        detail_header_options(&item, 60, None).len()
    );
    assert_eq!(model.sticky_lines[0].to_string(), "Fix token refresh race");
    let visible = 5usize.saturating_sub(model.sticky_lines.len().min(5));
    assert_eq!(
        model.lines.len(),
        model.content_height.saturating_sub(4).min(visible)
    );
    assert!(model.scrollbar_position > 0);
}

#[test]
fn cached_geometry_accepts_frame_specific_detail_styles() {
    let item = detail_test_item();
    let expanded_sections = BTreeSet::new();
    let images = DetailInlineImageContext::default();
    let base = DetailRenderContext {
        terminal_area: Rect::new(0, 0, 80, 24),
        scroll: 0,
        detail_revision: DetailRevision::UNCACHED,
        inline_title_editor: None,
        active_target: None,
        hovered_target: None,
        expanded_sections: &expanded_sections,
        selection: None,
        inline_images: Some(&images),
        pending_attachments: &[],
        removed_epic_child: None,
    };
    let document = DetailDocument::build(&item, &base);
    let active = DetailTargetId::Task {
        section: DetailSection::DependsOn,
        task_id: item.depends_on[0].task_id.clone(),
    };
    let hovered = DetailTargetId::Task {
        section: DetailSection::Blocks,
        task_id: item.blocks[0].task_id.clone(),
    };
    let selection =
        DetailTextSelection::new(item.task.id.clone(), 80, TextCell { start: 0, end: 3 });
    let focused_images = DetailInlineImageContext {
        focused_attachment_id: Some("attachment".to_string()),
        ..images.clone()
    };
    let styled = DetailRenderContext {
        terminal_area: base.terminal_area,
        scroll: 0,
        detail_revision: base.detail_revision,
        inline_title_editor: None,
        active_target: Some(&active),
        hovered_target: Some(&hovered),
        expanded_sections: &expanded_sections,
        selection: Some(&selection),
        inline_images: Some(&focused_images),
        pending_attachments: &[],
        removed_epic_child: None,
    };

    assert!(document.matches_frame(&item, &styled));
}

#[test]
fn cached_geometry_invalidates_for_semantic_detail_changes() {
    let item = detail_test_item();
    let expanded_sections = BTreeSet::new();
    let images = DetailInlineImageContext::default();
    let base = DetailRenderContext {
        terminal_area: Rect::new(0, 0, 80, 24),
        scroll: 0,
        detail_revision: DetailRevision::UNCACHED,
        inline_title_editor: None,
        active_target: None,
        hovered_target: None,
        expanded_sections: &expanded_sections,
        selection: None,
        inline_images: Some(&images),
        pending_attachments: &[],
        removed_epic_child: None,
    };
    let document = DetailDocument::build(&item, &base);
    assert!(document.matches_frame(&item, &base));

    let changed_context = DetailRenderContext {
        detail_revision: DetailRevision::next(),
        ..base
    };
    let mut changed_item = item.clone();
    changed_item.task.status = crate::choices::TaskStatus::Done;
    assert!(!document.matches_frame(&changed_item, &changed_context));

    changed_item = item.clone();
    changed_item.notes[0].body = "Updated note".to_string();
    assert!(!document.matches_frame(&changed_item, &changed_context));

    let epic = detail_test_epic_item();
    let epic_document = DetailDocument::build(&epic, &base);
    let mut changed_epic = epic.clone();
    changed_epic.epic_children[0].status = "done".to_string();
    changed_epic.epic_children[0].unresolved = false;
    assert!(!epic_document.matches_frame(&changed_epic, &changed_context));

    let pending = [crate::tui::attachment_controller::PendingAttachmentView {
        attachment_id: "pending".to_string(),
        task_id: item.task.id.clone(),
        status: crate::tui::attachment_controller::PendingAttachmentStatus::Preparing,
    }];
    let pending_context = DetailRenderContext {
        pending_attachments: &pending,
        ..base
    };
    assert!(!document.matches_frame(&item, &pending_context));

    let editor = TextInputView {
        kind: crate::tui::overlay::TextInputKind::EditTitle,
        title: "Edit title".to_string(),
        prompt: String::new(),
        input: item.task.title.clone(),
        cursor: 2,
    };
    let editor_context = DetailRenderContext {
        inline_title_editor: Some(&editor),
        pending_attachments: &[],
        ..base
    };
    assert!(!document.matches_frame(&item, &editor_context));

    let expanded_sections = [DetailSection::DependsOn].into_iter().collect();
    let expanded_context = DetailRenderContext {
        expanded_sections: &expanded_sections,
        pending_attachments: &[],
        ..base
    };
    assert!(!document.matches_frame(&item, &expanded_context));
}
