use super::*;

#[tokio::test]
async fn detail_tab_focuses_locally_available_images_independent_of_preview_state() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.store
        .ensure_task_details(std::slice::from_ref(&task_id))
        .await
        .unwrap();
    let suppressed = test_attachment("SUPPRESSED", "image/png", true, Some((640, 480)));
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext {
            unavailable_hashes: [suppressed.sha256.clone()].into_iter().collect(),
            ..crate::tui::ui::DetailInlineImageContext::default()
        });
    let mut unavailable = test_attachment("UNAVAILABLE", "image/png", true, Some((640, 480)));
    unavailable.bytes_state = crate::attachments::AttachmentBytesState::Unavailable;
    app.store.tasks[selected].attachments = vec![
        test_attachment("PENDING", "image/png", false, Some((640, 480))),
        unavailable,
        suppressed,
        test_attachment("DOCUMENT", "application/pdf", true, Some((640, 480))),
        test_attachment("INVALID", "image/png", true, Some((0, 480))),
        test_attachment("FIRSTIMAGE", "image/png", true, Some((640, 480))),
        test_attachment("LASTIMAGE", "image/jpeg", true, Some((800, 600))),
    ];
    app.show_detail(3);

    app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::attachment_id),
        Some("SUPPRESSED")
    );
    assert_eq!(
        app.view()
            .detail_focus
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::attachment_id),
        Some("SUPPRESSED")
    );
    let forward_scroll = detail_scroll(&app);
    let context = app.inline_images.context_override().unwrap().clone();
    assert_eq!(
        detail_attachment_hit_id(
            &app.store.tasks[selected],
            100,
            30,
            forward_scroll,
            &context,
        )
        .as_deref(),
        Some("SUPPRESSED")
    );

    app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(shift_key(KeyCode::Tab), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(&DetailTargetId::Expand {
            section: DetailSection::Activity,
        })
    );
    app.dispatch_key(shift_key(KeyCode::Tab), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::attachment_id),
        Some("LASTIMAGE")
    );
    let reverse_scroll = detail_scroll(&app);
    assert!(reverse_scroll >= forward_scroll);
}

fn fail_image_viewer(_path: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("viewer unavailable")
}

#[tokio::test]
async fn external_viewer_notification_keeps_inline_previews_enabled() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
    create_and_select_task(&mut app, test_task_draft("Image task")).await;
    app.show_detail(0);

    app.set_success("opened attachment in default image viewer");

    assert!(
        app.inline_image_context()
            .is_some_and(|context| context.previews_enabled)
    );
}

#[tokio::test]
async fn unsupported_terminal_focus_opens_highlighted_image_externally() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let attachment_id =
        add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext {
            previews_enabled: false,
            ..crate::tui::ui::DetailInlineImageContext::default()
        });
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::attachment_id),
        Some(attachment_id.as_str())
    );
    app.dispatch_key(key(KeyCode::Char('o')), (100, 30).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.inline_images.export_count(), 1);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("opened attachment in default image viewer")
    );
}

#[tokio::test]
async fn unsupported_terminal_enter_and_mouse_use_external_viewer() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let attachment_id =
        add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;
    let context = crate::tui::ui::DetailInlineImageContext {
        previews_enabled: false,
        ..crate::tui::ui::DetailInlineImageContext::default()
    };
    app.inline_images.set_context_override(context.clone());
    app.show_detail(0);
    app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(app.inline_images.export_count(), 1);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    let item = app.store.selected_task(app.list.selected_task()).unwrap();
    let (column, row) = (0..30)
        .flat_map(|row| (0..100).map(move |column| (column, row)))
        .find(|(column, row)| {
            crate::tui::ui::detail_attachment_at_position(item, 100, 30, *column, *row, 0, &context)
                .is_some_and(|hit| hit.attachment_id == attachment_id)
        })
        .expect("attachment label hit target");
    app.dispatch_mouse(left_click(column, row), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(app.inline_images.export_count(), 1);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn full_screen_preview_opens_current_image_externally() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let attachment_id =
        add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
    app.overlay = Some(OverlayState::AttachmentPreview {
        attachment_id: attachment_id.clone(),
        scroll: 3,
    });

    app.dispatch_key(key(KeyCode::Char('o')), (100, 30).into())
        .await
        .unwrap();

    assert_eq!(app.inline_images.export_count(), 1);
    assert!(matches!(
        app.overlay,
        Some(OverlayState::AttachmentPreview {
            attachment_id: ref current,
            scroll: 3,
        }) if current == &attachment_id
    ));
}

