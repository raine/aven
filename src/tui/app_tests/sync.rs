use super::*;

#[tokio::test]
async fn sync_now_requires_setup() {
    let mut app = test_app().await;

    app.execute(Action::SyncNow).await.unwrap();

    let message = toast_message(&app).unwrap();
    assert!(message.starts_with("sync unavailable:"), "{message}");
    assert!(message.contains(":sync"), "{message}");
    assert!(!app.sync.work_pending());
}

#[tokio::test]
async fn sync_now_runs_the_encrypted_drain_and_reports_its_failure() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetUp;
    app.show_sync_dialog();

    // Enter runs the focused Sync now action.
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Sync(_))));
    assert!(app.sync.work_pending());
    assert!(matches!(
        app.notification,
        Some(Notification::Loading { .. })
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while app.sync.work_pending() {
            app.poll_sync().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("sync task settles");

    // The database was never set up, so the shared drain refuses it. The
    // toast stays plain; the dialog keeps the engine error as details.
    let message = toast_message(&app).unwrap();
    assert!(message.starts_with("sync failed:"), "{message}");
    assert!(!message.contains("error "), "{message}");
    let failure = last_failure(&app);
    assert_eq!(
        failure.kind,
        crate::tui::sync_operations::OperationKind::Sync
    );
    assert!(failure.details.contains("sync-not-set-up"));
}

async fn run_command(app: &mut App, typed: &str) {
    app.begin_command().await;
    type_chars(app, typed).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
}

#[tokio::test]
async fn sync_command_and_compatibility_alias_open_the_dialog_without_syncing() {
    for typed in ["sync", "config-status"] {
        let mut app = test_app().await;
        app.store.sync_status.set_up = true;
        app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetUp;

        run_command(&mut app, typed).await;

        assert!(
            matches!(app.overlay, Some(OverlayState::Sync(_))),
            ":{typed} opens Sync"
        );
        assert!(!app.sync.work_pending(), ":{typed} must not sync");
    }
}

#[tokio::test]
async fn sync_shortcut_still_syncs_immediately() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;

    app.handle_normal_key(KeyCode::Char('S')).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.sync.work_pending());
}

#[tokio::test]
async fn sync_dialog_add_device_hands_off_to_the_invitation_flow() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetUp;
    app.show_sync_dialog();

    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.invite.work_pending());
}

fn sync_page(app: &App) -> &crate::tui::overlay::SyncPage {
    let Some(OverlayState::Sync(state)) = &app.overlay else {
        panic!("expected sync dialog, got {:?}", app.overlay);
    };
    &state.page
}

async fn paste(app: &mut App, text: &str) {
    app.dispatch_paste(text).await.unwrap();
}

async fn settle_operation(app: &mut App) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while app.sync_ops.work_pending() {
            app.poll_sync_operations().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("sync operation settles");
}

fn last_failure(app: &App) -> &crate::tui::sync_operations::OperationFailure {
    match &app.sync_ops.activity.last {
        Some(crate::tui::sync_operations::OperationResult::Failed(failure)) => failure,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn joining_refuses_a_nonempty_database_without_changing_it() {
    let mut app = test_app().await;
    let database = app.store.database();
    database.create_workspace("Work").await.unwrap();
    let before = database.sync_persistence_status().await.unwrap();
    app.show_sync_dialog();

    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Home);
    assert!(!app.sync_ops.work_pending());
    assert_eq!(
        last_failure(&app).message,
        crate::tui::sync_errors::JOIN_REQUIRES_EMPTY
    );
    assert!(database.enrollment_pin().await.unwrap().is_none());
    assert_eq!(
        crate::sync::encrypted::local_phase(&database)
            .await
            .unwrap(),
        crate::sync::encrypted::LocalPhase::NotSetUp
    );
    let after = database.sync_persistence_status().await.unwrap();
    assert_eq!(after.pending_changes, before.pending_changes);
}

#[tokio::test]
async fn joining_validates_confirms_and_keeps_running_after_the_dialog_closes() {
    let mut app = test_app().await;
    let (setup, device) = crate::sync::encrypted::sample_invitations("https://sync.example.com");
    app.show_sync_dialog();
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        sync_page(&app),
        crate::tui::overlay::SyncPage::Invitation { .. }
    ));

    paste(&mut app, &setup).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let crate::tui::overlay::SyncPage::Invitation { error, .. } = sync_page(&app) else {
        panic!("expected the invitation form");
    };
    assert!(error.is_some_and(|error| error.contains("choose Set up sync")));

    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    paste(&mut app, &device).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let crate::tui::overlay::SyncPage::ConfirmJoin { server, .. } = sync_page(&app) else {
        panic!("expected join confirmation");
    };
    assert_eq!(server, "https://sync.example.com");
    assert!(!app.sync_ops.work_pending());

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Home);
    assert!(app.sync_ops.work_pending());

    // Closing hides progress; it does not cancel joining.
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(app.sync_ops.work_pending());

    // Local edits wait until joined tasks are installed.
    app.execute(Action::BeginAddTask).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(toast_message(&app).is_some_and(|message| message.starts_with("joining sync")));
    app.execute(Action::SyncNow).await.unwrap();
    assert!(!app.sync.work_pending());

    settle_operation(&mut app).await;
    let failure = last_failure(&app);
    assert_eq!(
        failure.kind,
        crate::tui::sync_operations::OperationKind::Join
    );
    assert!(!failure.details.contains(device.as_str()));
    assert!(toast_message(&app).is_some_and(|message| message.contains("open :sync")));
}

