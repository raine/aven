//! Real CLI commands in separate worker processes: two installations with
//! independent databases, configuration and file-backed protected keys, and an
//! `aven server` process on loopback.
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::time::Instant;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use super::SetupInvitation;

const WORKER: &str = "sync::encrypted::tests::cli_worker";

#[test]
fn protected_key_storage_failures_explain_setup_errors() {
    use crate::protected_local_keys::{
        ProtectedLocalKeyStoreError, ProtectedLocalKeyStoreErrorKind,
    };
    use crate::sync::error_explanations::{self, ErrorAction, ErrorSurface};

    for (kind, code, message) in [
        (
            ProtectedLocalKeyStoreErrorKind::Unavailable,
            "protected-key-storage-unavailable",
            "Protected sync key storage is unavailable.",
        ),
        (
            ProtectedLocalKeyStoreErrorKind::Corrupt,
            "protected-key-storage-unsafe",
            "Protected sync key storage is corrupt or unsafe.",
        ),
    ] {
        let error = anyhow::Error::new(ProtectedLocalKeyStoreError::new(kind));
        let explanation =
            error_explanations::explain(ErrorAction::Setup, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, code);
        assert_eq!(explanation.message, message);
    }
}

/// Runs `aven` argument vectors from `AVEN_CLI_WORKER_ARGS` through the
/// ordinary parse and dispatch path, then exits with the command's status.
#[test]
#[ignore = "subprocess worker for encrypted CLI tests"]
fn cli_worker() {
    let Ok(args) = std::env::var("AVEN_CLI_WORKER_ARGS") else {
        return;
    };
    let args: Vec<String> = serde_json::from_str(&args).unwrap();
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(crate::run_cli_from(
            std::iter::once("aven".to_string()).chain(args),
        ));
    std::io::stdout().flush().unwrap();
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("Error: {error:#}");
            std::process::exit(1)
        }
    }
}

struct Installation {
    root: PathBuf,
    name: &'static str,
}

impl Installation {
    fn new(root: &Path, name: &'static str) -> Self {
        Self {
            root: root.to_path_buf(),
            name,
        }
    }

    fn db(&self) -> PathBuf {
        self.root.join(format!("{}.sqlite", self.name))
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut full = vec!["--db".to_string(), self.db().display().to_string()];
        full.extend(args.iter().map(|arg| arg.to_string()));
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", WORKER, "--nocapture", "--quiet"])
            .env(
                "AVEN_CLI_WORKER_ARGS",
                serde_json::to_string(&full).unwrap(),
            )
            .env("AVEN_CONFIG_DIR", self.root.join(self.name).join("config"))
            .env(
                "AVEN_TEST_PROTECTED_KEYS",
                self.root.join(self.name).join("keys"),
            )
            .env("XDG_STATE_HOME", self.root.join(self.name).join("state"))
            .env(
                "AVEN_LOG_FILE",
                self.root.join(format!("{}.log", self.name)),
            )
            .env("AVEN_NO_UPDATE_CHECK", "1")
            .env_remove("AVEN_DB")
            .env_remove("AVEN_DEV_DB")
            .env_remove("AVEN_SYNC_DISABLED")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    async fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().await.unwrap()
    }

    async fn run_with_input(&self, args: &[&str], input: &str) -> Output {
        let mut child = self.command(args).stdin(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).await.unwrap();
        drop(stdin);
        child.wait_with_output().await.unwrap()
    }

    async fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args).await;
        success(&output, args)
    }
}

/// Command output without the test harness banner printed before the worker.
fn stdout(output: &Output) -> String {
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_once("running 1 test\n")
        .map_or(text.as_ref(), |(_, rest)| rest)
        .to_string()
}

fn success(output: &Output, args: &[&str]) -> String {
    let stdout = stdout(output);
    assert!(
        output.status.success(),
        "{args:?} failed\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn failure(output: &Output) -> String {
    assert!(!output.status.success(), "command unexpectedly succeeded");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn created_ref(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("created "))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no created ref in {stdout}"))
        .to_string()
}

fn line_with(stdout: &str, prefix: &str) -> String {
    stdout
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} line in {stdout}"))
        .to_string()
}

