//! An unfinished setup resumes from what this database saved, without the
//! setup invitation, across separate processes.
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::Ordering};

use aven_core::db::Database;

use super::{
    FAIL_UPLOADS, FORGE_CLAIM_REJECTED, FORGE_STATUS_CLAIMED, ForgingRelay, HOLD_CLAIMS,
    Installation, LOSE_CLAIM_REPLY, LOSE_PUBLISH_REPLY, RELAY, SetupInvitation, failure, line_with,
    start_server, status, success, titles,
};

const SERVER_HOLDS_NO_CLAIM: &str = "doesn't hold this setup's claim yet";

/// Server storage with a setup invitation, reached through a relay.
struct Hosted {
    operator: Installation,
    data: PathBuf,
    bind: String,
    relay: Arc<ForgingRelay>,
    url: String,
    setup: String,
    _relay_task: tokio::task::JoinHandle<()>,
}

impl Hosted {
    async fn new(root: &Path, mode: u8) -> Self {
        let operator = Installation::new(root, "operator");
        let data = root.join("server.sqlite");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let relay = Arc::new(ForgingRelay {
            upstream: format!("http://127.0.0.1:{port}"),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            mode: mode.into(),
            claims: 0.into(),
        });
        let app = axum::Router::new()
            .fallback(super::forging_relay)
            .with_state(relay.clone());
        let relay_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut hosted = Self {
            operator,
            data,
            bind: format!("127.0.0.1:{port}"),
            relay,
            url,
            setup: String::new(),
            _relay_task: relay_task,
        };
        hosted.setup = hosted.issue().await;
        hosted
    }

    /// Issues the current setup invitation; unclaimed storage keeps its ID.
    async fn issue(&self) -> String {
        let data = self.data.display().to_string();
        line_with(
            &self
                .operator
                .ok(&["server", "setup", "--data", &data, "--url", &self.url])
                .await,
            "aven-setup:",
        )
    }

    async fn start(&self) -> tokio::process::Child {
        start_server(&self.operator, &self.data, &self.bind).await
    }

    fn mode(&self, mode: u8) {
        self.relay.mode.store(mode, Ordering::SeqCst);
    }

    /// Lets every issued setup invitation expire.
    async fn expire_setup(&self) {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", self.data.display()))
            .await
            .unwrap();
        let expired = sqlx::query(
            "UPDATE meta SET value = rtrim(value, '0123456789') || '1'
             WHERE key = 'e2ee_server_setup'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(expired.rows_affected(), 1);
        pool.close().await;
    }

    async fn claimed(&self) -> bool {
        Database::open(&self.data)
            .await
            .unwrap()
            .e2ee_server_is_claimed()
            .await
            .unwrap()
    }

    /// A valid invitation for another server, which resuming must not read.
    fn unrelated_invitation(&self) -> String {
        let mut unrelated = SetupInvitation::decode(&self.setup).unwrap();
        unrelated.server = "http://127.0.0.1:9".into();
        unrelated.setup_id = [42; 32];
        unrelated.encode().unwrap().to_string()
    }
}

async fn seed_pin(node: &Installation) -> Option<[u8; 32]> {
    Database::open(&node.db())
        .await
        .unwrap()
        .local_seed_genesis_commitment()
        .await
        .unwrap()
}

async fn claim_refused(node: &Installation) -> bool {
    Database::open(&node.db())
        .await
        .unwrap()
        .local_seed_claim_refused()
        .await
        .unwrap()
}

fn protected_file(node: &Installation, suffix: &str) -> PathBuf {
    let keys = node.root.join(node.name).join("keys");
    std::fs::read_dir(&keys)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(&format!(".{suffix}")))
        .unwrap_or_else(|| panic!("no protected {suffix} in {}", keys.display()))
}

/// Joins a new database to `inviter` and returns its tasks.
async fn joined_titles(root: &Path, inviter: &Installation) -> Vec<String> {
    let joiner = Installation::new(root, "joiner");
    let (invite, invitation, _) = super::spawn_invite(inviter, None).await;
    success(
        &joiner
            .run_with_input(&["sync", "join", "--yes"], &invitation)
            .await,
        &["sync", "join"],
    );
    assert!(invite.wait_with_output().await.unwrap().status.success());
    titles(&joiner).await
}

