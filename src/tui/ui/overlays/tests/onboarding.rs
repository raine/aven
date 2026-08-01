use super::*;

#[test]
fn welcome_card_renders_at_minimum_tui_size() {
    let rendered = render_overlay_view_at(
        OverlayView::Onboarding {
            splash_underlay: false,
        },
        70,
        18,
    );

    assert!(rendered.contains("Welcome to aven"));
    assert!(rendered.contains("Local-first tasks for power users and coding agents."));
    assert!(rendered.contains("Everyday keys"));
    assert!(rendered.contains("Add a task and capture what's on your mind"));
    assert!(rendered.contains("Learn more"));
    assert!(rendered.contains("Agents guide"));
    assert!(rendered.contains("https://aven.raine.dev/agents/"));
    assert!(rendered.contains("Open the command panel"));
    assert!(rendered.contains("https://aven.raine.dev/tui/"));
    assert!(rendered.contains("a create first task"));
    assert!(rendered.contains("? shortcuts"));
    assert!(rendered.contains("Enter explore"));
}

#[test]
fn welcome_card_uses_shared_dialog_chrome() {
    assert_overlay_uses_dialog_chrome(
        OverlayView::Onboarding {
            splash_underlay: false,
        },
        "Welcome to aven",
    );
}

#[test]
fn welcome_card_styles_action_keys() {
    let keys = onboarding_lines_for_test()
        .into_iter()
        .flat_map(styled_key_contents)
        .collect::<Vec<_>>();

    assert!(keys.iter().any(|key| key.trim() == "a"));
    assert!(keys.iter().any(|key| key.trim() == "Enter"));
    assert!(keys.iter().any(|key| key.trim() == "?"));
}
