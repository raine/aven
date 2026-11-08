//! The daemon drives the encrypted engine after a local mutation wakes it.
use super::*;

fn write_sync_config(node: &Installation, enabled: bool, interval_seconds: u64) {
    let dir = node.root.join(node.name).join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.yaml"),
        format!("sync:\n  enabled: {enabled}\n  interval_seconds: {interval_seconds}\n"),
    )
    .unwrap();
}

fn enable_automatic_sync(node: &Installation) {
    write_sync_config(node, true, 3600);
}

type DaemonLog = std::sync::Arc<std::sync::Mutex<String>>;

/// Starts `aven daemon` and returns it with its standard output lines and
/// its collected standard error log.
async fn start_daemon(
    node: &Installation,
) -> (
    Child,
    tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    DaemonLog,
) {
    let mut child = node
        .command(&["daemon"])
        .env("AVEN_LOG", "aven=debug")
        .spawn()
        .unwrap();
    let lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let log = DaemonLog::default();
    let mut stderr = BufReader::new(child.stderr.take().unwrap()).lines();
    let sink = log.clone();
    tokio::spawn(async move {
        while let Ok(Some(line)) = stderr.next_line().await {
            let mut log = sink.lock().unwrap();
            log.push_str(&line);
            log.push('\n');
        }
    });
    (child, lines, log)
}

async fn wait_for_line(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    prefix: &str,
) -> String {
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.starts_with(prefix) {
                return line;
            }
        }
        panic!("daemon exited before {prefix}");
    })
    .await
    .unwrap_or_else(|_| panic!("no {prefix} line"))
}

#[tokio::test]
async fn daemon_waits_for_setup_then_syncs_encrypted_rounds_on_wake() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // An unconfigured database is reported once and never contacts a server.
    let local = Installation::new(root, "local");
    enable_automatic_sync(&local);
    local.ok(&["add", "Local only"]).await;
    let (mut daemon, mut lines, _) = start_daemon(&local).await;
    wait_for_line(&mut lines, "daemon db=").await;
    let line = wait_for_line(&mut lines, "daemon-sync-not-set-up").await;
    assert!(line.contains("aven sync setup"), "{line}");
    daemon.kill().await.unwrap();

    let pair = pair(root).await;
    enable_automatic_sync(&pair.a);
    let (_daemon, mut lines, log) = start_daemon(&pair.a).await;
    wait_for_line(&mut lines, "daemon db=").await;
    let line = wait_for_line(&mut lines, "daemon-synced").await;
    assert!(line.contains("metadata_caught_up=true"), "{line}");

    // A mutation wakes the daemon, which uploads it without `aven sync`.
    pair.a
        .ok(&["add", "Synced by the daemon", "--project", "app"])
        .await;
    let line = wait_for_line(&mut lines, "daemon-synced").await;
    assert!(line.contains("metadata_caught_up=true"), "{line}");
    assert_eq!(status(&pair.a).await["local_changes_pending"], false);
    pair.b.ok(&["sync"]).await;
    assert!(
        titles(&pair.b)
            .await
            .contains(&"Synced by the daemon".to_string())
    );

    // Plain daemon events are not duplicated by tracing output.
    let log = log.lock().unwrap().clone();
    assert!(!log.contains("daemon sync completed"), "{log}");
    assert!(!log.contains("Synced by the daemon"));
}

const MCP_TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// Leaves `sync.enabled` off, since `--sync` is what requests the worker.
fn configure_mcp_sync(node: &Installation, interval_seconds: u64) {
    write_sync_config(node, false, interval_seconds);
}

struct McpServer {
    child: Child,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    url: String,
    http: reqwest::Client,
}

impl McpServer {
    async fn start(node: &Installation) -> Self {
        let mut child = node
            .command(&["mcp", "--http", "--bind", "127.0.0.1:0", "--sync"])
            .env("AVEN_MCP_TOKEN", MCP_TOKEN)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let listening = wait_for_line(&mut lines, "listening url=").await;
        Self {
            child,
            lines,
            url: listening
                .strip_prefix("listening url=")
                .unwrap()
                .to_string(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    async fn call(&self, tool: &str, arguments: serde_json::Value) -> serde_json::Value {
        let response: serde_json::Value = self
            .http
            .post(&self.url)
            .bearer_auth(MCP_TOKEN)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": tool, "arguments": arguments},
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let text = response["result"]["content"][0]["text"].as_str();
        serde_json::from_str(text.unwrap_or_else(|| panic!("{tool}: {response}"))).unwrap()
    }
}

#[cfg(unix)]
#[tokio::test]
async fn mcp_write_reaches_the_sync_worker_directly_and_shutdown_exits_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let pair = pair(temp.path()).await;
    // The hour-long interval leaves the direct wake as the only trigger.
    configure_mcp_sync(&pair.a, 3600);
    let mut server = McpServer::start(&pair.a).await;
    wait_for_line(&mut server.lines, "daemon-synced").await;

    let body = server
        .call("create_task", serde_json::json!({"title": "Sent over MCP"}))
        .await;
    assert_eq!(body["task"]["title"], "Sent over MCP", "{body}");
    wait_for_line(&mut server.lines, "daemon-synced").await;
    pair.b.ok(&["sync"]).await;
    assert!(titles(&pair.b).await.contains(&"Sent over MCP".to_string()));

    let pid = server.child.id().unwrap() as libc::pid_t;
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(15), server.child.wait())
        .await
        .expect("the server exits after SIGTERM")
        .unwrap();
    assert!(status.success(), "{status}");
}

#[tokio::test]
async fn mcp_sync_worker_brings_in_remote_changes_without_a_local_write() {
    let temp = tempfile::tempdir().unwrap();
    let pair = pair(temp.path()).await;
    configure_mcp_sync(&pair.a, 1);
    let server = McpServer::start(&pair.a).await;

    pair.b.ok(&["add", "Written on the peer"]).await;
    pair.b.ok(&["sync"]).await;

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let body = server
            .call("search_tasks", serde_json::json!({"query": "peer"}))
            .await;
        if body["tasks"]
            .as_array()
            .is_some_and(|tasks| !tasks.is_empty())
        {
            break;
        }
        assert!(Instant::now() < deadline, "peer task never arrived: {body}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn foreground_sync_success_sends_retry_wake_for_recorded_daemon_failure() {
    let temp = tempfile::tempdir().unwrap();
    let pair = pair(temp.path()).await;
    enable_automatic_sync(&pair.a);
    let db = pair.a.db();
    let database = aven_core::db::Database::open(&db).await.unwrap();
    // Stands in for a host that syncs: it holds the lock, has recorded its
    // status and listens on the database's wake socket.
    let lock = std::fs::File::create(aven_host::host::lock_path(&db)).unwrap();
    lock.try_lock().unwrap();
    database
        .record_host_start(
            7,
            "daemon",
            "0.1.0",
            &[aven_host::host::CAPABILITY_SYNC],
            chrono::Utc::now().timestamp(),
            None,
        )
        .await
        .unwrap();
    let socket = tokio::net::UnixDatagram::bind(&aven_host::host::wake::paths(&db)[0]).unwrap();
    database
        .record_daemon_sync_round(
            100,
            Some(("protected-key-storage-unavailable", "authorize", 400)),
        )
        .await
        .unwrap();
    pair.a.ok(&["sync"]).await;
    let mut buf = [0; 16];
    let len = tokio::time::timeout(Duration::from_secs(1), socket.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..len], b"r");
    assert!(database.daemon_sync_state().await.unwrap().error.is_some());
}