/// The server admits the claim but its reply is lost; after the invitation
/// expires, a restarted setup finishes from saved state without reading input.
#[tokio::test]
async fn setup_resumes_a_committed_claim_without_its_expired_invitation() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let hosted = Hosted::new(root, LOSE_CLAIM_REPLY).await;
    let _server = hosted.start().await;
    let a = Installation::new(root, "a");
    a.ok(&["add", "Kept through resume"]).await;

    let error = failure(
        &a.run_with_input(&["sync", "setup", "--yes"], &hosted.setup)
            .await,
    );
    assert!(error.contains("sync-setup-outcome-unknown"), "{error}");
    assert!(hosted.claimed().await);
    assert_eq!(status(&a).await["state"], "setup-incomplete");
    let pin = seed_pin(&a).await;

    hosted.expire_setup().await;
    hosted.mode(RELAY);
    let output = a
        .run_with_input(&["sync", "setup"], &hosted.unrelated_invitation())
        .await;
    let stdout = success(&output, &["sync", "setup"]);
    assert!(
        stdout.contains(&format!("Sync set up with {}", hosted.url)),
        "{stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Resuming the unfinished setup"), "{stderr}");
    assert!(
        !stderr.contains("Set up sync from this database"),
        "{stderr}"
    );
    assert_eq!(status(&a).await["state"], "ready");
    assert_eq!(seed_pin(&a).await, pin);

    let error = failure(&a.run(&["sync", "setup"]).await);
    assert!(error.contains("sync-already-set-up"), "{error}");
    assert_eq!(joined_titles(root, &a).await, ["Kept through resume"]);
}

/// Upload and publication interruptions resume the sealed publication with
/// closed input, without claiming again.
#[tokio::test]
async fn setup_resumes_an_interrupted_publication_without_input() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let hosted = Hosted::new(root, FAIL_UPLOADS).await;
    let _server = hosted.start().await;
    let a = Installation::new(root, "a");
    a.ok(&["add", "Published after restart"]).await;

    failure(
        &a.run_with_input(&["sync", "setup", "--yes"], &hosted.setup)
            .await,
    );
    assert_eq!(status(&a).await["state"], "setup-incomplete");
    let database = Database::open(&a.db()).await.unwrap();
    let intent = database.seed_publication_intent_bytes().await.unwrap();
    assert!(intent.is_some());
    let claims = hosted.relay.claims.load(Ordering::SeqCst);

    hosted.expire_setup().await;
    hosted.mode(LOSE_PUBLISH_REPLY);
    failure(&a.run(&["sync", "setup"]).await);
    assert_eq!(status(&a).await["state"], "setup-incomplete");

    hosted.mode(RELAY);
    let stdout = success(&a.run(&["sync", "setup"]).await, &["sync", "setup"]);
    assert!(
        stdout.contains(&format!("Sync set up with {}", hosted.url)),
        "{stdout}"
    );
    assert_eq!(status(&a).await["state"], "ready");
    assert_eq!(hosted.relay.claims.load(Ordering::SeqCst), claims);
    assert_eq!(
        database
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .unwrap()
            .1,
        "adopted"
    );
    assert_eq!(joined_titles(root, &a).await, ["Published after restart"]);
}

