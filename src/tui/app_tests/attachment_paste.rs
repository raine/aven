use super::*;

async fn finish_attachment_work(app: &mut App) {
    for _ in 0..400 {
        app.poll_attachment_work().await.unwrap();
        if !app.attachment_controller.work_pending() {
            app.poll_attachment_work().await.unwrap();
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("attachment work did not finish");
}

async fn finish_task_intake_draft(app: &mut App) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            app.poll_pending_task_intake().await.unwrap();
            if matches!(app.overlay, Some(OverlayState::AddTask(_))) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("task intake should produce an add-task draft");
}

fn compressible_png_bytes() -> Vec<u8> {
    let width = 16u32;
    let height = 16u32;
    let mut raw = Vec::new();
    for _ in 0..height {
        raw.push(0);
        raw.extend(std::iter::repeat_n(255, width as usize * 4));
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut encoder, &raw).unwrap();
    let idat = encoder.finish().unwrap();

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    append_png_chunk(&mut png, b"IHDR", &ihdr);
    append_png_chunk(&mut png, b"tEXt", &vec![b'a'; 4096]);
    append_png_chunk(&mut png, b"IDAT", &idat);
    append_png_chunk(&mut png, b"IEND", &[]);
    png
}

fn jpeg_bytes() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 1))
        .write_to(&mut bytes, image::ImageFormat::Jpeg)
        .unwrap();
    bytes.into_inner()
}

fn append_png_chunk(png: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    png.extend((data.len() as u32).to_be_bytes());
    png.extend(name);
    png.extend(data);
    let mut crc_data = Vec::with_capacity(name.len() + data.len());
    crc_data.extend(name);
    crc_data.extend(data);
    png.extend(crc32(&crc_data).to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[tokio::test]
async fn detail_paste_image_path_attaches_to_selected_task() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let selected = create_and_select_task(&mut app, test_task_draft("image target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let image = dir.path().join("photo.png");
    let original_bytes = compressible_png_bytes();
    std::fs::write(&image, &original_bytes).unwrap();
    app.show_detail(0);

    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    let pending = app.attachment_controller.views();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].task_id, task_id);
    assert_eq!(
        pending[0].status,
        crate::tui::attachment_controller::PendingAttachmentStatus::Preparing
    );
    let mut conn = pool.acquire().await.unwrap();
    let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(attachment_count, 0);
    drop(conn);
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("image already attached")
    );
    let item = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == task_id)
        .unwrap();
    assert!(item.task.description.is_empty());
    let mut conn = pool.acquire().await.unwrap();
    let attachments = crate::operations::attachment_read_items_by_task(
        &mut conn,
        app.store.active_workspace.id.as_str(),
        &task_id,
        false,
    )
    .await
    .unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0].attachment.filename.as_deref(),
        Some("photo.png")
    );
    assert_eq!(attachments[0].attachment.media_type, "image/png");
    assert_eq!(
        attachments[0].attachment.byte_size,
        original_bytes.len() as i64
    );
    assert!(attachments[0].has_blob);
}

#[tokio::test]
async fn focused_attachment_reports_task_commands_as_unavailable() {
    let (dir, _pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    create_and_select_task(&mut app, test_task_draft("image target")).await;
    let image = dir.path().join("photo.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    app.show_detail(0);
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;
    let attachment_id = app.store.tasks[0].attachments[0].attachment_id.clone();
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Attachment { attachment_id }));

    for code in [KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('e')] {
        app.dispatch_key(key(code), (100, 30).into()).await.unwrap();
    }

    assert!(app.footer_choice.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("leave attachment focus before using that command")
    );
    assert!(matches!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(DetailTargetId::Attachment { .. })
    ));
}

#[tokio::test]
async fn focused_detail_image_can_be_removed_after_confirmation() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    create_and_select_task(&mut app, test_task_draft("image target")).await;
    let image = dir.path().join("photo.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    app.show_detail(7);
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;
    let attachment_id = app.store.tasks[0].attachments[0].attachment_id.clone();
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target =
        Some(crate::tui::app::DetailTargetId::Attachment {
            attachment_id: attachment_id.clone(),
        });
    app.show_detail(7);

    app.dispatch_key(shift_key(KeyCode::Char('D')), (100, 30).into())
        .await
        .unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::DeleteAttachment { .. },
            ref title,
            ref prompt,
        })) if title == "Remove image" && prompt == "Remove photo.png?"
    ));

    app.dispatch_key(key(KeyCode::Char('y')), (100, 30).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.detail.state_mut().unwrap().focused_target.is_none());
    assert!(app.store.tasks[0].attachments.is_empty());
    assert_eq!(toast_message(&app).as_deref(), Some("removed image"));
    let deleted: i64 =
        sqlx::query_scalar("SELECT deleted FROM task_attachments WHERE attachment_id = ?")
            .bind(&attachment_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(deleted, 1);
}

