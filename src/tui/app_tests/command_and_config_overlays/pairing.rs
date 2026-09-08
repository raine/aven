use super::*;

const TEST_SERVER: &str = "https://sync.example.test:8443/aven";
const TEST_TOKEN: &str = "pairing-token-fixture-0123456789";

fn configure_pairing(app: &mut App, server: &str, token: &str) {
    let mut config = AppConfig::default();
    config.sync.server_url = Some(server.to_string());
    config.sync.auth_token = Some(token.to_string());
    app.store.set_config(config);
}

async fn activate_pairing_command(app: &mut App) {
    app.begin_command().await;
    type_chars(app, "pair-mobile").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
}

#[tokio::test]
async fn command_panel_activates_safe_shared_pairing_presentation() {
    let mut app = test_app().await;
    configure_pairing(&mut app, TEST_SERVER, TEST_TOKEN);

    activate_pairing_command(&mut app).await;

    let Some(OverlayState::Pairing(presentation)) = &app.overlay else {
        panic!("expected pairing overlay");
    };
    assert_eq!(
        presentation.server_identity(),
        "https://sync.example.test:8443"
    );
    let debug = format!("{:?}", app.overlay);
    assert!(!debug.contains(TEST_TOKEN));
    assert!(!debug.contains("aven://pair/"));
}

#[tokio::test]
async fn missing_pairing_inputs_have_safe_disabled_reasons() {
    let mut app = test_app().await;
    app.begin_command().await;
    type_chars(&mut app, "pair-mobile").await;
    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("expected command panel");
    };
    assert_eq!(state.candidates.len(), 1);
    assert_eq!(
        state.candidates[0].availability.reason(),
        Some("configure sync.server_url")
    );

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":pair-mobile is disabled: configure sync.server_url")
    );

    configure_pairing(&mut app, TEST_SERVER, "");
    app.begin_command().await;
    type_chars(&mut app, "pair-mobile").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":pair-mobile is disabled: configure a nonempty sync.auth_token")
    );
}

#[tokio::test]
async fn invalid_and_loopback_servers_map_shared_secret_safe_errors() {
    for (server, expected) in [
        (
            "ssh://sync.example.test",
            "pairing unavailable · sync.server_url must be an http or https URL without credentials, query, or fragment",
        ),
        (
            "http://127.0.0.1:3746",
            "pairing unavailable · sync.server_url must be reachable from the phone",
        ),
    ] {
        let mut app = test_app().await;
        configure_pairing(&mut app, server, TEST_TOKEN);

        activate_pairing_command(&mut app).await;

        assert!(app.overlay.is_none());
        let message = toast_message(&app).unwrap();
        assert_eq!(message, expected);
        assert!(!message.contains(TEST_TOKEN));
        assert!(!message.contains("aven://pair/"));
    }
}

#[tokio::test]
async fn oversized_invitation_maps_shared_capacity_error_safely() {
    let mut app = test_app().await;
    let token = "t".repeat(3000);
    configure_pairing(&mut app, TEST_SERVER, &token);

    activate_pairing_command(&mut app).await;

    assert!(app.overlay.is_none());
    let message = toast_message(&app).unwrap();
    assert_eq!(
        message,
        "pairing unavailable · shorten sync.auth_token or sync.server_url"
    );
    assert!(!message.contains(&token));
    assert!(!message.contains("aven://pair/"));
}

#[tokio::test]
async fn escape_closes_the_pairing_overlay() {
    let mut app = test_app().await;
    configure_pairing(&mut app, TEST_SERVER, TEST_TOKEN);
    activate_pairing_command(&mut app).await;

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn detail_surface_reports_pairing_as_list_only() {
    let mut app = test_app().await;
    configure_pairing(&mut app, TEST_SERVER, TEST_TOKEN);
    create_and_select_task(&mut app, test_task_draft("Detail pairing target")).await;
    app.execute(Action::ToggleDetail).await.unwrap();

    activate_pairing_command(&mut app).await;

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":pair-mobile is disabled: available only in the task list")
    );
}

#[tokio::test]
async fn outside_click_clears_the_pairing_overlay() {
    let mut app = test_app().await;
    configure_pairing(&mut app, TEST_SERVER, TEST_TOKEN);
    activate_pairing_command(&mut app).await;
    let size = ratatui::layout::Size::new(160, 80);
    let Some(OverlayState::Pairing(presentation)) = &app.overlay else {
        panic!("expected pairing overlay");
    };
    let area = crate::tui::ui::pairing_layout(
        ratatui::layout::Rect::new(0, 0, size.width, size.height),
        presentation.as_ref(),
    )
    .area;

    app.dispatch_mouse(left_click(area.x.saturating_sub(1), area.y), size)
        .await
        .unwrap();

    assert!(app.overlay.is_none());
}