fn png(path: &Path, marker: u8) -> Vec<u8> {
    let mut image = ::image::RgbaImage::new(11, 7);
    for (index, byte) in image.as_mut().iter_mut().enumerate() {
        *byte = marker.wrapping_add(index as u8).rotate_left(3);
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    std::fs::write(path, bytes.get_ref()).unwrap();
    bytes.into_inner()
}

async fn start_server(operator: &Installation, data: &Path, bind: &str) -> Child {
    let data = data.display().to_string();
    let mut child = operator
        .command(&["server", "--data", &data, "--bind", bind])
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.starts_with("listening url=") {
                return;
            }
        }
        panic!("encrypted server exited before listening");
    })
    .await
    .unwrap();
    child
}

/// Starts `sync invite` and returns its invitation line and remaining output.
async fn spawn_invite(
    node: &Installation,
    seconds: Option<&str>,
) -> (
    Child,
    String,
    tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) {
    let mut command = node.command(&["sync", "invite"]);
    if let Some(seconds) = seconds {
        command.env("AVEN_TEST_INVITATION_SECONDS", seconds);
    }
    let mut child = command.spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    loop {
        let line = lines.next_line().await.unwrap().unwrap();
        if line.starts_with(aven_core::sync::device_invitation::PREFIX) {
            return (child, line, lines);
        }
    }
}

async fn enrollment_identity(node: &Installation) -> (Vec<u8>, String) {
    let database = aven_core::db::Database::open(&node.db()).await.unwrap();
    let (identity, client, _) = database.enrollment_pin().await.unwrap().unwrap();
    (identity.to_vec(), client)
}

