use super::*;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[test]
fn healthy_sync_status_renders_compact_summary() {
    let rendered = render_overlay_view(sync_status_overlay(sync_status(), false));

    assert!(rendered.contains(CONFIG_STATUS_TITLE));
    assert!(rendered.contains("Up to date"));
    assert!(rendered.contains("Last synced 12s ago"));
    assert!(rendered.contains("https://sync.example"));
    assert!(rendered.contains("pending"));
    assert!(rendered.contains("conflicts"));
    assert!(rendered.contains("d details"));
    assert!(rendered.contains("S sync"));
    assert!(!rendered.contains("database pin"));
    assert!(!rendered.contains("sync cursor"));
    assert!(!rendered.contains("CONNECTION"));
}

#[test]
fn details_reveal_internal_diagnostics() {
    let rendered = render_overlay_view(sync_status_overlay(sync_status(), true));

    assert!(rendered.contains("DETAILS"));
    assert!(rendered.contains("database pin"));
    assert!(rendered.contains("sync cursor"));
    assert!(rendered.contains("last pushed"));
    assert!(rendered.contains("d summary"));
}

#[test]
fn failures_are_visible_without_expanding_details() {
    let mut status = sync_status();
    status.last_error =
        Some("connection refused while contacting the configured server".to_string());
    let view = sync_status_view(status, false);
    let lines = sync_status_lines_for_test(&view);

    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("Sync needs attention"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("last error"))
    );
    assert!(lines.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.style.fg == Some(RED) && span.content.contains("connection"))
    }));
    assert!(!lines.iter().any(|line| line.to_string() == "DETAILS"));
}

#[test]
fn local_and_runtime_disabled_states_have_distinct_copy() {
    let local = sync_status_view(TuiSyncStatus::default(), false);
    let disabled = sync_status_view(
        TuiSyncStatus {
            enabled: true,
            runtime_allowed: false,
            configured_server: Some(SyncStatusCheck::new(true, "https://sync.example")),
            ..TuiSyncStatus::default()
        },
        false,
    );

    let local_lines = sync_status_lines_for_test(&local);
    let disabled_lines = sync_status_lines_for_test(&disabled);

    assert!(
        local_lines
            .iter()
            .any(|line| line.to_string().contains("Local only"))
    );
    assert!(
        disabled_lines
            .iter()
            .any(|line| line.to_string().contains("Sync disabled"))
    );
}

#[test]
fn conflicts_use_attention_color() {
    let mut status = sync_status();
    status.conflicts = 2;
    let lines = sync_status_lines_for_test(&sync_status_view(status, false));

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
    let rendered = render_overlay_view(sync_status_overlay(status, false));

    assert!(rendered.contains("Local only"));
    assert!(rendered.contains("S sync"));
}

#[test]
fn summary_fits_narrow_terminals() {
    let rendered = render_overlay_view_at(sync_status_overlay(sync_status(), false), 30, 14);

    assert!(rendered.contains("Sync status"));
    assert!(rendered.contains("Up to date"));
    assert!(!rendered.contains("database pin"));
}

#[test]
fn expanded_details_scroll_on_short_terminals() {
    let status = sync_status();

    assert!(sync_status_scroll_cap(&status, true, (52, 12).into()) > 0);
    assert_eq!(sync_status_scroll_cap(&status, false, (100, 30).into()), 0);
    let rendered = render_overlay_view_at(sync_status_overlay(status, true), 52, 12);
    assert!(rendered.contains("j/k scroll"));
}

fn sync_status_overlay(status: TuiSyncStatus, details: bool) -> OverlayView {
    OverlayView::SyncStatus(Box::new(sync_status_view(status, details)))
}

fn sync_status_view(status: TuiSyncStatus, details: bool) -> SyncStatusView {
    SyncStatusView {
        state: SyncStatusState { details, scroll: 0 },
        status: Box::new(status),
        syncing: false,
        now: OffsetDateTime::parse("2026-06-25T10:20:12Z", &Rfc3339).unwrap(),
    }
}

fn sync_status() -> TuiSyncStatus {
    TuiSyncStatus {
        enabled: true,
        configured_server: Some(SyncStatusCheck::new(true, "https://sync.example")),
        pinned_server: Some("https://sync.example".to_string()),
        server_match: Some(SyncStatusCheck::new(true, "yes")),
        daemon_server: Some(SyncStatusCheck::new(true, "https://sync.example")),
        auth_token_configured: true,
        interval_seconds: 60,
        daemon_wake: SyncStatusCheck::new(true, "127.0.0.1:3554"),
        pending_changes: 0,
        conflicts: 0,
        sync_cursor: Some("42".to_string()),
        local_sequence: Some("45".to_string()),
        last_attempt: Some("2026-06-25T10:20:00Z".to_string()),
        last_success: Some("2026-06-25T10:20:00Z".to_string()),
        last_pushed: Some("2".to_string()),
        last_pulled: Some("3".to_string()),
        last_cursor: Some("44".to_string()),
        ..TuiSyncStatus::default()
    }
}
