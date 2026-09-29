use super::*;

#[test]
fn detail_renders_pending_and_failed_attachment_rows() {
    let item = detail_test_item();
    let pending = vec![
        crate::tui::attachment_controller::PendingAttachmentView {
            attachment_id: "PENDINGATTACH01".to_string(),
            task_id: item.task.id.clone(),
            status: crate::tui::attachment_controller::PendingAttachmentStatus::Preparing,
        },
        crate::tui::attachment_controller::PendingAttachmentView {
            attachment_id: "PENDINGATTACH02".to_string(),
            task_id: item.task.id.clone(),
            status: crate::tui::attachment_controller::PendingAttachmentStatus::Failed,
        },
    ];

    let rendered =
        detail_body_lines_with_pending_images(&item, 60, None, &BTreeSet::new(), None, &pending)
            .0
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

    assert!(rendered.contains("ATTACHMENTS"));
    assert!(rendered.contains("[image: preparing]"));
    assert!(rendered.contains("[image: failed]"));
}

#[test]
fn detail_empty_description_renders_attachment_section() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    item.attachments[0].filename = None;

    let rendered = detail_content_lines(&item, 80, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("ATTACHMENTS\n│ [image: attachment]"));
}

#[test]
fn detail_attachment_rows_show_filename_and_generic_fallback() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![
        attachment_metadata("ATTACHMENT000001", false, true),
        attachment_metadata("ATTACHMENT000002", false, true),
    ];
    item.attachments[0].filename = Some("super_aïti_floral_transparent.png".to_string());
    item.attachments[1].filename = None;

    let rendered = detail_content_lines(&item, 80, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| {
        line == "│ [image: attachment] super_aïti_floral_transparent.png · 640×480 · 4 B"
    }));
    assert!(
        rendered
            .iter()
            .any(|line| line == "│ [image: attachment] · 640×480 · 4 B")
    );
}

#[test]
fn detail_attachment_filename_truncates_to_content_width() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    item.attachments[0].filename = Some("a-very-long-attachment-filename.png".to_string());

    let rendered = detail_content_lines(&item, 30, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let attachment = rendered
        .iter()
        .find(|line| line.starts_with("│ [image: attachment]"))
        .expect("attachment row");

    assert_eq!(UnicodeWidthStr::width(attachment.as_str()), 29);
    assert!(attachment.ends_with('…'));
}

#[test]
fn detail_attachment_row_styles_filename_and_metadata() {
    let attachment = attachment_metadata("ATTACHMENT000001", false, true);

    let line = attachment_detail_line(&attachment, 80, false);

    assert_eq!(
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>(),
        vec![
            "[image: attachment]",
            " chart.png",
            " · ",
            "640×480",
            " · ",
            "4 B",
        ]
    );
    assert_eq!(line.spans[0].style.fg, Some(theme::fg_muted()));
    assert_eq!(line.spans[1].style.fg, Some(theme::fg()));
    assert_eq!(line.spans[2].style.fg, Some(theme::fg_dim()));
    assert_eq!(line.spans[3].style.fg, Some(theme::fg_muted()));

    let focused = attachment_detail_line(&attachment, 80, true);
    assert!(
        focused
            .spans
            .iter()
            .all(|span| span.style.fg == Some(theme::accent()))
    );
}

#[test]
fn detail_attachment_section_renders_live_rows_once_in_order() {
    let mut item = detail_test_item();
    item.attachments = vec![
        attachment_metadata("ATTACHMENT000001", false, true),
        attachment_metadata("ATTACHMENT000002", false, false),
        attachment_metadata("ATTACHMENT000003", false, false),
        attachment_metadata("ATTACHMENT000004", true, true),
    ];
    item.attachments[0].alt_text = Some("First".to_string());
    item.attachments[1].alt_text = Some("Second".to_string());
    item.attachments[2].alt_text = Some("Third".to_string());
    item.attachments[2].bytes_state = crate::attachments::AttachmentBytesState::Unavailable;
    item.attachments[3].alt_text = Some("Deleted".to_string());

    let rendered = detail_content_lines(&item, 80, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered.matches("ATTACHMENTS").count(), 1);
    assert_eq!(rendered.matches("[image: attachment]").count(), 1);
    assert_eq!(rendered.matches("[image: pending download]").count(), 1);
    assert_eq!(rendered.matches("[image: unavailable bytes]").count(), 1);
    assert!(
        rendered.find("[image: attachment]").unwrap()
            < rendered.find("[image: pending download]").unwrap()
    );
    assert!(!rendered.contains("Deleted"));
}

#[test]
fn detail_reserves_rows_for_previewable_attachment() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    let context = DetailInlineImageContext::default();

    let (lines, placements, _, _) = detail_body_lines_with_images(&item, 80, None, Some(&context));

    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].height, 12);
    assert!(lines.iter().any(|line| {
        line.to_string() == "│ [image: attachment] chart.png · 640×480 · 4 B"
    }));
    assert_eq!(
        lines[placements[0].line_index - 1].to_string(),
        format!("│ ┌{}┐", "─".repeat(placements[0].width as usize))
    );
    assert_eq!(
        lines[placements[0].line_index + placements[0].height as usize].to_string(),
        format!("│ └{}┘", "─".repeat(placements[0].width as usize))
    );
}

