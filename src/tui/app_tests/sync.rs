use super::*;

#[tokio::test]
async fn sync_now_requires_configured_server() {
    let mut app = test_app().await;

    app.execute(Action::SyncNow).await.unwrap();

    let message = toast_message(&app).unwrap();
    assert!(message.starts_with("sync unavailable:"));
    assert!(message.contains("sync-server-required"));
    assert!(!app.sync.work_pending());
}

#[tokio::test]
async fn sync_now_completes_without_daemon() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = axum::Router::new().route(
        "/sync",
        axum::routing::post(|| async {
            axum::Json(serde_json::json!({
                "protocol_version": aven_core::sync::wire::SYNC_PROTOCOL_VERSION,
                "cursor": 0,
                "has_more": false,
                "push_acks": [],
                "changes": [],
            }))
        }),
    );
    let server_task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let mut app = test_app().await;
    let mut config = crate::config::AppConfig::default();
    config.sync.enabled = true;
    config.sync.server_url = Some(format!("http://{address}"));
    app.set_config(config);
    app.show_config_status().unwrap();

    app.handle_overlay_key(key(KeyCode::Char('S')))
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::SyncStatus(_))));
    assert!(app.sync.work_pending());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            app.poll_sync().await.unwrap();
            if !app.sync.work_pending() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("sync task settles");
    server_task.abort();

    assert!(matches!(app.overlay, Some(OverlayState::SyncStatus(_))));
    let view = app.view();
    assert!(matches!(
        view.overlay,
        Some(OverlayView::SyncStatus(status)) if !status.syncing
    ));
    assert!(toast_message(&app).unwrap().starts_with("sync complete"));
}

#[tokio::test]
async fn sync_now_reports_unavailable_server() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut app = test_app().await;
    let mut config = crate::config::AppConfig::default();
    config.sync.server_url = Some(format!("http://127.0.0.1:{port}"));
    app.set_config(config);

    app.execute(Action::SyncNow).await.unwrap();
    assert!(app.sync.work_pending());
    assert!(matches!(
        app.notification,
        Some(Notification::Loading { .. })
    ));

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            app.poll_sync().await.unwrap();
            if !app.sync.work_pending() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("sync task settles");

    assert!(toast_message(&app).unwrap().starts_with("sync failed:"));
}
