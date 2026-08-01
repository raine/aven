use super::*;

#[test]
fn sync_status_overlay_renders_key_sections_without_scrollbar() {
    let rendered = render_overlay_view(OverlayView::SyncStatus(Box::new(sync_status())));

    assert!(rendered.contains(CONFIG_STATUS_TITLE));
    assert!(rendered.contains("CONNECTION"));
    assert!(rendered.contains("STATE"));
    assert!(rendered.contains("LAST SYNC"));
    assert!(rendered.contains("server reach"));
    assert!(rendered.contains("last sync reached server"));
    assert!(rendered.contains("last synced"));
    assert!(!rendered.contains("2026-06-25T10:20:00Z"));
    assert!(rendered.contains("Enter/Esc close"));
    assert!(!rendered.contains('▲'));
    assert!(!rendered.contains('▼'));
}

#[test]
fn sync_status_lines_style_sections_successes_and_errors() {
    let mut status = sync_status();
    status.last_error = Some("connection refused".to_string());
    let lines = sync_status_lines_for_test(&status);

    let section = lines
        .iter()
        .find(|line| line.to_string() == "CONNECTION")
        .unwrap();
    assert_eq!(section.spans[0].style.fg, Some(ACCENT));

    assert_eq!(row_value_fg(&lines, "last synced"), Some(GREEN));
    assert_eq!(row_value_fg(&lines, "last error"), Some(RED));
    assert_eq!(row_value_fg(&lines, "configured server"), Some(GREEN));
    assert_eq!(row_value_fg(&lines, "daemon server"), Some(RED));
}

fn sync_status() -> TuiSyncStatus {
    TuiSyncStatus {
        enabled: true,
        configured_server: Some(SyncStatusCheck::new(true, "https://sync.example")),
        pinned_server: Some("https://sync.example".to_string()),
        server_match: Some(SyncStatusCheck::new(true, "yes")),
        daemon_server: Some(SyncStatusCheck::new(false, "not configured")),
        auth_token_configured: true,
        interval_seconds: 60,
        daemon_wake: SyncStatusCheck::new(true, "127.0.0.1:3554"),
        pending_changes: 2,
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

fn row_value_fg(lines: &[Line<'static>], label: &str) -> Option<ratatui::style::Color> {
    lines
        .iter()
        .find(|line| line.to_string().starts_with(label))
        .and_then(|line| line.spans.get(1))
        .and_then(|span| span.style.fg)
}
