use super::*;
use crate::sync::encrypted::LocalPhase;

#[test]
fn idle_sync_renders_compact_summary_and_actions() {
    let rendered = render_overlay_view(sync_overlay(sync_status(), false));

    assert!(rendered.contains(SYNC_TITLE));
    assert!(!rendered.contains("Sync status"));
    assert!(rendered.contains("No changes waiting"));
    assert!(rendered.contains("end-to-end encrypted"));
    assert!(rendered.contains("automatic"));
    assert!(rendered.contains("pending"));
    assert!(rendered.contains("conflicts"));
    assert!(rendered.contains("› Sync now"));
    assert!(rendered.contains("Add device"));
    assert!(rendered.contains("d details"));
    assert!(!rendered.contains("sync cursor"));
    assert!(!rendered.contains("Up to date"));
}

#[test]
fn details_reveal_internal_diagnostics() {
    let rendered = render_overlay_view(sync_overlay(sync_status(), true));

    assert!(rendered.contains("DETAILS"));
    assert!(rendered.contains("wake address"));
    assert!(rendered.contains("sync cursor"));
    assert!(rendered.contains("local sequence"));
    assert!(rendered.contains("d summary"));
}

#[test]
fn wake_failures_are_visible_without_expanding_details() {
    let mut status = sync_status();
    status.daemon_wake = SyncStatusCheck::new(false, "invalid daemon wake address");
    let state = SyncDialogState::default();
    let lines = sync_dialog_lines_for_test(&sync_view(&state, status));

    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("Sync needs attention"))
    );
    assert!(lines.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.style.fg == Some(ORANGE) && span.content.contains("invalid"))
    }));
    assert!(!lines.iter().any(|line| line.to_string() == "DETAILS"));
}

#[test]
fn unset_up_and_runtime_disabled_states_have_distinct_copy() {
    let local = render_overlay_view(sync_overlay(TuiSyncStatus::default(), false));
    let disabled = render_overlay_view(sync_overlay(
        TuiSyncStatus {
            runtime_allowed: false,
            ..sync_status()
        },
        false,
    ));

    assert!(local.contains("Local only"));
    assert!(!local.contains("Sync now"));
    assert!(disabled.contains("Sync disabled"));
}

#[test]
fn conflicts_use_attention_color() {
    let mut status = sync_status();
    status.conflicts = 2;
    let state = SyncDialogState::default();
    let lines = sync_dialog_lines_for_test(&sync_view(&state, status));

    assert_eq!(lines[0].spans[0].style.fg, Some(ORANGE));
    assert!(lines.iter().any(|line| {
        let text = line.to_string();
        text.starts_with("conflicts") && text.ends_with('2')
    }));
}

#[test]
fn manual_sync_is_available_when_automatic_sync_is_off() {
    let mut status = sync_status();
    status.enabled = false;
    let rendered = render_overlay_view(sync_overlay(status, false));

    assert!(rendered.contains("off"));
    assert!(rendered.contains("Sync now"));
}

#[test]
fn summary_fits_narrow_terminals() {
    let rendered = render_overlay_view_at(sync_overlay(sync_status(), false), 30, 16);

    assert!(rendered.contains("Sync"));
    assert!(rendered.contains("No changes"));
    assert!(rendered.contains("Sync now"));
    assert!(!rendered.contains("sync cursor"));
}

#[test]
fn expanded_details_scroll_on_short_terminals() {
    let state = SyncDialogState {
        details: true,
        ..SyncDialogState::default()
    };
    let view = sync_view(&state, sync_status());

    assert!(sync_dialog_scroll_cap(&view, (52, 12).into()) > 0);
    let collapsed = SyncDialogState::default();
    assert_eq!(
        sync_dialog_scroll_cap(&sync_view(&collapsed, sync_status()), (100, 30).into()),
        0
    );
    let rendered = render_overlay_view_at(OverlayView::Sync(Box::new(view)), 52, 12);
    assert!(rendered.contains("/"), "{rendered}");
}

#[test]
fn clicks_resolve_to_the_rendered_action_rows() {
    let state = SyncDialogState::default();
    let view = sync_view(&state, sync_status());
    let backend = TestBackend::new(80, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_non_help_overlay_content(
                frame,
                &OverlayView::Sync(Box::new(sync_view(&state, sync_status()))),
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let (row, line) = (0..30)
        .map(|row| (row, buffer_row(buffer, row)))
        .find(|(_, line)| line.contains("Add device"))
        .expect("add device row");
    let column = line[..line.find("Add device").unwrap()].chars().count() as u16;

    assert_eq!(
        sync_dialog_hit(&view, (80, 30).into(), column, row),
        SyncDialogHit::Action(1)
    );
    assert_eq!(
        sync_dialog_hit(&view, (80, 30).into(), 0, 0),
        SyncDialogHit::Outside
    );
}

fn sync_overlay(status: TuiSyncStatus, details: bool) -> OverlayView<'static> {
    let state = borrow_value(SyncDialogState {
        details,
        ..SyncDialogState::default()
    });
    OverlayView::Sync(Box::new(sync_view(state, status)))
}

fn sync_view(state: &SyncDialogState, status: TuiSyncStatus) -> SyncDialogView<'_> {
    SyncDialogView {
        state,
        status: borrow_value(status),
        syncing: false,
    }
}

fn sync_status() -> TuiSyncStatus {
    TuiSyncStatus {
        enabled: true,
        set_up: true,
        phase: LocalPhase::SetUp,
        interval_seconds: 60,
        daemon_wake: SyncStatusCheck::new(true, "127.0.0.1:3554"),
        sync_cursor: Some("42".to_string()),
        local_sequence: Some("45".to_string()),
        ..TuiSyncStatus::default()
    }
}
