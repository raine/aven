use super::*;

#[test]
fn database_stats_overlay_renders_like_sync_status() {
    let rendered = render_overlay_view(OverlayView::DatabaseStats {
        stats: borrow_value(database_stats()),
        scroll: 0,
    });

    assert!(rendered.contains(DATABASE_STATS_TITLE));
    assert!(rendered.contains("WORKSPACE"));
    assert!(rendered.contains("TASKS"));
    assert!(rendered.contains("SYNC HISTORY"));
    assert!(rendered.contains("change rows"));
    assert!(rendered.contains("min server_seq"));
    assert!(rendered.contains("payload bytes"));
    assert!(rendered.contains("1234"));
    assert!(rendered.contains("Enter/Esc close"));
}

#[test]
fn database_stats_overlay_scroll_changes_visible_content() {
    let backend = TestBackend::new(100, 18);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_non_help_overlay_content(
                frame,
                &OverlayView::DatabaseStats {
                    stats: borrow_value(database_stats()),
                    scroll: 14,
                },
            )
        })
        .unwrap();
    let rendered = buffer_text(terminal.backend());

    assert!(rendered.contains("LATEST TASK TIMESTAMPS"));
    assert!(rendered.contains("Enter/Esc close"));
    assert!(!rendered.contains("WORKSPACE"));
}

fn database_stats() -> TuiDatabaseStats {
    TuiDatabaseStats {
        workspace_name: "Default".to_string(),
        workspace_key: "default".to_string(),
        total_tasks: 3,
        open_tasks: 1,
        statuses: DatabaseStatsStatusCounts {
            inbox: 1,
            done: 2,
            ..DatabaseStatsStatusCounts::default()
        },
        priorities: DatabaseStatsPriorityCounts {
            urgent: 1,
            ..DatabaseStatsPriorityCounts::default()
        },
        projects: 1,
        labels: 2,
        notes: 3,
        task_labels: 2,
        sync_history: SyncHistoryStats {
            total_change_rows: 9,
            pending_change_rows: 4,
            synced_change_rows: 5,
            min_server_seq: Some(11),
            max_server_seq: Some(15),
            payload_bytes: 1234,
        },
        sqlite_page_size: 4096,
        sqlite_page_count: 1024,
        ..TuiDatabaseStats::default()
    }
}
