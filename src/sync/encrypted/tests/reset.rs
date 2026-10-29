//! A device stuck on a bad record keeps its data and rebuilds sync from it.
use super::{
    Installation, converge, failure, line_with, pair, spawn_invite, start_server, status, success,
    titles,
};

/// Breaks the newest record in encrypted server storage so it no longer opens.
async fn damage_newest_record(server_data: &std::path::Path) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", server_data.display()))
        .await
        .unwrap();
    let damaged = sqlx::query(
        "UPDATE server_e2ee_tail
         SET record = substr(record, 1, length(record) - 1) || x'00'
         WHERE sequence = (SELECT MAX(sequence) FROM server_e2ee_tail)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(damaged.rows_affected(), 1);
    pool.close().await;
}

#[tokio::test]
async fn device_stuck_on_a_bad_record_rebuilds_sync_after_reset() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let pair = pair(root).await;
    let (a, b) = (&pair.a, &pair.b);
    a.ok(&["add", "From a"]).await;
    b.ok(&["add", "From b"]).await;
    converge(&[a, b]).await;

    a.ok(&["add", "Damaged on the server"]).await;
    a.ok(&["sync"]).await;
    damage_newest_record(&root.join("server.sqlite")).await;
    b.ok(&["add", "Unsynced on b"]).await;

    // Every sync stops at the same record, with an explanation, while local
    // tasks stay readable and editable.
    for _ in 0..2 {
        let error = failure(&b.run(&["sync"]).await);
        assert!(error.contains("error encrypted-tail-invalid"), "{error}");
    }
    let lines = crate::cli_error_lines(&anyhow::anyhow!("error encrypted-tail-invalid")).join("\n");
    assert!(lines.contains("to protect your data"), "{lines}");
    assert!(lines.contains("`aven sync reset`"), "{lines}");
    assert!(lines.contains("#rebuilding-sync"), "{lines}");
    b.ok(&["add", "Added while stuck"]).await;
    let kept = titles(b).await;
    assert_eq!(
        kept,
        ["Added while stuck", "From a", "From b", "Unsynced on b"]
    );

    // Reset needs confirmation, then leaves an ordinary local database.
    let error = failure(&b.run(&["sync", "reset"]).await);
    assert!(
        error.contains("sync-reset-confirmation-required"),
        "{error}"
    );
    let report: serde_json::Value =
        serde_json::from_str(&b.ok(&["sync", "reset", "--yes", "--json"]).await).unwrap();
    assert_eq!(report["state"], "reset");
    assert_eq!(status(b).await["state"], "not-set-up");
    assert_eq!(titles(b).await, kept);
    let error = failure(&b.run(&["sync"]).await);
    assert!(error.contains("sync-not-set-up"), "{error}");
    let error = failure(&b.run_with_input(&["sync", "join", "--yes"], "").await);
    assert!(
        error.contains("sync-join-requires-empty-database"),
        "{error}"
    );
    let report: serde_json::Value =
        serde_json::from_str(&b.ok(&["sync", "reset", "--yes", "--json"]).await).unwrap();
    assert_eq!(report["state"], "not-set-up");

    // Rebuild on fresh storage from b, and join a new empty database.
    let operator = Installation::new(root, "operator");
    let fresh = root.join("fresh-server.sqlite");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let setup = line_with(
        &operator
            .ok(&[
                "server",
                "setup",
                "--data",
                &fresh.display().to_string(),
                "--url",
                &format!("http://127.0.0.1:{port}"),
            ])
            .await,
        "aven-setup:",
    );
    let _server = start_server(&operator, &fresh, &format!("127.0.0.1:{port}")).await;
    success(
        &b.run_with_input(&["sync", "setup", "--yes"], &setup).await,
        &["sync", "setup"],
    );
    let c = Installation::new(root, "c");
    let (invite, invitation, _) = spawn_invite(b, None).await;
    success(
        &c.run_with_input(&["sync", "join", "--yes"], &invitation)
            .await,
        &["sync", "join"],
    );
    assert!(invite.wait_with_output().await.unwrap().status.success());
    assert_eq!(titles(&c).await, kept);
    c.ok(&["add", "From c"]).await;
    converge(&[b, &c]).await;
    assert_eq!(titles(b).await, titles(&c).await);
}