#[tokio::test]
async fn external_viewer_reports_unavailable_bytes_and_launch_failures() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let attachment_id =
        add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("UPDATE blob_inventory SET available = 0")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    app.open_attachment_externally(&attachment_id).await;
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("attachment bytes are unavailable")
    );
    assert!(app.inline_images.export_count() == 0);

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("UPDATE blob_inventory SET available = 1")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    app.inline_images
        .set_image_viewer_launcher(fail_image_viewer);
    app.open_attachment_externally(&attachment_id).await;
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("could not start the default image viewer")
    );
    assert!(app.inline_images.export_count() == 0);
    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_leases")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(lease_count, 0);

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("UPDATE task_attachments SET deleted = 1, deleted_at = '2026-07-19T00:00:00Z'")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    app.open_attachment_externally(&attachment_id).await;
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("attachment is no longer available")
    );
}

#[tokio::test]
async fn successful_external_export_holds_temp_file_until_app_cleanup() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let attachment_id =
        add_real_attachment(&mut app, &pool, &dir.path().join("test.db"), selected).await;

    app.open_attachment_externally(&attachment_id).await;

    let export_dir = app.inline_images.export_directory(0).to_path_buf();
    assert!(export_dir.join("attachment.png").is_file());
    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_leases")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(lease_count, 0);
    app.inline_images.clear_exports();
    assert!(!export_dir.exists());
}

#[tokio::test]
async fn dropped_external_export_releases_read_lease() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    let db_path = dir.path().join("test.db");
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let attachment_id = add_real_attachment(&mut app, &pool, &db_path, selected).await;
    let blob_dir = crate::config::resolve_blob_dir(&db_path, app.intake.config()).unwrap();
    let export = app
        .store
        .lease_image_export(&blob_dir, &attachment_id)
        .await
        .unwrap();
    let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_leases")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(lease_count, 1);

    drop(export);
    for _ in 0..20 {
        let lease_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_leases")
            .fetch_one(&pool)
            .await
            .unwrap();
        if lease_count == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("dropped image export retained its read lease");
}

#[tokio::test]
async fn detail_keyboard_scroll_includes_framed_preview_rows() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let context = crate::tui::ui::DetailInlineImageContext::default();
    app.inline_images.set_context_override(context.clone());
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    app.store.tasks[selected].attachments = vec![test_attachment(
        "OVERFLOWIMAGE",
        "image/png",
        true,
        Some((640, 480)),
    )];
    app.show_detail(0);
    let text_only_cap = crate::tui::ui::detail_scroll_cap(&app.store.tasks[selected], 80, 26);
    let preview_cap = crate::tui::ui::detail_scroll_cap_with_images(
        &app.store.tasks[selected],
        80,
        26,
        Some(&context),
    );
    assert!(preview_cap > text_only_cap);

    app.dispatch_key(key(KeyCode::PageDown), (80, 26).into())
        .await
        .unwrap();

    assert_eq!(detail_scroll(&app), preview_cap);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_child_focus_order_includes_images_and_enter_opens_preview() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child"]).await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.store.tasks[parent_index].attachments = vec![test_attachment(
        "FOCUSIMAGE",
        "image/png",
        true,
        Some((640, 480)),
    )];
    app.show_detail(5);

    app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::task_id),
        Some(&child_id)
    );
    app.dispatch_key(key(KeyCode::Char('j')), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::attachment_id),
        Some("FOCUSIMAGE")
    );
    let image_scroll = detail_scroll(&app);
    assert_eq!(
        detail_attachment_hit_id(
            &app.store.tasks[parent_index],
            100,
            30,
            image_scroll,
            app.inline_images.context_override().unwrap(),
        )
        .as_deref(),
        Some("FOCUSIMAGE")
    );
    app.dispatch_key(key(KeyCode::Char('k')), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::task_id),
        Some(&child_id)
    );
    app.dispatch_key(key(KeyCode::Char('j')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::AttachmentPreview {
            ref attachment_id,
            scroll,
        }) if attachment_id == "FOCUSIMAGE" && scroll == image_scroll
    ));
}

