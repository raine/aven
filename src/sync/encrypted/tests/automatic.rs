//! The daemon drives the encrypted engine after a local mutation wakes it.
use super::*;

fn enable_automatic_sync(node: &Installation, wake: &str) {
    let dir = node.root.join(node.name).join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.yaml"),
        format!(
            "sync:\n  enabled: true\n  interval_seconds: 3600\ndaemon:\n  wake_addr: \"{wake}\"\n"
        ),
    )
    .unwrap();
}

fn free_wake_addr() -> String {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string()
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
    enable_automatic_sync(&local, &free_wake_addr());
    local.ok(&["add", "Local only"]).await;
    let (mut daemon, mut lines, _) = start_daemon(&local).await;
    wait_for_line(&mut lines, "daemon db=").await;
    let line = wait_for_line(&mut lines, "daemon-sync-not-set-up").await;
    assert!(line.contains("aven sync setup"), "{line}");
    daemon.kill().await.unwrap();

    let pair = pair(root).await;
    enable_automatic_sync(&pair.a, &free_wake_addr());
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

    // Daemon logs report rounds without task content.
    let log = log.lock().unwrap().clone();
    assert!(log.contains("daemon sync completed"), "{log}");
    assert!(!log.contains("Synced by the daemon"));
}
