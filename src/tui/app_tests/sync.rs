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
    app.show_config_status().unwrap();

    app.handle_overlay_key(key(KeyCode::Char('S')))
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::SyncStatus(_))));
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