#[tokio::test]
async fn full_screen_attachment_preview_switches_between_images() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let suppressed = test_attachment("SUPPRESSEDIMAGE", "image/png", true, Some((640, 480)));
    let suppressed_hash = suppressed.sha256.clone();
    app.store.tasks[selected].attachments = vec![
        test_attachment("FIRSTIMAGE", "image/png", true, Some((640, 480))),
        test_attachment("MISSINGIMAGE", "image/png", false, Some((640, 480))),
        test_attachment("DOCUMENT", "application/pdf", true, None),
        suppressed,
        test_attachment("SECONDIMAGE", "image/jpeg", true, Some((800, 600))),
    ];
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext {
            unavailable_hashes: [suppressed_hash].into_iter().collect(),
            ..crate::tui::ui::DetailInlineImageContext::default()
        });
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target =
        Some(crate::tui::app::DetailTargetId::Attachment {
            attachment_id: "FIRSTIMAGE".to_string(),
        });
    app.overlay = Some(OverlayState::AttachmentPreview {
        attachment_id: "FIRSTIMAGE".to_string(),
        scroll: 4,
    });

    app.dispatch_key(key(KeyCode::Char('j')), (100, 30).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::AttachmentPreview {
            ref attachment_id,
            scroll: 4,
        }) if attachment_id == "SECONDIMAGE"
    ));
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::attachment_id),
        Some("SECONDIMAGE")
    );

    app.dispatch_key(key(KeyCode::Down), (100, 30).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::AttachmentPreview {
            ref attachment_id,
            scroll: 4,
        }) if attachment_id == "FIRSTIMAGE"
    ));

    app.dispatch_key(key(KeyCode::Char('k')), (100, 30).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::AttachmentPreview {
            ref attachment_id,
            scroll: 4,
        }) if attachment_id == "SECONDIMAGE"
    ));
}

#[tokio::test]
async fn detail_image_click_opens_preview_while_payload_is_loading() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    let attachment = test_attachment("CLICKIMAGE", "image/png", true, Some((640, 480)));
    let source_hash = attachment.sha256.clone();
    app.store.tasks[selected].attachments = vec![attachment];
    app.show_detail(0);
    app.widgets.inline_image_placements = vec![crate::tui::ui::DetailInlineImagePlacement {
        attachment_id: "CLICKIMAGE".to_string(),
        source_hash,
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }];
    let item = &app.store.tasks[selected];
    let context = crate::tui::ui::DetailInlineImageContext::default();
    let (column, row) = (0..30)
        .flat_map(|row| (0..100).map(move |column| (column, row)))
        .find(|(column, row)| {
            crate::tui::ui::detail_attachment_at_position(item, 100, 30, *column, *row, 0, &context)
                .is_some()
        })
        .expect("image hit target");

    app.dispatch_mouse(left_click(column, row), (100, 30).into())
        .await
        .unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::AttachmentPreview {
            ref attachment_id,
            scroll: 0,
        }) if attachment_id == "CLICKIMAGE"
    ));
}

#[tokio::test]
async fn duplicate_hash_click_requires_the_visible_attachment_identity() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let context = crate::tui::ui::DetailInlineImageContext::default();
    app.inline_images.set_context_override(context.clone());
    let selected = create_and_select_task(&mut app, test_task_draft("Duplicate images")).await;
    let first = test_attachment("FIRSTDUPLICATE", "image/png", true, Some((640, 480)));
    let mut second = test_attachment("SECONDDUPLICATE", "image/png", true, Some((640, 480)));
    second.sha256 = first.sha256.clone();
    let source_hash = first.sha256.clone();
    app.store.tasks[selected].attachments = vec![first, second];
    let scroll = crate::tui::ui::detail_attachment_scroll_target(
        &app.store.tasks[selected],
        "SECONDDUPLICATE",
        0,
        80,
        30,
        &context,
    )
    .expect("second attachment scroll target");
    app.show_detail(scroll);
    let (column, row) = (0..30)
        .flat_map(|row| (0..80).map(move |column| (column, row)))
        .find(|(column, row)| {
            crate::tui::ui::detail_attachment_at_position(
                &app.store.tasks[selected],
                80,
                30,
                *column,
                *row,
                scroll,
                &context,
            )
            .is_some_and(|hit| hit.attachment_id == "SECONDDUPLICATE")
        })
        .expect("second attachment hit");
    app.widgets.inline_image_placements = vec![crate::tui::ui::DetailInlineImagePlacement {
        attachment_id: "FIRSTDUPLICATE".to_string(),
        source_hash: source_hash.clone(),
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }];

    app.dispatch_mouse(left_click(column, row), (80, 30).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(detail_scroll(&app), scroll);

    app.widgets.inline_image_placements[0].attachment_id = "SECONDDUPLICATE".to_string();
    app.dispatch_mouse(left_click(column, row), (80, 30).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::AttachmentPreview {
            ref attachment_id,
            scroll: current,
        }) if attachment_id == "SECONDDUPLICATE" && current == scroll
    ));
}

