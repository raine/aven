use super::*;

use crate::tui::overlay::OverlayView;
use crate::tui::store::OnboardingStatus;

#[tokio::test]
async fn automatic_welcome_completes_once() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;

    app.maybe_open_onboarding().await;
    assert!(app.onboarding_intro.is_some());
    assert!(matches!(
        app.view().overlay,
        Some(OverlayView::Onboarding {
            splash_underlay: true
        })
    ));
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Onboarding {
            persist_on_exit: true
        })
    ));

    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(
        app.store.onboarding_status().await.unwrap(),
        OnboardingStatus::Complete
    );

    app.maybe_open_onboarding().await;
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn welcome_actions_open_the_first_use_flows() {
    let mut add_app = test_app().await;
    add_app.maybe_open_onboarding().await;
    add_app
        .dispatch_key(key(KeyCode::Char('a')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(add_app.overlay, Some(OverlayState::AddTask(_))));

    let mut help_app = test_app().await;
    help_app.maybe_open_onboarding().await;
    help_app
        .dispatch_key(shift_key(KeyCode::Char('?')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        help_app.overlay,
        Some(OverlayState::Help { scroll: 0 })
    ));
}

#[tokio::test]
async fn forced_welcome_uses_automatic_intro() {
    let mut app = test_app().await;

    app.show_welcome_intro();
    assert!(app.onboarding_intro.is_some());
    assert!(matches!(
        app.view().overlay,
        Some(OverlayView::Onboarding {
            splash_underlay: true
        })
    ));
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Onboarding {
            persist_on_exit: true
        })
    ));
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store.onboarding_status().await.unwrap(),
        OnboardingStatus::Complete
    );
}

#[tokio::test]
async fn welcome_replay_does_not_complete_automatic_onboarding() {
    let mut app = test_app().await;

    app.execute(Action::ShowWelcome).await.unwrap();
    assert!(app.onboarding_intro.is_none());
    assert!(matches!(
        app.view().overlay,
        Some(OverlayView::Onboarding {
            splash_underlay: false
        })
    ));
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Onboarding {
            persist_on_exit: false
        })
    ));
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store.onboarding_status().await.unwrap(),
        OnboardingStatus::Due
    );
}

#[tokio::test]
async fn hidden_welcome_ignores_actions_and_modifiers() {
    let mut app = test_app().await;
    app.maybe_open_onboarding().await;

    app.dispatch_key(key(KeyCode::Enter), (69, 17).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Onboarding { .. })));

    app.dispatch_key(ctrl_a(), (80, 24).into()).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Onboarding { .. })));
    assert_eq!(
        app.store.onboarding_status().await.unwrap(),
        OnboardingStatus::Due
    );
}

#[tokio::test]
async fn welcome_quit_completes_but_control_c_does_not() {
    let mut app = test_app().await;
    app.maybe_open_onboarding().await;
    app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
        .await
        .unwrap();
    assert!(app.should_quit);
    assert_eq!(
        app.store.onboarding_status().await.unwrap(),
        OnboardingStatus::Complete
    );

    let mut interrupted = test_app().await;
    interrupted.maybe_open_onboarding().await;
    interrupted
        .dispatch_key(ctrl_c(), (80, 24).into())
        .await
        .unwrap();
    assert!(interrupted.should_quit);
    assert_eq!(
        interrupted.store.onboarding_status().await.unwrap(),
        OnboardingStatus::Due
    );
}

#[test]
fn welcome_command_is_discoverable() {
    assert!(
        crate::tui::event::COMMANDS
            .iter()
            .any(|command| { command.name == "welcome" && command.action == Action::ShowWelcome })
    );
}

#[tokio::test]
async fn update_review_supports_action_focus_and_later() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Update(
        crate::tui::overlay::UpdateOverlayState::Available {
            plan: crate::update::InstallPlan {
                release: crate::update::Release {
                    version: semver::Version::new(99, 0, 0),
                    tag: "v99.0.0".to_string(),
                    archive_name: "aven-test.tar.gz".to_string(),
                    archive_url: "https://example.com/aven-test.tar.gz".to_string(),
                    checksum_url: "https://example.com/aven-test.sha256".to_string(),
                },
                method: crate::update::InstallMethod::Direct {
                    target: "/tmp/aven".into(),
                },
            },
            notes: crate::tui::overlay::UpdateNotesState::Ready(
                "## v99.0.0\n\n- release note".to_string(),
            ),
            scroll: 0,
            focus: crate::tui::overlay::UpdateActionFocus::Primary,
            cached: false,
        },
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Update(
            crate::tui::overlay::UpdateOverlayState::Available {
                focus: crate::tui::overlay::UpdateActionFocus::Later,
                ..
            }
        ))
    ));

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn changelog_command_starts_loading_release_notes_from_github() {
    let mut app = test_app().await;

    app.execute(Action::ShowChangelog).await.unwrap();

    let Some(OverlayState::Changelog(state)) = app.overlay else {
        panic!("expected changelog overlay");
    };
    assert_eq!(state.markdown, "## Loading changelog…");
    assert!(app.changelog.work_pending());
}

#[tokio::test]
async fn changelog_link_click_opens_documentation_in_browser() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Changelog(
        crate::tui::overlay::ChangelogState {
            markdown: "## Unreleased\n\n- [Read the guide](/guide/) for details.".to_string(),
            scroll: 0,
        },
    ));

    app.dispatch_mouse(click_at(10, 6), (100, 30).into())
        .await
        .unwrap();

    assert_eq!(
        crate::tui::platform::browser_url_for_test().as_deref(),
        Some("https://aven.raine.dev/guide/")
    );
    assert!(matches!(app.overlay, Some(OverlayState::Changelog(_))));
}

#[tokio::test]
async fn changelog_reader_supports_less_style_paging_and_close() {
    let mut app = test_app().await;
    app.execute(Action::ShowChangelog).await.unwrap();
    app.overlay = Some(OverlayState::Changelog(
        crate::tui::overlay::ChangelogState {
            markdown: format!(
                "## Release notes\n\n{}",
                "- A changelog entry with enough content for paging.\n".repeat(40)
            ),
            scroll: 0,
        },
    ));

    app.handle_overlay_key(key(KeyCode::Char('d')))
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Changelog(ref state)) if state.scroll == 9
    ));
    app.handle_overlay_key(key(KeyCode::Char('u')))
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Changelog(ref state)) if state.scroll == 0
    ));
    app.handle_overlay_key(key(KeyCode::PageDown))
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Changelog(ref state)) if state.scroll == 17
    ));
    app.handle_overlay_key(key(KeyCode::PageUp)).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Changelog(ref state)) if state.scroll == 0
    ));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
}
