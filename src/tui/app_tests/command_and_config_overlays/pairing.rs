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
async fn add_device_reports_invitation_failures_on_its_page() {
    use crate::tui::overlay::PairingOverlay;
    let mut app = test_app().await;
    app.store.sync_status.set_up = true;

    activate_add_device(&mut app, "pair").await;
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Pairing(PairingOverlay::Creating { .. }))
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while app.invite.work_pending() {
            app.poll_invite().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("invitation task settles");

    let Some(OverlayState::Pairing(PairingOverlay::Failed(message))) = &app.overlay else {
        panic!("expected the failure on the Add device page");
    };
    assert!(
        message.contains("Sync isn't set up for this database"),
        "{message}"
    );

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Pairing(PairingOverlay::Creating { .. }))
    ));
    assert!(app.invite.work_pending());
}

#[tokio::test]
async fn back_from_a_failed_add_device_returns_to_the_sync_dialog() {
    use crate::tui::overlay::PairingOverlay;
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Pairing(PairingOverlay::Failed("x".into())));

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(matches!(app.overlay, Some(OverlayState::Sync(_))));
}

#[tokio::test]
async fn copy_invitation_writes_the_clipboard_only_when_asked() {
    let mut app = test_app().await;
    let (_, invitation) = crate::sync::encrypted::sample_invitations("https://sync.example.com");
    let presentation = std::sync::Arc::new(
        crate::pairing::PairingPresentation::new(
            "https://sync.example.com",
            &invitation,
            0,
            crate::pairing::QrGlyphs::HalfBlock,
        )
        .unwrap(),
    );
    let before = crate::tui::platform::clipboard_text_for_test();
    app.invite.show_for_test(presentation, &invitation);
    app.show_pairing_invitation();
    assert!(matches!(app.overlay, Some(OverlayState::Pairing(_))));
    // Showing the QR code writes nothing to the clipboard.
    assert_eq!(crate::tui::platform::clipboard_text_for_test(), before);

    app.handle_overlay_key(key(KeyCode::Char('c')))
        .await
        .unwrap();

    assert!(matches!(app.overlay, Some(OverlayState::Pairing(_))));
    assert_eq!(
        crate::tui::platform::clipboard_text_for_test().as_deref(),
        Some(invitation.as_str())
    );
    let message = toast_message(&app).unwrap();
    assert!(message.contains("grants access"), "{message}");
    assert!(!message.contains("aven://"), "{message}");
}

#[tokio::test]
async fn copy_invitation_without_a_waiting_invitation_copies_nothing() {
    let mut app = test_app().await;
    let before = crate::tui::platform::clipboard_text_for_test();

    app.copy_pairing_invitation();

    assert_eq!(crate::tui::platform::clipboard_text_for_test(), before);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no invitation is waiting")
    );
}