#[tokio::test]
async fn setup_previews_this_database_and_starts_only_after_confirmation() {
    let mut app = test_app().await;
    app.store.database().create_workspace("Work").await.unwrap();
    let (setup, device) = crate::sync::encrypted::sample_invitations("https://sync.example.com");
    app.show_sync_dialog();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    paste(&mut app, &device).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let crate::tui::overlay::SyncPage::Invitation { error, .. } = sync_page(&app) else {
        panic!("expected the invitation form");
    };
    assert!(error.is_some_and(|error| error.contains("choose Join existing sync")));

    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    paste(&mut app, &setup).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let crate::tui::overlay::SyncPage::ConfirmSetup {
        server, preview, ..
    } = sync_page(&app)
    else {
        panic!("expected setup confirmation");
    };
    assert_eq!(server, "https://sync.example.com");
    assert_eq!(preview.workspaces, 2);

    // Focus starts on Back, which abandons the unsubmitted form.
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Home);
    assert!(!app.sync_ops.work_pending());

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    paste(&mut app, &setup).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Right)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.sync_ops.work_pending());
    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Home);

    settle_operation(&mut app).await;
    assert_eq!(
        last_failure(&app).kind,
        crate::tui::sync_operations::OperationKind::Setup
    );
    assert!(matches!(app.overlay, Some(OverlayState::Sync(_))));
}

#[tokio::test]
async fn interrupted_joining_pauses_local_edits_and_offers_resume() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::JoinIncomplete;

    app.execute(Action::BeginAddTask).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(toast_message(&app).is_some_and(|message| message.starts_with("joining sync")));

    app.show_sync_dialog();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    // Resuming reuses the stored request; no invitation form appears.
    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Home);
    assert!(app.sync_ops.work_pending());
    settle_operation(&mut app).await;
}

#[tokio::test]
async fn interrupted_joining_takes_a_new_invitation_after_confirmation() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::JoinIncomplete;
    let (_, device) = crate::sync::encrypted::sample_invitations("https://sync.example.com");

    app.show_sync_dialog();
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        sync_page(&app),
        crate::tui::overlay::SyncPage::Invitation { .. }
    ));
    paste(&mut app, &device).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let crate::tui::overlay::SyncPage::ConfirmJoin { replace, .. } = sync_page(&app) else {
        panic!("expected join confirmation");
    };
    assert!(*replace);
    assert!(!app.sync_ops.work_pending());

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Home);
    assert!(app.sync_ops.work_pending());
    settle_operation(&mut app).await;
}

fn listed_devices(app: &mut App) -> [u8; 32] {
    use crate::sync::encrypted::{Device, DeviceListing};
    let other = [2; 32];
    app.sync_ops.activity.devices = Some(crate::tui::sync_operations::DeviceSnapshot {
        listing: DeviceListing {
            server: "https://sync.example.com".to_string(),
            key_rotation_pending: false,
            devices: vec![
                Device {
                    id: [1; 32],
                    label: Some("Office Mac".to_string()),
                    current: true,
                    admission_sequence: 0,
                },
                Device {
                    id: other,
                    label: None,
                    current: false,
                    admission_sequence: 2,
                },
            ],
        },
        checked_at: std::time::Instant::now(),
        after_removal: false,
    });
    other
}

#[tokio::test]
async fn manage_devices_checks_with_the_server_and_reports_failures_on_the_page() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetUp;
    app.show_sync_dialog();
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Devices);
    assert_eq!(
        app.sync_ops.activity.running.map(|running| running.kind),
        Some(crate::tui::sync_operations::OperationKind::ListDevices)
    );
    settle_operation(&mut app).await;
    assert_eq!(
        last_failure(&app).kind,
        crate::tui::sync_operations::OperationKind::ListDevices
    );
    assert!(app.sync_ops.activity.devices.is_none());
}

#[tokio::test]
async fn removing_requires_confirmation_targets_the_full_id_and_survives_closing() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetUp;
    let other = listed_devices(&mut app);
    app.overlay = Some(OverlayState::Sync(SyncDialogState::page(
        crate::tui::overlay::SyncPage::Devices,
    )));

    // This device offers no removal.
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Devices);
    assert!(toast_message(&app).is_some_and(|message| message.contains("another device")));

    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        *sync_page(&app),
        crate::tui::overlay::SyncPage::ConfirmRemove { device: other }
    );
    // Enter on the default Cancel returns to the list without removing.
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Devices);
    assert!(!app.sync_ops.work_pending());

    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Right)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        app.sync_ops.activity.running.map(|running| running.kind),
        Some(crate::tui::sync_operations::OperationKind::RemoveDevice(
            other
        ))
    );

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(app.sync_ops.work_pending());

    settle_operation(&mut app).await;
    let failure = last_failure(&app);
    assert_eq!(
        failure.kind,
        crate::tui::sync_operations::OperationKind::RemoveDevice(other)
    );
    assert!(toast_message(&app).is_some_and(|message| message.contains("device removal")));

    // The failed removal resumes the same target, not a new one.
    app.show_sync_dialog();
    app.overlay = Some(OverlayState::Sync(SyncDialogState::page(
        crate::tui::overlay::SyncPage::Devices,
    )));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        app.sync_ops.activity.running.map(|running| running.kind),
        Some(crate::tui::sync_operations::OperationKind::RemoveDevice(
            other
        ))
    );
    settle_operation(&mut app).await;
}
