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
    let blank_underlay = &buf[(119, 10)];

    assert_eq!(blank_underlay.bg, Color::Reset);
    assert!(!blank_underlay.modifier.contains(Modifier::DIM));
    assert!(buf.content.iter().any(|cell| {
        cell.bg == Color::Reset
            && cell
                .symbol()
                .chars()
                .any(|character| !character.is_whitespace())
            && cell.modifier.contains(Modifier::DIM)
    }));
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

#[tokio::test]
async fn light_background_renders_with_light_palette() {
    use crate::tui::theme::{Background, Theme};

    let mut app = test_app().await;
    app.set_background(Background::Light);

    let buf = render_app_buffer(&mut app, 120, 30);
    let light = Theme::DEFAULT.light;
    let dark = Theme::DEFAULT.dark;

    assert!(buf.content.iter().any(|cell| cell.fg == light.fg));
    assert!(!buf.content.iter().any(|cell| cell.fg == dark.fg));
    assert_eq!(buf[(119, 10)].bg, Color::Reset);
}

#[tokio::test]
async fn command_panel_toggles_background() {
    use crate::tui::theme::{Background, Theme};

    let mut app = test_app().await;

    for (expected, palette) in [
        (Background::Light, Theme::DEFAULT.light),
        (Background::Dark, Theme::DEFAULT.dark),
    ] {
        app.begin_command().await;
        type_chars(&mut app, "toggle-background").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.background, expected);
        let buf = render_app_buffer(&mut app, 120, 30);
        assert!(buf.content.iter().any(|cell| cell.fg == palette.fg));
    }
}