async fn titles(node: &Installation) -> Vec<String> {
    let json: serde_json::Value =
        serde_json::from_str(&node.ok(&["list", "--all", "--json"]).await).unwrap();
    let mut titles = json
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["title"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    titles.sort();
    titles
}

async fn attachment_bytes(node: &Installation, task: &str, out: &Path) -> Vec<Vec<u8>> {
    let json: serde_json::Value =
        serde_json::from_str(&node.ok(&["attachment", "list", task, "--json"]).await).unwrap();
    let mut images = Vec::new();
    for (index, attachment) in json.as_array().unwrap().iter().enumerate() {
        let id = attachment["attachment_id"].as_str().unwrap();
        let path = out.join(format!("{}-{task}-{index}.png", node.name));
        let path = path.display().to_string();
        node.ok(&["attachment", "get", id, "--output", &path]).await;
        images.push(std::fs::read(&path).unwrap());
    }
    images.sort();
    images
}

async fn status(node: &Installation) -> serde_json::Value {
    serde_json::from_str(&node.ok(&["sync", "status", "--json"]).await).unwrap()
}

/// Whether `aven sync` text output reports a completed task sync.
fn synced(stdout: &str) -> bool {
    stdout.contains("Changes: sent ") || stdout.contains("Tasks were already up to date")
}

async fn converge(nodes: &[&Installation]) {
    for _ in 0..2 {
        for node in nodes {
            let stdout = node.ok(&["sync"]).await;
            assert!(synced(&stdout), "{stdout}");
        }
    }
}

#[tokio::test]
async fn cli_sets_up_pairs_and_syncs_two_installations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let operator = Installation::new(root, "operator");
    let a = Installation::new(root, "a");
    let b = Installation::new(root, "b");
    let server_data = root.join("server.sqlite");
    let data = server_data.display().to_string();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let bind = format!("127.0.0.1:{port}");
    let url = format!("http://127.0.0.1:{port}");

    // Synthetic data on the future seed, created through ordinary commands.
    a.ok(&["label", "create", "home"]).await;
    let epic = created_ref(
        &a.ok(&["add", "Seed epic", "--project", "app", "--epic"])
            .await,
    );
    let child = created_ref(
        &a.ok(&[
            "add",
            "Seed child",
            "--project",
            "app",
            "--label",
            "home",
            "--metadata",
            "owner=a",
            "--priority",
            "high",
        ])
        .await,
    );
    let blocker = created_ref(&a.ok(&["add", "Seed blocker", "--project", "app"]).await);
    a.ok(&["epic", "add", &child, &epic]).await;
    a.ok(&["dep", "add", &child, &blocker]).await;
    a.ok(&["note", &child, "seed note"]).await;
    a.ok(&["add", "Seed daily", "--project", "app", "--repeat", "daily"])
        .await;
    let seed_png = root.join("seed.png");
    let seed_image = png(&seed_png, 1);
    a.ok(&["attachment", "add", &child, &seed_png.display().to_string()])
        .await;

    // An unconfigured database stays local: sync explains setup, status
    // reports it, and invitations need setup first.
    let error = failure(&a.run(&["sync"]).await);
    assert!(error.contains("sync-not-set-up"), "{error}");
    assert_eq!(status(&a).await["state"], "not-set-up");
    let text = a.ok(&["sync", "status"]).await;
    assert!(text.contains("Sync: not set up"), "{text}");
    let error = failure(&a.run(&["sync", "invite"]).await);
    assert!(error.contains("sync-not-set-up"), "{error}");

    // Storage must be prepared, loopback-bound, and never holds change history.
    let error = failure(&operator.run(&["server", "--data", &data]).await);
    assert!(error.contains("server-storage-unprepared"), "{error}");
    assert!(!server_data.exists());
    let with_history = Installation::new(root, "history");
    with_history.ok(&["add", "Existing server history"]).await;
    let history = with_history.db().display().to_string();
    let error = failure(&operator.run(&["server", "--data", &history]).await);
    assert!(error.contains("server-storage-unsupported"), "{error}");
    let error = failure(
        &operator
            .run(&["server", "setup", "--data", &history, "--url", &url])
            .await,
    );
    assert!(error.contains("server-storage-unsupported"), "{error}");
    let error = failure(
        &operator
            .run(&[
                "server",
                "setup",
                "--data",
                &data,
                "--url",
                "https://sync.example.com/aven",
            ])
            .await,
    );
    assert!(error.contains("sync-server-url-invalid"), "{error}");
    let setup_invitation = line_with(
        &operator
            .ok(&["server", "setup", "--data", &data, "--url", &url])
            .await,
        "aven-sync-setup-1:",
    );
    let error = failure(
        &operator
            .run(&["server", "--data", &data, "--bind", "0.0.0.0:0"])
            .await,
    );
    assert!(error.contains("server-bind-loopback"), "{error}");
    for retired in [
        &["server", "--encrypted", "--data", &data][..],
        &["server", "--unsafe-public-bind", "--data", &data][..],
        &["sync", "--server", &url][..],
        &["sync", "pair"][..],
    ] {
        let output = operator.run(retired).await;
        assert!(!output.status.success(), "{retired:?}");
    }

    // Setup requires explicit confirmation when standard input is not a terminal.
    let error = failure(
        &a.run_with_input(&["sync", "setup"], &setup_invitation)
            .await,
    );
    assert!(
        error.contains("sync-setup-confirmation-required"),
        "{error}"
    );
    assert!(
        error.contains("tasks: 4 (including scheduled and recurring)"),
        "{error}"
    );
    // Setup interrupted before the server answers resumes the same capture.
    let error = failure(
        &a.run_with_input(&["sync", "setup", "--yes"], &setup_invitation)
            .await,
    );
    assert!(error.contains("outcome-unknown"), "{error}");
    assert_eq!(status(&a).await["state"], "setup-incomplete");
    let error = failure(&a.run(&["sync"]).await);
    assert!(error.contains("sync-setup-incomplete"), "{error}");
    // Reissue keeps the setup ID the interrupted device bound, with a new secret.
    let reissued = line_with(
        &operator
            .ok(&["server", "setup", "--data", &data, "--url", &url])
            .await,
        "aven-sync-setup-1:",
    );
    let (old, new) = (
        SetupInvitation::decode(&setup_invitation).unwrap(),
        SetupInvitation::decode(&reissued).unwrap(),
    );
    assert_eq!(old.setup_id, new.setup_id);
    assert_ne!(old.secret.expose(), new.secret.expose());
    let mut server = start_server(&operator, &server_data, &bind).await;
    let error = failure(
        &a.run_with_input(&["sync", "setup"], &setup_invitation)
            .await,
    );
    assert!(
        error.contains("bootstrap-setup-invitation-rejected"),
        "{error}"
    );
    assert!(
        error.contains("sync-setup-fenced-invitation-rejected"),
        "{error}"
    );
    assert!(error.contains("newest invitation for that same server storage"));
    assert!(error.contains("back up this database and restore it to a new path"));
    assert!(!error.contains("nothing here was changed"));
    let setup_invitation = reissued;
    let output = a
        .run_with_input(&["sync", "setup"], &setup_invitation)
        .await;
    let stdout = success(&output, &["sync", "setup"]);
    assert!(
        stdout.contains(&format!("Sync set up with {url}")),
        "{stdout}"
    );
    assert!(stdout.contains("Images are up to date"), "{stdout}");
    assert!(stdout.contains("aven daemon install"), "{stdout}");

    // A setup invitation for storage claimed by another database is a
    // definite refusal and leaves local data local-only.
    let rejected = Installation::new(root, "rejected-setup");
    rejected.ok(&["add", "Keep local"]).await;
    let error = failure(
        &rejected
            .run_with_input(&["sync", "setup", "--yes"], &setup_invitation)
            .await,
    );
    assert!(
        error.contains("sync-setup-storage-already-claimed"),
        "{error}"
    );
    assert!(error.contains("nothing here was changed"), "{error}");
    assert_eq!(status(&rejected).await["state"], "not-set-up");
    let local = rejected.ok(&["list", "--all"]).await;
    assert!(local.contains("Keep local"), "{local}");

    // Joining refuses a database that already holds tasks.
    let occupied = Installation::new(root, "occupied");
    occupied.ok(&["add", "Unrelated local task"]).await;

    // An abandoned invite command resumes the same invitation; B joins while
    // the second command waits for it.
    // Open and expired unused invitations never stop sync.
    let (mut expiring, expired_invitation, _) = spawn_invite(&a, Some("5")).await;
    let declared = Instant::now();
    expiring.kill().await.unwrap();
    expiring.wait().await.unwrap();
    assert_eq!(status(&a).await["state"], "ready");
    let stdout = a.ok(&["sync"]).await;
    assert!(synced(&stdout), "{stdout}");
    tokio::time::sleep_until(declared + Duration::from_secs(6)).await;
    let stdout = a.ok(&["sync"]).await;
    assert!(synced(&stdout), "{stdout}");
    assert_eq!(status(&a).await["state"], "ready");

    let (mut abandoned, first_invitation, _) = spawn_invite(&a, None).await;
    assert_ne!(first_invitation, expired_invitation);
    assert_eq!(status(&a).await["state"], "ready");
    let stdout = a.ok(&["sync"]).await;
    assert!(synced(&stdout), "{stdout}");
    abandoned.kill().await.unwrap();
    abandoned.wait().await.unwrap();
    // A join interrupted while waiting for admission resumes its stored request.
    let mut interrupted = b
        .command(&["sync", "join", "--yes"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = interrupted.stdin.take().unwrap();
    stdin.write_all(first_invitation.as_bytes()).await.unwrap();
    drop(stdin);
    let mut stderr = BufReader::new(interrupted.stderr.take().unwrap()).lines();
    while let Some(line) = stderr.next_line().await.unwrap() {
        if line.contains("Waiting for the inviting device") {
            break;
        }
    }
    interrupted.kill().await.unwrap();
    interrupted.wait().await.unwrap();
    assert_eq!(status(&b).await["state"], "join-incomplete");
    let requested_identity = enrollment_identity(&b).await;
    let (invite, device_invitation, mut invite_stdout) = spawn_invite(&a, None).await;
    assert_eq!(device_invitation, first_invitation);
    let error = failure(
        &occupied
            .run_with_input(&["sync", "join"], &device_invitation)
            .await,
    );
    assert!(
        error.contains("sync-join-requires-empty-database"),
        "{error}"
    );
    let joined = b.run(&["sync", "join"]).await;
    let stdout = success(&joined, &["sync", "join"]);
    assert_eq!(enrollment_identity(&b).await, requested_identity);
    assert!(
        stdout.contains(&format!("Joined sync with {url}")),
        "{stdout}"
    );
    assert!(stdout.contains("aven daemon install"), "{stdout}");
    let invited = invite.wait_with_output().await.unwrap();
    assert!(invited.status.success());
    let mut rest = String::new();
    while let Some(line) = invite_stdout.next_line().await.unwrap() {
        rest.push_str(&line);
    }
    assert!(!rest.contains("Device added"), "{rest}");
    let invite_stderr = String::from_utf8_lossy(&invited.stderr);
    assert!(invite_stderr.contains("Device added"), "{invite_stderr}");
    assert_eq!(titles(&a).await, titles(&b).await);
    assert_eq!(
        attachment_bytes(&b, &child, root).await,
        vec![seed_image.clone()]
    );

    // Bidirectional edits, including a new image from B.
    b.ok(&["edit", &child, "--title", "Child renamed on B"])
        .await;
    let b_png = root.join("b.png");
    let b_image = png(&b_png, 2);
    b.ok(&["attachment", "add", &blocker, &b_png.display().to_string()])
        .await;
    a.ok(&["note", &blocker, "note from A"]).await;
    a.ok(&["add", "Task from A", "--project", "app"]).await;
    let sent = a.ok(&["sync"]).await;
    assert!(sent.contains("Changes: sent "), "{sent}");
    converge(&[&b, &a, &b]).await;
    assert_eq!(titles(&a).await, titles(&b).await);
    assert!(titles(&a).await.contains(&"Child renamed on B".to_string()));
    assert!(titles(&b).await.contains(&"Task from A".to_string()));
    assert_eq!(
        attachment_bytes(&a, &blocker, root).await,
        vec![b_image.clone()]
    );

    // Offline edits survive a server restart and then sync.
    server.kill().await.unwrap();
    server.wait().await.unwrap();
    a.ok(&["add", "Offline task from A", "--project", "app"])
        .await;
    let error = failure(&a.run(&["sync"]).await);
    assert!(error.contains("outcome-unknown"), "{error}");
    assert_eq!(status(&a).await["local_changes_pending"], true);
    // Startup replays stored membership and refuses altered history before binding.
    let tampered = root.join("tampered.sqlite");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", server_data.display()))
        .await
        .unwrap();
    sqlx::query("VACUUM INTO ?")
        .bind(tampered.display().to_string())
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", tampered.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE server_membership_transitions SET record=zeroblob(length(record))")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let error = failure(
        &operator
            .run(&[
                "server",
                "--data",
                &tampered.display().to_string(),
                "--bind",
                "127.0.0.1:0",
            ])
            .await,
    );
    assert!(error.contains("server-membership-invalid"), "{error}");
    let _server = start_server(&operator, &server_data, &bind).await;
    converge(&[&a, &b]).await;
    assert!(
        titles(&b)
            .await
            .contains(&"Offline task from A".to_string())
    );

    for node in [&a, &b] {
        let report = status(node).await;
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["server"], url.as_str());
        assert_eq!(report["local_changes_pending"], false);
        assert_eq!(report["image_uploads_pending"], false);
        assert_eq!(report["image_downloads_pending"], false);
        assert_eq!(report["images_unavailable"], false);
    }
    let text = a.ok(&["sync", "status"]).await;
    assert!(text.contains("Sync: end-to-end encrypted"), "{text}");
    let error = failure(
        &a.run_with_input(&["sync", "setup", "--yes"], &setup_invitation)
            .await,
    );
    assert!(error.contains("sync-already-set-up"), "{error}");

    // Secrets stay out of databases, configuration and logs.
    let secret = setup_invitation.trim_start_matches("aven-sync-setup-1:");
    let device_secret =
        device_invitation.trim_start_matches(aven_core::sync::device_invitation::PREFIX);
    for path in [
        a.db(),
        b.db(),
        server_data.clone(),
        root.join("a.log"),
        root.join("b.log"),
    ] {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(secret) && !text.contains(device_secret),
            "{}",
            path.display()
        );
    }
}