#[test]
fn focused_preview_changes_border_style_without_moving_image() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    let unfocused = DetailInlineImageContext::default();
    let focused = DetailInlineImageContext {
        focused_attachment_id: Some("ATTACHMENT000001".to_string()),
        ..DetailInlineImageContext::default()
    };

    let (unfocused_lines, unfocused_placements, _, _) =
        detail_body_lines_with_images(&item, 80, None, Some(&unfocused));
    let (focused_lines, focused_placements, _, _) =
        detail_body_lines_with_images(&item, 80, None, Some(&focused));

    assert_eq!(unfocused_placements.len(), 1);
    assert_eq!(
        (
            unfocused_placements[0].line_index,
            unfocused_placements[0].width,
            unfocused_placements[0].height,
        ),
        (
            focused_placements[0].line_index,
            focused_placements[0].width,
            focused_placements[0].height,
        )
    );
    let border_index = unfocused_placements[0].line_index - 1;
    assert_eq!(
        unfocused_lines[border_index].to_string(),
        focused_lines[border_index].to_string()
    );
    assert_ne!(
        unfocused_lines[border_index].spans[1].style,
        focused_lines[border_index].spans[1].style
    );
}

#[test]
fn detail_preview_preserves_image_aspect_within_max_rows() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    item.attachments[0].width = Some(646);
    item.attachments[0].height = Some(302);
    let context = DetailInlineImageContext::default();

    let (_lines, placements, _, _) =
        detail_body_lines_with_images(&item, 200, None, Some(&context));

    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].height, 12);
    assert_eq!(placements[0].width, 51);
}

#[test]
fn detail_suppressed_preview_keeps_textual_attachment_row() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    let context = DetailInlineImageContext {
        unavailable_hashes: [item.attachments[0].sha256.clone()].into_iter().collect(),
        ..DetailInlineImageContext::default()
    };

    let (lines, placements, _, _) = detail_body_lines_with_images(&item, 80, None, Some(&context));

    assert!(placements.is_empty());
    assert!(lines.iter().any(|line| {
        line.to_string() == "│ [image: attachment] chart.png · 640×480 · 4 B"
    }));
}

#[test]
fn detail_falls_back_to_single_placeholder_when_previews_disabled() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];

    let (lines, placements, _, _) = detail_body_lines_with_images(&item, 80, None, None);

    assert!(placements.is_empty());
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.to_string().contains("[image: attachment]"))
            .count(),
        1
    );
    assert_eq!(
        lines.iter().filter(|line| line.to_string() == "│ ").count(),
        0
    );
}

#[test]
fn detail_placements_only_include_visible_preview_rows() {
    let mut item = detail_test_item();
    item.task.description = "intro".to_string();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    let context = DetailInlineImageContext::default();

    let model = build_detail_content_model(
        &item,
        Rect::new(0, 0, 80, 10),
        0,
        None,
        None,
        &BTreeSet::new(),
        None,
        Some(&context),
    );

    assert_eq!(model.image_placements.len(), 1);
    assert!(model.image_placements[0].line_index < model.content_height);
}

#[test]
fn detail_omits_preview_when_frame_is_clipped_by_viewport() {
    let model = DetailContentRenderModel {
        sticky_lines: Vec::new(),
        lines: vec![Line::from(""); 5],
        content_height: 12,
        body_start: 0,
        scrollbar_position: 0,
        image_placements: vec![DetailBodyImagePlacement {
            attachment_id: "ATTACHMENT000001".to_string(),
            source_hash: "0".repeat(64),
            line_index: 3,
            width: 30,
            height: 12,
        }]
        .into(),
        interactive_rows: Vec::new().into(),
    };
    let mut widgets = WidgetState::default();
    let backend = TestBackend::new(20, 5);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            render_detail_content_from_model(frame, Rect::new(0, 0, 20, 5), model, &mut widgets);
        })
        .unwrap();

    assert!(widgets.inline_image_placements.is_empty());
}

#[test]
fn detail_attachment_hit_tracks_scroll_and_excludes_suppressed_preview() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    let terminal = Rect::new(0, 0, 100, 40);
    let layout = detail_content_layout(terminal);
    let context = DetailInlineImageContext::default();
    let scroll = 2;
    let model = build_detail_content_model(
        &item,
        layout.content_area,
        scroll,
        None,
        None,
        &BTreeSet::new(),
        None,
        Some(&context),
    );
    let placement = &model.image_placements[0];
    let body_y = layout.content_area.y.saturating_add(
        model
            .sticky_lines
            .len()
            .min(layout.content_area.height as usize) as u16,
    );
    let column = layout.content_area.x.saturating_add(2);
    let row = body_y.saturating_add(
        placement
            .line_index
            .saturating_sub(model.body_start)
            .saturating_sub(1) as u16,
    );

    let hit = detail_attachment_at_position(
        &item,
        terminal.width,
        terminal.height,
        column,
        row,
        scroll,
        &context,
    );
    assert_eq!(
        hit.map(|hit| hit.attachment_id),
        Some("ATTACHMENT000001".to_string())
    );
    let suppressed_context = DetailInlineImageContext {
        unavailable_hashes: [item.attachments[0].sha256.clone()].into_iter().collect(),
        ..DetailInlineImageContext::default()
    };
    assert!(
        detail_attachment_at_position(
            &item,
            terminal.width,
            terminal.height,
            column,
            row,
            scroll,
            &suppressed_context,
        )
        .is_none()
    );
}