/// A claim that never reached the server can't be made with the seed bearer.
/// Resuming leaves everything in place and asks for an invitation for the
/// same storage; storage that another sync claimed meanwhile is still refused
/// through the invitation, so the database isn't stuck resuming.
#[tokio::test]
async fn setup_resume_asks_for_an_invitation_when_the_server_holds_no_claim() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let hosted = Hosted::new(root, HOLD_CLAIMS).await;
    let _server = hosted.start().await;
    let a = Installation::new(root, "a");
    a.ok(&["add", "Local only"]).await;

    // Stop the process while its claim is in flight, before any reply.
    let mut interrupted = a
        .command(&["sync", "setup", "--yes"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = interrupted.stdin.take().unwrap();
        stdin.write_all(hosted.setup.as_bytes()).await.unwrap();
    }
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while hosted.relay.claims.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    interrupted.kill().await.unwrap();
    interrupted.wait().await.unwrap();
    hosted.mode(RELAY);
    assert!(!hosted.claimed().await);
    assert_eq!(status(&a).await["state"], "setup-incomplete");
    let pin = seed_pin(&a).await;
    assert!(pin.is_some());

    let error = failure(&a.run(&["sync", "setup"]).await);
    assert!(error.contains(SERVER_HOLDS_NO_CLAIM), "{error}");
    assert!(error.contains("sync-setup-invitation-invalid"), "{error}");
    assert_eq!(status(&a).await["state"], "setup-incomplete");
    assert!(!claim_refused(&a).await);
    assert_eq!(seed_pin(&a).await, pin);

    // Another sync claims the storage first. The invitation path reports it
    // and lets this database start over, as it did before resuming existed.
    let other = Installation::new(root, "other");
    success(
        &other
            .run_with_input(&["sync", "setup", "--yes"], &hosted.setup)
            .await,
        &["sync", "setup"],
    );
    let error = failure(&a.run_with_input(&["sync", "setup"], &hosted.setup).await);
    assert!(error.contains(SERVER_HOLDS_NO_CLAIM), "{error}");
    assert!(
        error.contains("sync-setup-storage-already-claimed"),
        "{error}"
    );
    assert_eq!(status(&a).await["state"], "not-set-up");
    assert_eq!(titles(&a).await, ["Local only"]);
}

/// After the original invitation expires, a reissued invitation for the same
/// storage continues the same setup through the prompt fallback.
#[tokio::test]
async fn setup_resume_continues_an_unadmitted_claim_with_a_reissued_invitation() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let hosted = Hosted::new(root, RELAY).await;
    let a = Installation::new(root, "a");
    a.ok(&["add", "Set up after reissue"]).await;

    // The server isn't running, so the claim outcome is unknown and fenced.
    let error = failure(
        &a.run_with_input(&["sync", "setup", "--yes"], &hosted.setup)
            .await,
    );
    assert!(error.contains("sync-setup-outcome-unknown"), "{error}");
    let pin = seed_pin(&a).await;
    let _server = hosted.start().await;
    hosted.expire_setup().await;

    let error = failure(&a.run_with_input(&["sync", "setup"], &hosted.setup).await);
    assert!(error.contains(SERVER_HOLDS_NO_CLAIM), "{error}");
    assert!(
        error.contains("sync-setup-fenced-invitation-rejected"),
        "{error}"
    );
    assert_eq!(status(&a).await["state"], "setup-incomplete");

    let reissued = hosted.issue().await;
    assert_ne!(reissued, hosted.setup);
    assert_eq!(
        SetupInvitation::decode(&reissued).unwrap().setup_id,
        SetupInvitation::decode(&hosted.setup).unwrap().setup_id
    );
    let stdout = success(
        &a.run_with_input(&["sync", "setup"], &reissued).await,
        &["sync", "setup"],
    );
    assert!(
        stdout.contains(&format!("Sync set up with {}", hosted.url)),
        "{stdout}"
    );
    assert_eq!(status(&a).await["state"], "ready");
    assert_eq!(seed_pin(&a).await, pin);
}