#[tokio::test]
async fn removing_focused_attachment_selects_the_next_attachment() {
    let (dir, _pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    create_and_select_task(&mut app, test_task_draft("image target")).await;
    let first = dir.path().join("first.png");
    let second = dir.path().join("second.jpg");
    std::fs::write(&first, compressible_png_bytes()).unwrap();
    std::fs::write(&second, jpeg_bytes()).unwrap();
    app.show_detail(7);
    app.dispatch_paste(first.to_str().unwrap()).await.unwrap();
    app.dispatch_paste(second.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;
    let first_id = app.store.tasks[0].attachments[0].attachment_id.clone();
    let second_id = app.store.tasks[0].attachments[1].attachment_id.clone();
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target =
        Some(crate::tui::app::DetailTargetId::Attachment {
            attachment_id: first_id,
        });
    app.show_detail(7);

    app.dispatch_key(shift_key(KeyCode::Char('D')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('y')), (100, 30).into())
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
        Some(second_id.as_str())
    );
    assert_eq!(app.store.tasks[0].attachments.len(), 1);
}

#[tokio::test]
async fn attachment_preview_remove_can_be_cancelled() {
    let (dir, _pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    create_and_select_task(&mut app, test_task_draft("image target")).await;
    let image = dir.path().join("photo.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    app.show_detail(3);
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;
    let attachment_id = app.store.tasks[0].attachments[0].attachment_id.clone();
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target =
        Some(crate::tui::app::DetailTargetId::Attachment {
            attachment_id: attachment_id.clone(),
        });
    app.overlay = Some(OverlayState::AttachmentPreview {
        attachment_id: attachment_id.clone(),
        scroll: 3,
    });

    app.dispatch_key(shift_key(KeyCode::Char('D')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('n')), (100, 30).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.store.tasks[0].attachments.len(), 1);
}

#[tokio::test]
async fn detail_paste_image_path_obeys_optimization_config() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let mut config = crate::config::AppConfig::default();
    config.local.image_optimization = crate::config::ImageOptimizationConfig::Off;
    app.set_config(config);
    let selected = create_and_select_task(&mut app, test_task_draft("image target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let image = dir.path().join("photo.png");
    let bytes = compressible_png_bytes();
    std::fs::write(&image, &bytes).unwrap();
    app.show_detail(0);

    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("image already attached")
    );
    let mut conn = pool.acquire().await.unwrap();
    let attachments = crate::operations::attachment_read_items_by_task(
        &mut conn,
        app.store.active_workspace.id.as_str(),
        &task_id,
        false,
    )
    .await
    .unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].attachment.byte_size, bytes.len() as i64);
}

#[tokio::test]
async fn detail_paste_optimizes_asynchronously_when_enabled() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let mut config = crate::config::AppConfig::default();
    config.local.image_optimization = crate::config::ImageOptimizationConfig::Paste;
    app.set_config(config);
    let selected = create_and_select_task(&mut app, test_task_draft("optimized target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let image = dir.path().join("optimized.png");
    let bytes = compressible_png_bytes();
    std::fs::write(&image, &bytes).unwrap();
    app.show_detail(0);

    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    assert_eq!(app.attachment_controller.views().len(), 1);
    finish_attachment_work(&mut app).await;

    let mut conn = pool.acquire().await.unwrap();
    let attachments = crate::operations::attachment_read_items_by_task(
        &mut conn,
        app.store.active_workspace.id.as_str(),
        &task_id,
        false,
    )
    .await
    .unwrap();
    assert_eq!(attachments.len(), 1);
    assert!(attachments[0].attachment.byte_size < bytes.len() as i64);
}

#[tokio::test]
async fn detail_paste_failure_stays_visible_without_metadata() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    create_and_select_task(&mut app, test_task_draft("invalid image target")).await;
    let image = dir.path().join("invalid.png");
    std::fs::write(&image, b"not an image").unwrap();
    app.show_detail(0);

    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;

    let pending = app.attachment_controller.views();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].status,
        crate::tui::attachment_controller::PendingAttachmentStatus::Failed
    );
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("image attachment failed")
    );
    let mut conn = pool.acquire().await.unwrap();
    let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(attachment_count, 0);
}

