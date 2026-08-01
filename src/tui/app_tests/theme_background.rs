use super::*;
use ratatui::style::{Color, Modifier};

#[tokio::test]
async fn tui_background_uses_terminal_background_for_main_surface() {
    let mut app = test_app().await;

    let buf = render_app_buffer(&mut app, 120, 30);

    assert_eq!(buf[(119, 10)].bg, Color::Reset);
}

#[tokio::test]
async fn modal_overlay_dims_main_surface_underlay() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Confirm(ConfirmState {
        intent: ConfirmIntent::InitializeConfig {
            path: std::path::PathBuf::from("/tmp/config.toml"),
        },
        title: "Confirm".to_string(),
        prompt: "Continue?".to_string(),
    }));

    let buf = render_app_buffer(&mut app, 120, 30);
    let underlay = &buf[(119, 10)];

    assert_eq!(underlay.bg, Color::Rgb(10, 11, 10));
    assert!(underlay.modifier.contains(Modifier::DIM));
}

#[tokio::test]
async fn popover_menu_keeps_main_surface_underlay_bright() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::OrderMenu(
        crate::tui::overlay::OrderMenuState {
            column: 0,
            row: 0,
            selected: TaskOrder::Created,
        },
    ));

    let buf = render_app_buffer(&mut app, 120, 30);
    let underlay = &buf[(119, 10)];

    assert_eq!(underlay.bg, Color::Reset);
    assert!(!underlay.modifier.contains(Modifier::DIM));
}