#[tokio::test]
async fn force_reset_abandons_an_unresumable_setup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let operator = Installation::new(root, "operator");
    let device = Installation::new(root, "device");
    device.ok(&["add", "Kept after abandoned setup"]).await;

    let server_data = root.join("lost-server.sqlite");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let disappearing = super::spawn_probe_only_server(listener);
    let setup = line_with(
        &operator
            .ok(&[
                "server",
                "setup",
                "--data",
                &server_data.display().to_string(),
                "--url",
                &format!("http://{address}"),
            ])
            .await,
        "aven-setup:",
    );
    let error = failure(
        &device
            .run_with_input(&["sync", "setup", "--yes"], &setup)
            .await,
    );
    disappearing.abort();
    assert!(error.contains("sync-setup-outcome-unknown"), "{error}");
    assert_eq!(status(&device).await["state"], "setup-incomplete");

    let error = failure(&device.run(&["sync", "reset", "--yes"]).await);
    assert!(error.contains("sync-reset-setup-in-progress"), "{error}");
    assert!(error.contains("sync reset --force"), "{error}");

    let error = failure(&device.run(&["sync", "reset", "--force"]).await);
    assert!(error.contains("may orphan a server"), "{error}");
    assert!(
        error.contains("sync-reset-confirmation-required"),
        "{error}"
    );

    let report: serde_json::Value = serde_json::from_str(
        &device
            .ok(&["sync", "reset", "--force", "--yes", "--json"])
            .await,
    )
    .unwrap();
    assert_eq!(report["state"], "reset");
    assert_eq!(status(&device).await["state"], "not-set-up");
    assert_eq!(titles(&device).await, ["Kept after abandoned setup"]);
}

/// Reset is refused while another device may still complete an invitation.
#[tokio::test]
async fn reset_waits_for_an_open_invitation() {
    let temp = tempfile::tempdir().unwrap();
    let (_server, a) = super::set_up(temp.path()).await;
    let (mut abandoned, _, _) = spawn_invite(&a, None).await;
    abandoned.kill().await.unwrap();
    abandoned.wait().await.unwrap();
    let error = failure(&a.run(&["sync", "reset", "--yes"]).await);
    assert!(error.contains("sync-reset-invitation-open"), "{error}");
    a.ok(&["sync", "invite", "--cancel"]).await;
    a.ok(&["sync", "reset", "--yes"]).await;
    assert_eq!(status(&a).await["state"], "not-set-up");
}

#[tokio::test]
async fn local_reset_guidance_is_readable_and_kept_out_of_json() {
    let temp = tempfile::tempdir().unwrap();
    let installation = Installation::new(temp.path(), "local");
    let output = installation.run(&["sync", "reset", "--yes"]).await;
    let stdout = success(&output, &["sync", "reset", "--yes"]);
    assert!(stdout.contains("It is local only."), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let guidance = "To start a new sync, run aven sync setup.\n\
                    Get a setup invitation from Aven Cloud or your hosting provider.\n\
                    If you self-host, create one with aven server setup.";
    assert!(stderr.contains(guidance), "{stderr}");
    assert!(!stderr.contains('`'), "{stderr}");
    assert!(!stderr.contains('\x1b'), "{stderr}");

    let status = installation.ok(&["sync", "status"]).await;
    assert!(status.contains(guidance), "{status}");
    assert!(status.contains("To join an existing sync, run aven sync join on a new database."));
    assert!(!status.contains('`'), "{status}");
    assert!(!status.contains('\x1b'), "{status}");

    let output = installation
        .run(&["sync", "reset", "--yes", "--json"])
        .await;
    let stdout = success(&output, &["sync", "reset", "--yes", "--json"]);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        report,
        serde_json::json!({"version": 1, "state": "not-set-up"})
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("To start a new sync"), "{stderr}");
}