#[tokio::test]
async fn changing_detail_task_clears_image_focus() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let first = create_and_select_task(&mut app, test_task_draft("First")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Second")).await;
    let first = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == first_id)
        .unwrap();
    app.store.tasks[first].attachments = vec![test_attachment(
        "FIRSTIMAGE",
        "image/png",
        true,
        Some((640, 480)),
    )];
    app.list.select_task(Some(first));
    app.show_detail(0);
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target =
        Some(crate::tui::app::DetailTargetId::Attachment {
            attachment_id: "FIRSTIMAGE".to_string(),
        });

    let previous = app.list.selected_task();
    app.select_detail_task(1).await.unwrap();

    assert_ne!(app.list.selected_task(), previous);
    assert!(app.detail.state_mut().unwrap().focused_target.is_none());
    assert!(app.detail.state_mut().unwrap().focused_target.is_none());
}

#[tokio::test]
async fn invalidated_image_focus_is_cleared_without_closing_detail() {
    for key_code in [KeyCode::Enter, KeyCode::Esc] {
        let (_dir, _pool, mut app) = test_app_with_pool().await;
        app.inline_images
            .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
        let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
        let attachment = test_attachment("INVALIDATEDIMAGE", "image/png", true, Some((640, 480)));
        app.store.tasks[selected].attachments = vec![attachment];
        app.show_detail(4);
        app.dispatch_key(key(KeyCode::Tab), (100, 30).into())
            .await
            .unwrap();
        let focused_scroll = detail_scroll(&app);
        assert_eq!(
            app.detail
                .state_mut()
                .unwrap()
                .focused_target
                .as_ref()
                .and_then(crate::tui::app::DetailTargetId::attachment_id),
            Some("INVALIDATEDIMAGE")
        );

        app.store.tasks[selected].attachments[0].has_blob = false;
        app.store.tasks[selected].attachments[0].bytes_state =
            crate::attachments::AttachmentBytesState::Unavailable;
        assert!(app.view().detail_focus.is_none());

        app.dispatch_key(key(key_code), (100, 30).into())
            .await
            .unwrap();

        assert!(app.detail.state_mut().unwrap().focused_target.is_none());
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        assert_eq!(detail_scroll(&app), focused_scroll);
    }
}

#[tokio::test]
async fn detail_image_escape_returns_and_preserves_focus() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
    let selected = create_and_select_task(&mut app, test_task_draft("Image task")).await;
    app.store.tasks[selected].attachments = vec![test_attachment(
        "ATTACHMENT000001",
        "image/png",
        true,
        Some((640, 480)),
    )];
    app.show_detail(4);
    app.detail.state_mut().unwrap().focused_target =
        Some(crate::tui::app::DetailTargetId::Attachment {
            attachment_id: "ATTACHMENT000001".to_string(),
        });
    app.overlay = Some(OverlayState::AttachmentPreview {
        attachment_id: "ATTACHMENT000001".to_string(),
        scroll: 4,
    });
    assert!(!app.detail_underlay());

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::attachment_id),
        Some("ATTACHMENT000001")
    );
}

#[tokio::test]
async fn attachment_preview_refresh_preserves_owning_task_across_query_change() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    app.inline_images
        .set_context_override(crate::tui::ui::DetailInlineImageContext::default());
    let selected = create_and_select_task(&mut app, test_task_draft("Preview owner")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.store
        .ensure_task_details(std::slice::from_ref(&task_id))
        .await
        .unwrap();
    app.store.tasks[selected].attachments = vec![test_attachment(
        "OWNEDIMAGE",
        "image/png",
        true,
        Some((640, 480)),
    )];
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target =
        Some(crate::tui::app::DetailTargetId::Attachment {
            attachment_id: "OWNEDIMAGE".to_string(),
        });
    app.overlay = Some(OverlayState::AttachmentPreview {
        attachment_id: "OWNEDIMAGE".to_string(),
        scroll: 6,
    });
    app.store.view_state.query = TaskQuery::Done;

    app.refresh().await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, task_id);
    assert_eq!(
        app.store.tasks[selected].attachments[0].attachment_id,
        "OWNEDIMAGE"
    );
    app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, task_id);
}
