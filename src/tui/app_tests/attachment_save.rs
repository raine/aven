use super::*;

async fn setup_attachment() -> (tempfile::TempDir, SqlitePool, App, usize, String) {
    let (dir, pool, mut app) = test_app_with_pool().await;
    let db_path = dir.path().join("test.db");
    let selected = create_and_select_task(&mut app, test_task_draft("save target")).await;
    let attachment_id = add_real_attachment(&mut app, &pool, &db_path, selected).await;
    app.show_detail(4);
    (dir, pool, app, selected, attachment_id)
}

#[tokio::test]
async fn attachment_save_writes_bytes_and_reports_destination() {
    let (dir, _pool, mut app, _selected, attachment_id) = setup_attachment().await;
    let destination = dir.path().join("saved.png");

    app.submit_save_attachment(
        attachment_id,
        "viewer-test.png".to_string(),
        4,
        destination.display().to_string(),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), png_bytes(2, 2));
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    let message = format!("saved attachment to {}", destination.display());
    assert_eq!(toast_message(&app).as_deref(), Some(message.as_str()));
}

#[tokio::test]
async fn attachment_save_can_be_cancelled() {
    let (dir, _pool, mut app, _selected, attachment_id) = setup_attachment().await;
    let destination = dir.path().join("viewer-test.png");
    app.detail.state_mut().unwrap().focused_target =
        Some(DetailTargetId::Attachment { attachment_id });

    app.dispatch_key(key(KeyCode::Char('s')), (100, 30).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::TextInput(_))));
    app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
        .await
        .unwrap();

    assert!(!destination.exists());
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn colon_dispatch_opens_captured_attachment_panel() {
    let (_dir, _pool, mut app, _selected, attachment_id) = setup_attachment().await;
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
        attachment_id: attachment_id.clone(),
    });

    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();

    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("command overlay");
    };
    let names = state
        .candidates
        .iter()
        .filter_map(|candidate| state.catalog.command(candidate.index))
        .map(crate::tui::event::CatalogCommand::name)
        .collect::<Vec<_>>();
    assert_eq!(
        &names[..3],
        &["attachment-open", "attachment-save", "attachment-delete"]
    );
    assert!(matches!(
        state.session.detail_focus(),
        Some(crate::tui::event::DetailCommandFocus::Attachment {
            attachment_id: captured,
            bytes_present: true,
        }) if captured == &attachment_id
    ));
}

#[tokio::test]
async fn command_palette_exposes_and_runs_attachment_save() {
    let (_dir, _pool, mut app, _selected, attachment_id) = setup_attachment().await;
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
        attachment_id: attachment_id.clone(),
    });

    app.dispatch_key(key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    app.detail.state_mut().unwrap().focused_target = None;
    type_chars(&mut app, "attachment-save").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextInput(ref state))
            if matches!(
                state.intent,
                TextIntent::SaveAttachment { attachment_id: ref captured, .. }
                    if captured == &attachment_id
            )
    ));
}

#[tokio::test]
async fn attachment_save_defaults_to_safe_metadata_filename() {
    let (_dir, _pool, mut app, selected, attachment_id) = setup_attachment().await;
    app.store.tasks[selected].attachments[0].filename = Some("../unsafe.png".to_string());

    app.begin_save_attachment(&attachment_id, 4);

    let OverlayState::TextInput(state) = app.overlay.as_ref().unwrap() else {
        panic!("save destination input");
    };
    assert_eq!(
        std::path::Path::new(&state.input.text)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("unsafe.png")
    );
    assert!(matches!(
        state.intent,
        TextIntent::SaveAttachment { ref filename, .. } if filename == "unsafe.png"
    ));

    app.overlay = None;
    app.store.tasks[selected].attachments[0].filename = None;
    app.begin_save_attachment(&attachment_id, 4);
    let OverlayState::TextInput(state) = app.overlay.as_ref().unwrap() else {
        panic!("save destination input");
    };
    let fallback = format!("attachment-{attachment_id}.png");
    assert_eq!(
        std::path::Path::new(&state.input.text)
            .file_name()
            .and_then(|name| name.to_str()),
        Some(fallback.as_str())
    );
    assert!(matches!(
        state.intent,
        TextIntent::SaveAttachment { ref filename, .. } if filename == &fallback
    ));
}

#[tokio::test]
async fn attachment_save_refuses_destination_collision() {
    let (dir, _pool, mut app, _selected, attachment_id) = setup_attachment().await;
    let destination = dir.path().join("existing.png");
    std::fs::write(&destination, b"keep me").unwrap();

    app.submit_save_attachment(
        attachment_id,
        "viewer-test.png".to_string(),
        4,
        destination.display().to_string(),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), b"keep me");
    assert!(matches!(app.overlay, Some(OverlayState::TextInput(_))));
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    assert!(
        toast_message(&app)
            .unwrap()
            .contains("destination already exists")
    );
}