/// Missing saved setup state is refused without replacing keys or resetting.
#[tokio::test]
async fn setup_resume_refuses_missing_saved_state() {
    for (lost, expected) in [
        (
            &["seed-origin", "seed-origin-authority"][..],
            "sync-setup-server-missing",
        ),
        (&["seed"][..], "protected local key authority is missing"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let hosted = Hosted::new(root, RELAY).await;
        let a = Installation::new(root, "a");
        a.ok(&["add", "Kept"]).await;
        failure(
            &a.run_with_input(&["sync", "setup", "--yes"], &hosted.setup)
                .await,
        );
        let pin = seed_pin(&a).await;
        hosted.relay.claims.store(0, Ordering::SeqCst);
        for suffix in lost {
            std::fs::remove_file(protected_file(&a, suffix)).unwrap();
        }
        let _server = hosted.start().await;

        let error = failure(&a.run_with_input(&["sync", "setup"], &hosted.setup).await);
        assert!(error.contains(expected), "{error}");
        assert_eq!(hosted.relay.claims.load(Ordering::SeqCst), 0);
        assert!(!hosted.claimed().await);
        assert_eq!(status(&a).await["state"], "setup-incomplete");
        assert_eq!(seed_pin(&a).await, pin);
        assert_eq!(titles(&a).await, ["Kept"]);
    }
}

/// Unsigned refusals may hide a committed claim, so they never discard the
/// seed authority or fence the setup; a later exact retry completes it.
#[tokio::test]
async fn cli_forged_setup_refusals_keep_committed_claim_recoverable() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let hosted = Hosted::new(root, FORGE_CLAIM_REJECTED).await;
    let _server = hosted.start().await;
    let a = Installation::new(root, "a");
    a.ok(&["add", "Keep local"]).await;

    // Both the setup claim and the bearer retry commit, but come back refused.
    let error = failure(
        &a.run_with_input(&["sync", "setup", "--yes"], &hosted.setup)
            .await,
    );
    assert!(error.contains("sync-setup-invitation-rejected"), "{error}");
    assert!(hosted.claimed().await);
    assert_eq!(status(&a).await["state"], "not-set-up");
    let local = Database::open(&a.db()).await.unwrap();
    assert!(
        local
            .local_seed_genesis_commitment()
            .await
            .unwrap()
            .is_some()
    );
    assert!(local.seed_source_pin().await.unwrap().is_none());

    let wrong_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let wrong_url = format!("http://{}", wrong_listener.local_addr().unwrap());
    let wrong_requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = wrong_requests.clone();
    let wrong_app = axum::Router::new().fallback(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            axum::http::StatusCode::FORBIDDEN
        }
    });
    let wrong_task = tokio::spawn(async move {
        axum::serve(wrong_listener, wrong_app).await.unwrap();
    });
    let mut changed = SetupInvitation::decode(&hosted.setup).unwrap();
    changed.server = wrong_url;
    let changed_origin = changed.encode().unwrap();
    let pin = local.local_seed_genesis_commitment().await.unwrap();
    let error = failure(
        &a.run_with_input(&["sync", "setup", "--yes"], changed_origin.as_str())
            .await,
    );
    assert!(error.contains("sync-setup-server-mismatch"), "{error}");
    assert_eq!(wrong_requests.load(Ordering::SeqCst), 0);
    assert_eq!(local.local_seed_genesis_commitment().await.unwrap(), pin);

    // The retry claims exactly; a forged status refusal then leaves the fenced
    // setup resumable instead of requiring recovery to a new path.
    hosted.mode(FORGE_STATUS_CLAIMED);
    let error = failure(
        &a.run_with_input(&["sync", "setup", "--yes"], &hosted.setup)
            .await,
    );
    assert!(
        error.contains("sync-setup-fenced-storage-claimed"),
        "{error}"
    );
    assert_eq!(status(&a).await["state"], "setup-incomplete");
    assert!(
        local
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .is_some()
    );
    // Resuming uses the saved origin and never reads an unrelated invitation.
    changed.setup_id = [99; 32];
    let unrelated = changed.encode().unwrap();
    let error = failure(
        &a.run_with_input(&["sync", "setup"], unrelated.as_str())
            .await,
    );
    assert!(
        error.contains("sync-setup-fenced-storage-claimed"),
        "{error}"
    );
    assert_eq!(wrong_requests.load(Ordering::SeqCst), 0);
    assert_eq!(local.local_seed_genesis_commitment().await.unwrap(), pin);

    hosted.mode(RELAY);
    let stdout = success(&a.run(&["sync", "setup"]).await, &["sync", "setup"]);
    assert!(
        stdout.contains(&format!("Sync set up with {}", hosted.url)),
        "{stdout}"
    );
    assert_eq!(status(&a).await["state"], "ready");

    wrong_task.abort();
}