mod automatic;
mod conflicts;
mod devices;

/// A running server with seed `a` and joined peer `b`.
struct Pair {
    _server: Child,
    a: Installation,
    b: Installation,
}

/// Sets up `a` from its current data and joins an empty `b`.
async fn pair(root: &Path) -> Pair {
    let (server, a) = set_up(root).await;
    let b = Installation::new(root, "b");
    let (invite, invitation, _invite_stdout) = spawn_invite(&a, None).await;
    success(
        &b.run_with_input(&["sync", "join", "--yes"], &invitation)
            .await,
        &["sync", "join", "--yes"],
    );
    assert!(invite.wait_with_output().await.unwrap().status.success());
    Pair {
        _server: server,
        a,
        b,
    }
}

/// Starts a server and sets up `a` from its current data.
async fn set_up(root: &Path) -> (Child, Installation) {
    let operator = Installation::new(root, "operator");
    let a = Installation::new(root, "a");
    let data = root.join("server.sqlite");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let setup = line_with(
        &operator
            .ok(&[
                "server",
                "setup",
                "--data",
                &data.display().to_string(),
                "--url",
                &url,
            ])
            .await,
        "aven-sync-setup-1:",
    );
    let server = start_server(&operator, &data, &format!("127.0.0.1:{port}")).await;
    success(
        &a.run_with_input(&["sync", "setup", "--yes"], &setup).await,
        &["sync", "setup"],
    );
    (server, a)
}