#[tokio::test]
async fn cold_attachment_save_rejects_missing_blob_before_destination_prompt() {
    let (dir, pool, app, selected, attachment_id) = setup_attachment().await;
    let db_path = dir.path().join("test.db");
    let task_id = app.store.tasks[selected].task.id.clone();
    let blob_dir = crate::config::resolve_blob_dir(&db_path, app.intake.config()).unwrap();
    let source = crate::attachments::storage::object_path(
        &blob_dir,
        &app.store.tasks[selected].attachments[0].sha256,
    )
    .unwrap();
    drop(app);
    drop(pool);
    std::fs::remove_file(source).unwrap();

    let database = aven_core::db::Database::open(&db_path).await.unwrap();
    let mut app = App::new_for_tests(database).await.unwrap();
    app.set_add_task_db_path(db_path);
    app.store.refresh(Some(&task_id)).await.unwrap();
    app.list.select_task(Some(0));
    app.show_detail(4);
    app.detail.state_mut().unwrap().focused_target =
        Some(DetailTargetId::Attachment { attachment_id });
    app.dispatch_key(key(KeyCode::Char('s')), (100, 30).into())
        .await
        .unwrap();

    assert!(!matches!(app.overlay, Some(OverlayState::TextInput(_))));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("attachment bytes are unavailable")
    );
}

#[tokio::test]
async fn attachment_save_reports_missing_blob() {
    let (dir, _pool, mut app, selected, attachment_id) = setup_attachment().await;
    let db_path = dir.path().join("test.db");
    let blob_dir = crate::config::resolve_blob_dir(&db_path, app.intake.config()).unwrap();
    let source = crate::attachments::storage::object_path(
        &blob_dir,
        &app.store.tasks[selected].attachments[0].sha256,
    )
    .unwrap();
    std::fs::remove_file(source).unwrap();
    let destination = dir.path().join("missing.png");

    app.submit_save_attachment(
        attachment_id,
        "viewer-test.png".to_string(),
        4,
        destination.display().to_string(),
    )
    .await
    .unwrap();

    assert!(!destination.exists());
    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("attachment bytes are unavailable")
    );
}

#[tokio::test]
async fn attachment_save_reports_destination_errors_for_retry() {
    let (dir, _pool, mut app, _selected, attachment_id) = setup_attachment().await;
    let destination = dir.path().join("missing").join("saved.png");

    app.submit_save_attachment(
        attachment_id,
        "viewer-test.png".to_string(),
        4,
        destination.display().to_string(),
    )
    .await
    .unwrap();

    assert!(!destination.exists());
    assert!(matches!(app.overlay, Some(OverlayState::TextInput(_))));
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    assert!(
        toast_message(&app)
            .unwrap()
            .contains("destination directory is unavailable")
    );
}

#[tokio::test]
async fn attachment_save_reports_write_errors_for_retry() {
    let (dir, _pool, mut app, _selected, attachment_id) = setup_attachment().await;
    let destination = dir.path().join("x".repeat(300));

    app.submit_save_attachment(
        attachment_id,
        "viewer-test.png".to_string(),
        4,
        destination.display().to_string(),
    )
    .await
    .unwrap();

    assert!(!destination.exists());
    assert!(matches!(app.overlay, Some(OverlayState::TextInput(_))));
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(
        toast_message(&app)
            .unwrap()
            .contains("could not save attachment")
    );
}

#[tokio::test]
async fn direct_attachment_save_resolution_requires_captured_bytes() {
    let (_dir, _pool, mut app, _selected, attachment_id) = setup_attachment().await;
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Attachment {
        attachment_id: attachment_id.clone(),
    });
    let mut snapshot = app.capture_command_session(None);
    let crate::tui::event::CommandSurfaceSnapshot::Detail { focus, .. } = &mut snapshot.surface
    else {
        panic!("detail command session");
    };
    *focus = Some(crate::tui::event::DetailCommandFocus::Attachment {
        attachment_id,
        bytes_present: false,
    });
    let command = crate::tui::event::COMMANDS
        .iter()
        .find(|command| command.action == Action::SaveAttachment)
        .unwrap();

    let resolved = app
        .resolve_builtin_command(&snapshot, command)
        .await
        .unwrap();

    assert!(matches!(resolved, Err(reason) if reason == "attachment bytes are unavailable"));
}
