use super::*;

async fn activate_add_device(app: &mut App, typed: &str) {
    app.begin_command().await;
    type_chars(app, typed).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
}

#[tokio::test]
async fn add_device_is_disabled_until_sync_is_set_up() {
    let mut app = test_app().await;
    app.begin_command().await;
    type_chars(&mut app, "add-device").await;
    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("expected command panel");
    };
    assert_eq!(state.candidates.len(), 1);
    assert_eq!(
        state.candidates[0].availability.reason(),
        Some("requires sync setup")
    );

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(!app.invite.work_pending());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":add-device is disabled: requires sync setup")
    );
}

#[tokio::test]
async fn add_device_reports_invitation_failures_without_an_overlay() {
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;

    activate_add_device(&mut app, "pair").await;
    assert!(app.invite.work_pending());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while app.invite.work_pending() {
            app.poll_invite().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("invitation task settles");

    assert!(app.overlay.is_none());
    let message = toast_message(&app).unwrap();
    assert!(message.starts_with("invitation unavailable:"), "{message}");
    assert!(message.contains("sync-not-set-up"), "{message}");
}
