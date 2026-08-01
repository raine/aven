use super::*;

#[test]
fn overlay_render_includes_confirm_prompt_and_hints() {
    let rendered = render_overlay_view(OverlayView::Confirm(ConfirmView {
        title: "Delete".to_string(),
        prompt: "Delete task?".to_string(),
    }));
    assert!(rendered.contains("Delete"));
    assert!(rendered.contains("Delete task?"));
    assert!(rendered.contains("y yes"));
}

#[test]
fn confirm_overlay_wraps_long_prompt() {
    let prompt =
        "Delete WI-2ZB3 Option to track treadmill sessions as HealthKit workouts ".repeat(2);
    let overlay = OverlayView::Confirm(ConfirmView {
        title: "Delete task".to_string(),
        prompt: prompt.clone(),
    });
    let buffer = overlay_buffer(overlay);

    for row in 0..buffer.area.height {
        assert!(!buffer_row(&buffer, row).contains(&prompt));
    }
    assert!(buffer_text_from_rows(&buffer).contains("y yes"));
}

#[test]
fn available_update_combines_release_notes_and_actions() {
    let rendered = render_overlay_view(OverlayView::Update(
        crate::tui::overlay::UpdateOverlayState::Available {
            plan: crate::update::InstallPlan {
                release: crate::update::Release {
                    version: semver::Version::new(1, 2, 3),
                    tag: "v1.2.3".to_string(),
                    archive_name: "aven-test.tar.gz".to_string(),
                    archive_url: "https://example.com/aven-test.tar.gz".to_string(),
                    checksum_url: "https://example.com/aven-test.sha256".to_string(),
                },
                method: crate::update::InstallMethod::Direct {
                    target: "/usr/local/bin/aven".into(),
                },
            },
            notes: crate::tui::overlay::UpdateNotesState::Ready(
                "## v1.2.3\n\n- Faster updates\n\n## v1.1.0\n\n- Earlier changes".to_string(),
            ),
            scroll: 0,
            focus: crate::tui::overlay::UpdateActionFocus::Primary,
            cached: false,
        },
    ));

    assert!(rendered.contains("Software Update"));
    assert!(rendered.contains("Aven v1.2.3 is available"));
    assert!(rendered.contains("You have v"));
    assert!(rendered.contains("Changelog"));
    assert!(rendered.contains("Faster updates"));
    assert!(rendered.contains("Earlier changes"));
    assert!(rendered.contains("Later"));
    assert!(rendered.contains("Update"));
}

#[test]
fn update_overlay_explains_restart_and_cancellation() {
    let success = render_overlay_view(OverlayView::Update(
        crate::tui::overlay::UpdateOverlayState::Success {
            version: "1.2.3".to_string(),
        },
    ));
    assert!(success.contains("Installed aven v1.2.3"));
    assert!(success.contains("Restart aven"));
    assert!(success.contains("q quit"));

    let lines = update_lines_for_test(&crate::tui::overlay::UpdateOverlayState::Cancelled);
    assert!(lines[0].to_string().contains("cancelled"));

    let current = update_lines_for_test(&crate::tui::overlay::UpdateOverlayState::Current {
        version: "1.2.3".to_string(),
        cached: false,
    });
    assert_eq!(current.len(), 3);
    assert!(current[1].to_string().is_empty());
    assert!(current[2].to_string().contains("Esc close"));
}

fn buffer_text_from_rows(buffer: &ratatui::buffer::Buffer) -> String {
    (0..buffer.area.height)
        .map(|row| buffer_row(buffer, row))
        .collect::<Vec<_>>()
        .join("\n")
}