#[tokio::test]
async fn detail_paste_keeps_target_across_refresh_and_task_switch() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let first = create_and_select_task(&mut app, test_task_draft("first target")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second target")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    assert!(app.select_task_by_id(&first_id));
    let image = dir.path().join("switch.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    app.show_detail(0);

    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    assert!(app.select_task_by_id(&second_id));
    app.refresh().await.unwrap();
    assert_eq!(app.attachment_controller.views()[0].task_id, first_id);
    finish_attachment_work(&mut app).await;

    let selected_id = app
        .store
        .selected_task(app.list.selected_task())
        .unwrap()
        .task
        .id
        .clone();
    assert_eq!(selected_id, second_id);
    let mut conn = pool.acquire().await.unwrap();
    let first_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_attachments WHERE task_id = ?")
            .bind(&first_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let second_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_attachments WHERE task_id = ?")
            .bind(&second_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!((first_count, second_count), (1, 0));
}

#[tokio::test]
async fn background_database_failure_cleans_staged_content() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    let db_path = dir.path().join("test.db");
    app.set_add_task_db_path(db_path.clone());
    create_and_select_task(&mut app, test_task_draft("failing target")).await;
    let image = dir.path().join("failure.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    app.show_detail(0);
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_background_attachment BEFORE INSERT ON task_attachments
         BEGIN SELECT RAISE(FAIL, 'injected attachment insert failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;

    assert_eq!(
        app.attachment_controller.views()[0].status,
        crate::tui::attachment_controller::PendingAttachmentStatus::Failed
    );
    let blob_dir = crate::config::resolve_blob_dir(&db_path, app.intake.config()).unwrap();
    let object_dir = blob_dir.join("objects").join("sha256");
    let object_count = std::fs::read_dir(object_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(object_count, 0);
    let mut conn = pool.acquire().await.unwrap();
    let inventory_count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_inventory")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(inventory_count, 0);
}

#[tokio::test]
async fn concurrent_pastes_keep_paste_order() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let mut config = crate::config::AppConfig::default();
    config.local.image_optimization = crate::config::ImageOptimizationConfig::Paste;
    app.set_config(config);
    let selected = create_and_select_task(&mut app, test_task_draft("ordered target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let first = dir.path().join("first.png");
    let second = dir.path().join("second.jpg");
    std::fs::write(&first, compressible_png_bytes()).unwrap();
    std::fs::write(&second, jpeg_bytes()).unwrap();
    app.show_detail(0);

    app.dispatch_paste(first.to_str().unwrap()).await.unwrap();
    app.dispatch_paste(second.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;

    let mut conn = pool.acquire().await.unwrap();
    let attachments = crate::operations::attachment_read_items_by_task(
        &mut conn,
        app.store.active_workspace.id.as_str(),
        &task_id,
        false,
    )
    .await
    .unwrap();
    let filenames = attachments
        .iter()
        .map(|item| item.attachment.filename.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(filenames, vec![Some("first.png"), Some("second.jpg")]);
}

#[tokio::test]
async fn attachment_shutdown_finishes_commit_and_clears_pending_state() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    create_and_select_task(&mut app, test_task_draft("shutdown target")).await;
    let image = dir.path().join("shutdown.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    app.show_detail(0);

    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    app.attachment_controller.shutdown().await;

    assert!(app.attachment_controller.views().is_empty());
    let mut conn = pool.acquire().await.unwrap();
    let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(attachment_count, 1);
}

#[tokio::test]
async fn detail_paste_image_path_ignores_existing_image() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let selected = create_and_select_task(&mut app, test_task_draft("image target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let image = dir.path().join("photo.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    app.show_detail(0);

    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    finish_attachment_work(&mut app).await;

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("image already attached")
    );
    let item = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == task_id)
        .unwrap();
    assert!(item.task.description.is_empty());
    let mut conn = pool.acquire().await.unwrap();
    let attachments = crate::operations::attachment_read_items_by_task(
        &mut conn,
        app.store.active_workspace.id.as_str(),
        &task_id,
        false,
    )
    .await
    .unwrap();
    assert_eq!(attachments.len(), 1);
}

#[tokio::test]
async fn add_task_paste_image_path_attaches_to_created_task_once() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let image = dir.path().join("composer.png");
    let image_bytes = compressible_png_bytes();
    std::fs::write(&image, &image_bytes).unwrap();

    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "Write docs").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    type_chars(&mut app, "Include setup details").await;
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if state.focus == crate::tui::authoring::AddTaskStep::Description
                && state.description.lines.join("\n") == "Include setup details"
                && state.attachments.len() == 1
                && state.attachments[0].byte_size == image_bytes.len() as i64
                && state.attachments[0].dimensions == Some((16, 16))
    ));
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(toast_message(&app).is_some_and(|message| message.starts_with("created task ")));
    let selected = app.list.selected_task().unwrap();
    let item = &app.store.tasks[selected];
    assert_eq!(item.task.title, "Write docs");
    assert_eq!(item.task.description, "Include setup details");
    let mut conn = pool.acquire().await.unwrap();
    let attachments = crate::operations::attachment_read_items_by_task(
        &mut conn,
        app.store.active_workspace.id.as_str(),
        &item.task.id,
        false,
    )
    .await
    .unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0].attachment.filename.as_deref(),
        Some("composer.png")
    );
    assert_eq!(
        attachments[0].attachment.alt_text.as_deref(),
        Some("pasted image")
    );
    assert!(attachments[0].has_blob);
}

#[tokio::test]
async fn add_task_removing_draft_image_preserves_fields_without_database_or_sync_changes() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let image = dir.path().join("remove.png");
    let retained = dir.path().join("retained.jpg");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    std::fs::write(&retained, jpeg_bytes()).unwrap();
    let changes_before: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
        .fetch_one(&pool)
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "Keep this title").await;
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    app.dispatch_paste(retained.to_str().unwrap())
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Up)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Left)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('D')))
        .await
        .unwrap();

    assert_eq!(app.authoring.add_task_attachments().len(), 1);
    let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
        panic!("add task composer should remain open");
    };
    assert_eq!(state.title.as_str(), "Keep this title");
    assert_eq!(state.focus, crate::tui::authoring::AddTaskStep::Images);
    assert_eq!(
        state
            .attachments
            .iter()
            .map(|attachment| attachment.filename.as_str())
            .collect::<Vec<_>>(),
        vec!["retained.jpg"]
    );
    assert_eq!(state.selected_attachment, 0);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("removed draft image remove.png")
    );
    app.handle_overlay_key(key(KeyCode::Char('D')))
        .await
        .unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
        panic!("add task composer should remain open");
    };
    assert_eq!(state.focus, crate::tui::authoring::AddTaskStep::Title);
    assert!(state.attachments.is_empty());
    let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
        .fetch_one(&pool)
        .await
        .unwrap();
    let changes_after: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(attachment_count, 0);
    assert_eq!(changes_after, changes_before);
}