#[test]
fn duplicate_hash_hit_uses_the_visible_attachment_frame() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![
        attachment_metadata("FIRSTATTACHMENT", false, true),
        attachment_metadata("SECONDATTACHMENT", false, true),
    ];
    assert_eq!(item.attachments[0].sha256, item.attachments[1].sha256);
    let context = DetailInlineImageContext::default();
    let terminal = Rect::new(0, 0, 80, 30);
    let layout = detail_content_layout(terminal);
    let model = build_detail_content_model(
        &item,
        layout.content_area,
        0,
        None,
        None,
        &BTreeSet::new(),
        None,
        Some(&context),
    );
    let sticky_height = model
        .sticky_lines
        .len()
        .min(layout.content_area.height as usize) as u16;
    let body_area = Rect::new(
        layout.content_area.x,
        layout.content_area.y.saturating_add(sticky_height),
        layout.content_area.width,
        layout.content_area.height.saturating_sub(sticky_height),
    );
    let cap = detail_scroll_cap_with_images(&item, 80, 30, Some(&context));
    let (scroll, image) = (0..=cap)
        .find_map(|scroll| {
            let model = build_detail_content_model(
                &item,
                layout.content_area,
                scroll,
                None,
                None,
                &BTreeSet::new(),
                None,
                Some(&context),
            );
            let first = visible_detail_image_rect(body_area, &model, &model.image_placements[0]);
            let second = visible_detail_image_rect(body_area, &model, &model.image_placements[1]);
            (first.is_none() && second.is_some()).then(|| (scroll, second.unwrap()))
        })
        .expect("only the second duplicate frame visible");
    let hit = detail_attachment_at_position(
        &item,
        80,
        30,
        image.x.saturating_sub(1),
        image.y.saturating_sub(1),
        scroll,
        &context,
    )
    .expect("visible duplicate hit");

    assert_eq!(hit.attachment_id, "SECONDATTACHMENT");
}

#[test]
fn framed_preview_overflow_expands_scroll_and_reaches_hit_target() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    let context = DetailInlineImageContext::default();
    let width = 80;
    let height = 26;
    let text_only_cap = detail_scroll_cap(&item, width, height);
    let preview_cap = detail_scroll_cap_with_images(&item, width, height, Some(&context));

    assert!(preview_cap > text_only_cap);
    let target =
        detail_attachment_scroll_target(&item, "ATTACHMENT000001", 0, width, height, &context)
            .expect("attachment scroll target");
    assert!(target > 0);
    assert!(target <= preview_cap);
    let reached = (0..height).any(|row| {
        (0..width).any(|column| {
            detail_attachment_at_position(&item, width, height, column, row, target, &context)
                .is_some()
        })
    });
    assert!(reached);
}

#[test]
fn large_attachment_preview_stays_inside_its_border() {
    let mut item = detail_test_item();
    item.task.description = String::new();
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
    let context = DetailInlineImageContext::default();
    let mut widgets = WidgetState::default();
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    let expanded_sections = BTreeSet::new();
    terminal
        .draw(|frame| {
            let render_context = DetailRenderContext {
                terminal_area: frame.area(),
                scroll: 0,
                detail_revision: DetailRevision::UNCACHED,
                inline_title_editor: None,
                active_target: None,
                hovered_target: None,
                expanded_sections: &expanded_sections,
                selection: None,
                inline_images: Some(&context),
                pending_attachments: &[],
                removed_epic_child: None,
            };
            render_detail(frame, &item, &render_context, &mut widgets);
        })
        .unwrap();
    let thumbnail = widgets.inline_image_placements[0].clone();
    widgets.inline_image_placements.clear();

    terminal
        .draw(|frame| {
            render_attachment_preview(
                frame,
                &item,
                "ATTACHMENT000001",
                &mut widgets,
                Some(&context),
            );
        })
        .unwrap();

    assert_eq!(widgets.inline_image_placements.len(), 1);
    let placement = &widgets.inline_image_placements[0];
    assert_eq!(placement.source_hash, thumbnail.source_hash);
    assert_ne!(
        (placement.x, placement.y, placement.width, placement.height),
        (thumbnail.x, thumbnail.y, thumbnail.width, thumbnail.height)
    );
    let area = detail_body_area(Rect::new(0, 0, 100, 30));
    assert!(placement.x > area.x);
    assert!(placement.y > area.y);
    assert!(placement.x.saturating_add(placement.width) < area.x + area.width);
    assert!(placement.y.saturating_add(placement.height) < area.y + area.height);
}
