use super::*;
use ratatui::style::{Color, Modifier};

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
    let before = render_app_buffer(&mut app, 120, 30);
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
    let glyphs = before
        .content
        .iter()
        .zip(&buf.content)
        .filter(|(before, after)| {
            before.symbol() == after.symbol() && before.symbol().chars().any(|c| !c.is_whitespace())
        })
        .collect::<Vec<_>>();
    assert!(!glyphs.is_empty());
    assert!(
        glyphs
            .iter()
            .all(|(_, cell)| !cell.modifier.contains(Modifier::DIM))
    );
}

#[tokio::test]
async fn theme_commands_select_explicitly_and_keep_terminal_background() {
    let mut app = test_app().await;
    assert_eq!(
        render_app_buffer(&mut app, 120, 30)[(119, 10)].bg,
        Color::Reset
    );
    for (value, appearance) in [
        ("light", crate::tui::theme::Appearance::Light),
        ("terminal", crate::tui::theme::Appearance::Terminal),
        ("dark", crate::tui::theme::Appearance::Dark),
        ("dark", crate::tui::theme::Appearance::Dark),
    ] {
        app.begin_command().await;
        type_chars(&mut app, &format!("theme {value}")).await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.appearance, appearance);
        assert_eq!(app.theme, appearance.theme());
        let buf = render_app_buffer(&mut app, 120, 30);
        assert_eq!(buf[(119, 10)].bg, Color::Reset);
    }
}

#[tokio::test]
async fn terminal_selection_and_caret_are_uniform_and_color_independent() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Terminal selection")).await;
    app.set_appearance(crate::tui::theme::Appearance::Terminal);
    let buffer = render_app_buffer(&mut app, 120, 30);
    let reversed = buffer
        .content
        .iter()
        .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
        .collect::<Vec<_>>();
    assert!(!reversed.is_empty());
    assert!(
        reversed
            .iter()
            .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
    );
    assert_eq!(app.widgets.text_cursor, None);
    assert!(
        buffer
            .content
            .iter()
            .all(|cell| !matches!(cell.fg, Color::Rgb(..)) && cell.bg == Color::Reset)
    );

    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    type_chars(&mut app, "한글").await;
    let buffer = render_app_buffer(&mut app, 120, 30);
    let caret = app.widgets.text_cursor.expect("active caret");
    assert_eq!(buffer[(caret.x - 4, caret.y)].symbol(), "한");
    assert_eq!(buffer[(caret.x - 2, caret.y)].symbol(), "글");
    assert!(
        buffer[(caret.x, caret.y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        buffer
            .content
            .iter()
            .all(|cell| !cell.modifier.contains(crate::tui::ui::CARET_MARKER))
    );
}

#[tokio::test]
async fn blank_caret_is_invalidated_by_stacked_dialogs_in_every_theme() {
    for appearance in [
        crate::tui::theme::Appearance::Dark,
        crate::tui::theme::Appearance::Light,
        crate::tui::theme::Appearance::Terminal,
    ] {
        let mut app = test_app().await;
        app.set_appearance(appearance);
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "ab").await;
        render_app_buffer(&mut app, 120, 30);
        assert!(app.widgets.text_cursor.is_some());
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        let buffer = render_app_buffer(&mut app, 120, 30);
        assert_eq!(app.widgets.text_cursor, None);
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| !cell.modifier.contains(crate::tui::ui::CARET_MARKER))
        );
        app.handle_overlay_key(key(KeyCode::Char('n')))
            .await
            .unwrap();
        render_app_buffer(&mut app, 120, 30);
        assert!(app.widgets.text_cursor.is_some());
    }
}

#[tokio::test]
async fn terminal_inline_caret_remains_visible_inside_selected_rows_and_cards() {
    for columns in [false, true] {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Inline caret")).await;
        app.set_appearance(crate::tui::theme::Appearance::Terminal);
        if columns {
            app.set_layout(TaskLayout::Columns);
        }
        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        let buffer = render_app_buffer(&mut app, 120, 30);
        let caret = app.widgets.text_cursor.expect("inline caret reported");
        assert_eq!(buffer[(caret.x, caret.y)].symbol(), " ");
        assert!(
            !buffer[(caret.x, caret.y)]
                .modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            buffer[(caret.x - 1, caret.y)]
                .modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| !cell.modifier.contains(crate::tui::ui::CARET_MARKER))
        );
    }
}