#[tokio::test]
async fn add_task_attachment_only_dismissal_confirms_and_discards_draft() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let image = dir.path().join("discard.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();

    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
        panic!("add task composer should remain open");
    };
    assert_eq!(state.attachments[0].filename, "discard.png");

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::AddTask(ref state))
            if state.mode == crate::tui::overlay::AddTaskMode::ConfirmDiscard
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.authoring.add_task_attachments().is_empty());
    let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(task_count, 0);
}

#[tokio::test]
async fn failed_attachment_task_submission_preserves_composer_for_retry() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let image = dir.path().join("retry.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();

    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "Retry attachment task").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    type_chars(&mut app, "Keep every field").await;
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_attachment_insert BEFORE INSERT ON task_attachments
         BEGIN SELECT RAISE(FAIL, 'injected attachment insert failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    assert!(app.handle_overlay_key(ctrl_s()).await.is_err());
    assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
    assert_eq!(app.authoring.add_task_attachments().len(), 1);
    let Some(OverlayState::AddTask(state)) = app.overlay.as_ref() else {
        panic!("add task composer should remain open");
    };
    assert_eq!(state.title.as_str(), "Retry attachment task");
    assert_eq!(state.description.lines.join("\n"), "Keep every field");
    let mut conn = pool.acquire().await.unwrap();
    let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(task_count, 0);
    sqlx::query("DROP TRIGGER fail_attachment_insert")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    app.handle_overlay_key(ctrl_s()).await.unwrap();
    let selected = app.list.selected_task().unwrap();
    let item = &app.store.tasks[selected];
    assert_eq!(item.task.title, "Retry attachment task");
    let mut conn = pool.acquire().await.unwrap();
    let attachment_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_attachments WHERE task_id = ?")
            .bind(&item.task.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(attachment_count, 1);
}

#[tokio::test]
async fn failed_epic_child_attachment_submission_retains_owned_origin_for_retry() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let epic = crate::tui::store::EpicContext {
        epic_id: app.store.tasks[parent_index].task.id.clone(),
        display_ref: app.store.tasks[parent_index].display_ref.clone(),
        project_key: app.store.tasks[parent_index].task.project_key.clone(),
    };
    let return_search = SearchState::for_intent(SearchIntent::AddEpicChild {
        epic_id: epic.epic_id.clone(),
        display_ref: epic.display_ref.clone(),
        project_key: epic.project_key.clone(),
    });
    app.authoring
        .begin_epic_child(epic.clone(), return_search, "Retry child".to_string());
    app.begin_add_task_title();
    let image = dir.path().join("child-retry.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_child_attachment_insert BEFORE INSERT ON task_attachments
         BEGIN SELECT RAISE(FAIL, 'injected child attachment failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
    assert_eq!(app.authoring.add_task_attachments().len(), 1);
    assert!(matches!(
        app.authoring
            .submission_context()
            .map(|context| context.origin),
        Some(AddTaskOrigin::EpicChild { .. })
    ));
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("DROP TRIGGER fail_child_attachment_insert")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    app.handle_overlay_key(ctrl_s()).await.unwrap();

    let parent = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == epic.epic_id)
        .unwrap();
    assert!(
        parent
            .epic_children
            .iter()
            .any(|child| child.title == "Retry child")
    );
    assert!(app.authoring.is_idle());
}

#[tokio::test]
async fn rolled_back_attachment_task_is_retained_for_retry() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    let image = dir.path().join("committed.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();

    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "Committed attachment task").await;
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_undo_insert BEFORE INSERT ON tui_undo_entries
         BEGIN SELECT RAISE(FAIL, 'injected undo failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    let error = app.handle_overlay_key(ctrl_s()).await.unwrap_err();
    assert!(!crate::tui::store::task_creation_committed(&error));
    assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
    assert_eq!(app.authoring.add_task_attachments().len(), 1);
    let mut conn = pool.acquire().await.unwrap();
    let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let attachment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!((task_count, attachment_count), (0, 0));
}

#[tokio::test]
async fn natural_add_paste_image_path_carries_attachment_into_created_task() {
    let (dir, pool, mut app) = test_app_with_pool().await;
    app.set_add_task_db_path(dir.path().join("test.db"));
    configure_task_intake_success(&mut app, dir.path(), "parsed natural task", "model details");
    let image = dir.path().join("natural.webp");
    std::fs::write(&image, compressible_png_bytes()).unwrap();

    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    app.begin_add_task_natural();
    app.dispatch_paste(image.to_str().unwrap()).await.unwrap();
    type_chars(&mut app, "make a task from this").await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();
    finish_task_intake_draft(&mut app).await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    let item = &app.store.tasks[selected];
    assert_eq!(item.task.title, "parsed natural task");
    assert_eq!(item.task.description, "model details");
    let mut conn = pool.acquire().await.unwrap();
    let attachments = crate::operations::attachment_read_items_by_task(
        &mut conn,
        app.store.active_workspace.id.as_str(),
        &item.task.id,
        false,
    )
    .await
    .unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0].attachment.filename.as_deref(),
        Some("natural.webp")
    );
}

