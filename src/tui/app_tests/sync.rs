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
    app.store.sync_status.enabled = true;
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
    let message = &last_failure(&app).message;
    assert!(message.contains("needs an empty database"), "{message}");
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
    // Continue stays blocked; the field names the right choice.
    assert!(error.is_none());

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
    // Continue stays blocked; the field names the right choice.
    assert!(error.is_none());

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
    app.store.sync_status.enabled = true;
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

fn record_setup_refusal(app: &mut App, code: &str) {
    app.sync_ops.activity.last = Some(crate::tui::sync_operations::OperationResult::Failed(
        crate::tui::sync_errors::failure(
            crate::tui::sync_operations::OperationKind::Setup,
            &anyhow::anyhow!("error {code}"),
        ),
    ));
}

#[tokio::test]
async fn enter_on_back_after_a_setup_refusal_returns_to_the_usual_choices() {
    let mut app = test_app().await;
    record_setup_refusal(&mut app, "sync-setup-storage-already-claimed");
    app.show_sync_dialog();

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.sync_ops.activity.last.is_none());
    let Some(OverlayState::Sync(state)) = &app.overlay else {
        panic!("expected the Sync dialog");
    };
    let actions =
        crate::tui::overlay::sync_actions(state, &app.store.sync_status, &app.sync_ops.activity);
    assert_eq!(
        actions,
        [
            crate::tui::overlay::SyncAction::SetUp,
            crate::tui::overlay::SyncAction::Join
        ]
    );
}

#[tokio::test]
async fn a_refused_setup_invitation_offers_pasting_a_new_one() {
    for code in [
        "sync-setup-invitation-rejected",
        "sync-setup-invitation-expired",
    ] {
        let mut app = test_app().await;
        record_setup_refusal(&mut app, code);
        app.show_sync_dialog();

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(
            matches!(
                sync_page(&app),
                crate::tui::overlay::SyncPage::Invitation {
                    kind: crate::tui::overlay::InvitationKind::Setup,
                    ..
                }
            ),
            "{code}"
        );
    }
}

#[tokio::test]
async fn enter_on_back_closes_the_dialog_when_recovery_is_required() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetupRecoveryRequired;
    app.show_sync_dialog();

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
}

fn select_add_device(app: &mut App) {
    let Some(OverlayState::Sync(state)) = &mut app.overlay else {
        panic!("expected the Sync dialog");
    };
    let actions =
        crate::tui::overlay::sync_actions(state, &app.store.sync_status, &app.sync_ops.activity);
    state.selected = actions
        .iter()
        .position(|action| *action == crate::tui::overlay::SyncAction::AddDevice)
        .expect("Add device is offered");
}

#[tokio::test]
async fn add_device_shows_loading_then_the_qr_code_without_closing() {
    use crate::tui::overlay::PairingOverlay;
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.enabled = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetUp;
    let (_, invitation) = crate::sync::encrypted::sample_invitations("https://sync.example.com");
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let pending = crate::sync::encrypted::PendingInvitation::for_test(
        "https://sync.example.com",
        &invitation,
        crate::sync::encrypted::unix_now().unwrap() + 600,
    );
    app.invite.create_with_for_test(tokio::spawn(async move {
        released.await.ok();
        Ok(pending)
    }));
    app.show_sync_dialog();
    select_add_device(&mut app);

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Pairing(PairingOverlay::Creating { .. }))
    ));
    app.poll_invite().await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Pairing(PairingOverlay::Creating { .. }))
    ));

    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let changed = app.poll_invite().await.unwrap();
            match &app.overlay {
                Some(OverlayState::Pairing(PairingOverlay::Creating { .. })) => {}
                Some(OverlayState::Pairing(PairingOverlay::Ready(_))) if changed => break,
                other => panic!("the page left the loading state for {other:?}"),
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("invitation is created");
    assert!(app.store.sync_status.invitation.is_some());
}

#[tokio::test]
async fn esc_on_add_device_returns_to_the_sync_page_and_keeps_waiting() {
    use crate::tui::overlay::PairingOverlay;
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.enabled = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetUp;
    let (_, invitation) = crate::sync::encrypted::sample_invitations("https://sync.example.com");
    let pending = crate::sync::encrypted::PendingInvitation::for_test(
        "https://sync.example.com",
        &invitation,
        crate::sync::encrypted::unix_now().unwrap() + 600,
    );
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    app.invite.create_with_for_test(tokio::spawn(async move {
        released.await.ok();
        Ok(pending)
    }));
    app.show_sync_dialog();
    select_add_device(&mut app);
    let Some(OverlayState::Sync(origin)) = app.overlay.clone() else {
        panic!("expected the Sync dialog");
    };

    // Back from the loading state leaves creation running.
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert_eq!(app.overlay, Some(OverlayState::Sync(origin.clone())));
    assert!(app.invite.work_pending());

    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while app.store.sync_status.invitation.is_none() {
            app.poll_invite().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("invitation is created");
    assert_eq!(app.overlay, Some(OverlayState::Sync(origin.clone())));

    // Back from the QR code keeps the invitation waiting for admission.
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Pairing(PairingOverlay::Ready(_)))
    ));
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert_eq!(app.overlay, Some(OverlayState::Sync(origin)));
    assert!(app.invite.work_pending());
    assert!(app.store.sync_status.invitation.is_some());
}

