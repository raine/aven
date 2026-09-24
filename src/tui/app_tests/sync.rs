use super::*;

#[tokio::test]
async fn sync_now_requires_setup() {
    let mut app = test_app().await;

    app.execute(Action::SyncNow).await.unwrap();

    let message = toast_message(&app).unwrap();
    assert!(message.starts_with("sync unavailable:"), "{message}");
    assert!(message.contains("aven sync setup"), "{message}");
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

    // The database was never set up, so the shared drain refuses it.
    let message = toast_message(&app).unwrap();
    assert!(message.starts_with("sync failed:"), "{message}");
    assert!(message.contains("sync-not-set-up"), "{message}");
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