#[tokio::test]
async fn natural_add_epic_child_uses_authoring_origin() {
    let (dir, _pool, mut app) = test_app_with_pool().await;
    configure_task_intake_success(&mut app, dir.path(), "Parsed child", "Model details");
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let epic = crate::tui::store::EpicContext {
        epic_id: app.store.tasks[parent_index].task.id.clone(),
        display_ref: app.store.tasks[parent_index].display_ref.clone(),
        project_key: app.store.tasks[parent_index].task.project_key.clone(),
    };
    let return_search = SearchState::for_intent(SearchIntent::AddEpicChild {
        epic_id: epic.epic_id.clone(),
        display_ref: epic.display_ref.clone(),
        project_key: epic.project_key.clone(),
    });
    app.authoring
        .begin_epic_child(epic.clone(), return_search, String::new());
    app.begin_add_task_title();
    app.begin_add_task_natural();
    type_chars(&mut app, "make a child task").await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();
    finish_task_intake_draft(&mut app).await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    let parent = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == epic.epic_id)
        .unwrap();
    assert!(
        parent
            .epic_children
            .iter()
            .any(|child| child.title == "Parsed child")
    );
    assert!(app.authoring.is_idle());
}

fn configure_task_intake_success(
    app: &mut App,
    dir: &std::path::Path,
    title: &str,
    description: &str,
) {
    let command = dir.join("task-intake-success.sh");
    std::fs::write(
        &command,
        format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{{\"title\":\"{}\",\"description\":\"{}\",\"project\":null,\"priority\":\"none\",\"labels\":[]}}'\n",
            title, description
        ),
    )
    .unwrap();
    set_executable(&command);
    let mut config = app.intake.config().clone();
    config.agent.task_intake.command = Some(command.display().to_string());
    config.agent.task_intake.args = Vec::new();
    config.agent.task_intake.timeout_seconds = Some(5);
    app.set_config(config);
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn set_executable(_path: &std::path::Path) {}