fn select_sync_automatically(app: &mut App) {
    let Some(OverlayState::Sync(state)) = &mut app.overlay else {
        panic!("sync dialog is open");
    };
    state.selected =
        crate::tui::overlay::sync_actions(state, &app.store.sync_status, &app.sync_ops.activity)
            .iter()
            .position(|action| *action == crate::tui::overlay::SyncAction::SyncAutomatically)
            .expect("sync automatically is offered");
}

static INSTALLED_DB: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

fn record_install(
    args: crate::daemon::ServiceInstallArgs,
) -> anyhow::Result<crate::daemon::InstalledService> {
    assert!(args.config.sync.enabled);
    *INSTALLED_DB.lock().unwrap() = Some(args.db_path);
    Ok(crate::daemon::InstalledService {
        path: std::path::PathBuf::from("/service"),
        logs: String::new(),
    })
}

fn refuse_install(
    _args: crate::daemon::ServiceInstallArgs,
) -> anyhow::Result<crate::daemon::InstalledService> {
    panic!("the service must not be installed");
}

async fn set_up_app_offering_automatic_sync() -> (App, tempfile::TempDir) {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;
    app.store.sync_status.phase = crate::sync::encrypted::LocalPhase::SetUp;
    app.show_sync_dialog();
    select_sync_automatically(&mut app);
    (app, tempfile::tempdir().unwrap())
}

#[tokio::test]
async fn sync_automatically_enables_sync_and_installs_the_service_for_this_database() {
    let (mut app, dir) = set_up_app_offering_automatic_sync().await;
    let config_path = dir.path().join("config.yaml");
    std::fs::write(&config_path, "# mine\nsync:\n  interval_seconds: 60\n").unwrap();

    app.turn_on_automatic_sync_at(crate::tui::app_sync_dialog::AutomaticSyncTarget {
        config_path: config_path.clone(),
        service_db: Some(app.store.database_path().to_path_buf()),
        install: record_install,
    })
    .await
    .unwrap();

    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(text.contains("# mine"), "{text}");
    assert!(text.contains("enabled: true"), "{text}");
    assert!(text.contains("interval_seconds: 60"), "{text}");
    assert!(app.intake.config().sync.enabled);
    assert!(app.store.sync_status.enabled);
    assert_eq!(
        INSTALLED_DB.lock().unwrap().as_deref(),
        Some(app.store.database_path())
    );
    assert_eq!(toast_message(&app).unwrap(), "automatic sync is on");
}

#[tokio::test]
async fn sync_automatically_leaves_the_service_alone_for_another_database() {
    let (mut app, dir) = set_up_app_offering_automatic_sync().await;
    let config_path = dir.path().join("config.yaml");

    app.turn_on_automatic_sync_at(crate::tui::app_sync_dialog::AutomaticSyncTarget {
        config_path: config_path.clone(),
        service_db: Some(dir.path().join("default.sqlite")),
        install: refuse_install,
    })
    .await
    .unwrap();

    assert!(
        std::fs::read_to_string(&config_path)
            .unwrap()
            .contains("enabled: true")
    );
    let message = toast_message(&app).unwrap();
    assert!(message.contains("--db"), "{message}");
    assert!(message.contains("daemon install"), "{message}");
}

#[tokio::test]
async fn sync_automatically_without_a_service_manager_points_to_the_daemon() {
    let (mut app, dir) = set_up_app_offering_automatic_sync().await;

    app.turn_on_automatic_sync_at(crate::tui::app_sync_dialog::AutomaticSyncTarget {
        config_path: dir.path().join("config.yaml"),
        service_db: None,
        install: refuse_install,
    })
    .await
    .unwrap();

    assert!(app.intake.config().sync.enabled);
    let message = toast_message(&app).unwrap();
    assert!(message.contains("`aven daemon`"), "{message}");
}

#[tokio::test]
async fn sync_automatically_asks_before_changing_anything() {
    let (mut app, _dir) = set_up_app_offering_automatic_sync().await;

    // Choosing the action only opens the confirmation; the real target
    // would panic under test if anything were written or installed.
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        sync_page(&app),
        crate::tui::overlay::SyncPage::ConfirmAutomaticSync { .. }
    ));
    assert!(!app.intake.config().sync.enabled);

    // Focus starts on Back.
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(*sync_page(&app), crate::tui::overlay::SyncPage::Home);
    assert!(!app.intake.config().sync.enabled);
}