/// A waiting `sync invite` stops as soon as another command cancels its
/// invitation, with a distinct failure scripts can detect.
#[cfg(unix)]
#[tokio::test]
async fn cli_waiting_invite_stops_when_another_command_cancels_it() {
    let root = tempfile::tempdir().unwrap();
    let (_server, a) = set_up(root.path()).await;

    let (waiting, _, _) = spawn_invite(&a, None).await;
    let cancelled = a.ok(&["sync", "invite", "--cancel"]).await;
    assert!(cancelled.contains("Invitation cancelled"), "{cancelled}");
    let output = tokio::time::timeout(Duration::from_secs(10), waiting.wait_with_output())
        .await
        .expect("waiting invite kept running after cancellation")
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sync-invitation-cancelled"), "{stderr}");
    assert_eq!(status(&a).await["invitation"], "none");
}

/// The reported flow with a short declared invitation lifetime: the inviting
/// device stops before admitting, the join times out and the invitation
/// expires, and the same database finishes with a new invitation.
#[cfg(unix)]
#[tokio::test]
async fn cli_invitation_cancel_and_resume_use_the_declared_expiry() {
    let root = tempfile::tempdir().unwrap();
    let (_server, a) = set_up(root.path()).await;

    let (interrupted, _, _) = spawn_invite(&a, Some("8")).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    unsafe {
        libc::kill(interrupted.id().unwrap() as i32, libc::SIGINT);
    }
    let output = interrupted.wait_with_output().await.unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Invitation cancelled"), "{stderr}");
    assert_eq!(status(&a).await["invitation"], "none");

    let (mut abandoned, _, _) = spawn_invite(&a, Some("8")).await;
    abandoned.kill().await.unwrap();
    abandoned.wait().await.unwrap();
    assert_eq!(status(&a).await["invitation"], "open");
    let cancelled = a.ok(&["sync", "invite", "--cancel"]).await;
    assert!(cancelled.contains("Invitation cancelled"), "{cancelled}");
    assert_eq!(status(&a).await["invitation"], "none");

    let (mut expiring, invitation, _) = spawn_invite(&a, Some("4")).await;
    expiring.kill().await.unwrap();
    expiring.wait().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let started = Instant::now();
    let (resumed, same_invitation, _) = spawn_invite(&a, Some("4")).await;
    assert_eq!(same_invitation, invitation);
    let output = resumed.wait_with_output().await.unwrap();
    assert!(!output.status.success());
    assert!(started.elapsed() < Duration::from_secs(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Resuming the open invitation"), "{stderr}");
    assert!(stderr.contains("sync-invitation-unused"), "{stderr}");
    assert_eq!(status(&a).await["invitation"], "none");
}

#[tokio::test]
async fn cli_join_continues_with_a_new_invitation_after_expiry() {
    let root = tempfile::tempdir().unwrap();
    let (_server, a) = set_up(root.path()).await;
    let b = Installation::new(root.path(), "b");
    let (mut invite, expired, _stdout) = spawn_invite(&a, Some("15")).await;
    invite.kill().await.unwrap();
    let mut join = b
        .command(&["sync", "join", "--yes"])
        .env("AVEN_TEST_INVITATION_SECONDS", "15")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = join.stdin.take().unwrap();
    stdin.write_all(expired.as_bytes()).await.unwrap();
    drop(stdin);
    let timed_out = failure(&join.wait_with_output().await.unwrap());
    assert!(timed_out.contains("error sync-join-timeout"), "{timed_out}");
    assert!(timed_out.contains("--new-invitation"), "{timed_out}");

    // The inviting device retires the unused expired invitation.
    a.ok(&["sync"]).await;
    let (invite, fresh, _stdout) = spawn_invite(&a, None).await;
    let args = ["sync", "join", "--new-invitation", "--yes"];
    success(&b.run_with_input(&args, &fresh).await, &args);
    assert!(invite.wait_with_output().await.unwrap().status.success());
    assert!(b.ok(&["sync", "status"]).await.contains("State: ready"));
}

/// Relays every request to a real server and, depending on `mode`, replaces
/// the reply to a committed claim or a status request with a refusal.
struct ForgingRelay {
    upstream: String,
    http: reqwest::Client,
    mode: std::sync::atomic::AtomicU8,
}

const RELAY: u8 = 0;
const FORGE_CLAIM_REJECTED: u8 = 1;
const FORGE_STATUS_CLAIMED: u8 = 2;

async fn forging_relay(
    axum::extract::State(relay): axum::extract::State<std::sync::Arc<ForgingRelay>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::http::header;
    use axum::response::IntoResponse;

    let path = request.uri().path().to_string();
    let authorization = request.headers().get(header::AUTHORIZATION).cloned();
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    let claim = text.contains("\"ClaimSetup\"") || text.contains("\"ClaimBearer\"");
    let status = text.contains("\"Status\"");
    let mut forwarded = relay
        .http
        .post(format!("{}{path}", relay.upstream))
        .header(header::CONTENT_TYPE, "application/json")
        .body(bytes);
    if let Some(authorization) = authorization {
        forwarded = forwarded.header(header::AUTHORIZATION, authorization);
    }
    let upstream = forwarded.send().await.unwrap();
    let forged = match relay.mode.load(std::sync::atomic::Ordering::SeqCst) {
        FORGE_CLAIM_REJECTED if claim => {
            assert!(
                upstream.status().is_success(),
                "the server commits the claim"
            );
            Some("bootstrap-setup-invitation-rejected")
        }
        FORGE_STATUS_CLAIMED if status => Some("bootstrap-storage-already-claimed"),
        _ => None,
    };
    if let Some(code) = forged {
        return (
            axum::http::StatusCode::CONFLICT,
            [(header::CONTENT_TYPE, "application/json")],
            format!("{{\"error\":\"{code}\"}}"),
        )
            .into_response();
    }
    let code = upstream.status();
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let body = upstream.bytes().await.unwrap();
    let mut response = (code, body).into_response();
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    response
}

/// Unsigned refusals may hide a committed claim, so they never discard the
/// seed authority or fence the setup; a later exact retry completes it.
#[tokio::test]
async fn cli_forged_setup_refusals_keep_committed_claim_recoverable() {
    use std::sync::{Arc, atomic::Ordering};

    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let operator = Installation::new(root, "operator");
    let a = Installation::new(root, "a");
    a.ok(&["add", "Keep local"]).await;
    let data = root.join("server.sqlite");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url = format!("http://{}", listener.local_addr().unwrap());
    let relay = Arc::new(ForgingRelay {
        upstream: format!("http://127.0.0.1:{port}"),
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        mode: FORGE_CLAIM_REJECTED.into(),
    });
    let app = axum::Router::new()
        .fallback(forging_relay)
        .with_state(relay.clone());
    let relay_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let setup = line_with(
        &operator
            .ok(&[
                "server",
                "setup",
                "--data",
                &data.display().to_string(),
                "--url",
                &relay_url,
            ])
            .await,
        "aven-sync-setup-1:",
    );
    let _server = start_server(&operator, &data, &format!("127.0.0.1:{port}")).await;

    // Both the setup claim and the bearer retry commit, but come back refused.
    let error = failure(&a.run_with_input(&["sync", "setup", "--yes"], &setup).await);
    assert!(error.contains("sync-setup-invitation-rejected"), "{error}");
    let server = aven_core::db::Database::open(&data).await.unwrap();
    assert!(server.e2ee_server_is_claimed().await.unwrap());
    assert_eq!(status(&a).await["state"], "not-set-up");
    let local = aven_core::db::Database::open(&a.db()).await.unwrap();
    assert!(
        local
            .local_seed_genesis_commitment()
            .await
            .unwrap()
            .is_some()
    );
    assert!(local.seed_source_pin().await.unwrap().is_none());

    // The retry claims exactly; a forged status refusal then leaves the fenced
    // setup resumable instead of requiring recovery to a new path.
    relay.mode.store(FORGE_STATUS_CLAIMED, Ordering::SeqCst);
    let error = failure(&a.run_with_input(&["sync", "setup", "--yes"], &setup).await);
    assert!(
        error.contains("sync-setup-fenced-storage-claimed"),
        "{error}"
    );
    assert!(!error.contains("sync-setup-recovery-required"), "{error}");
    assert_eq!(status(&a).await["state"], "setup-incomplete");

    relay.mode.store(RELAY, Ordering::SeqCst);
    let stdout = success(
        &a.run_with_input(&["sync", "setup"], &setup).await,
        &["sync", "setup"],
    );
    assert!(
        stdout.contains(&format!("Sync set up with {relay_url}")),
        "{stdout}"
    );
    assert_eq!(status(&a).await["state"], "ready");

    relay_task.abort();
}
