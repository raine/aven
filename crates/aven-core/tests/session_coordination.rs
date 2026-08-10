use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use aven_core::db::Database;
use aven_core::sync::{SyncHttpResponse, SyncLockPolicy, SyncSession, SyncSessionBusy};

const TEST_SERVER: &str = "https://sync.example.test";
const HOLDER_TEST: &str = "sync_lock_holder_process";
const ENV_DATABASE: &str = "AVEN_SESSION_COORDINATION_TEST_DATABASE";
const ENV_READY: &str = "AVEN_SESSION_COORDINATION_TEST_READY";
const ENV_RELEASE: &str = "AVEN_SESSION_COORDINATION_TEST_RELEASE";
const PROCESS_WAIT: Duration = Duration::from_secs(20);

struct ProcessLockFixture {
    _directory: tempfile::TempDir,
    database: Database,
    child: Option<Child>,
    ready: PathBuf,
    release: PathBuf,
    owner_pid: u32,
}

impl ProcessLockFixture {
    async fn spawn() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("aven.sqlite");
        let database = Database::open(&database_path).await.unwrap();
        let ready = directory.path().join("holder.ready");
        let release = directory.path().join("holder.release");
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                HOLDER_TEST,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(ENV_DATABASE, &database_path)
            .env(ENV_READY, &ready)
            .env(ENV_RELEASE, &release)
            .spawn()
            .unwrap();
        wait_for_path(&ready);
        let owner_pid = std::fs::read_to_string(&ready)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        Self {
            _directory: directory,
            database,
            child: Some(child),
            ready,
            release,
            owner_pid,
        }
    }

    fn release_and_wait(&mut self) {
        std::fs::write(&self.release, b"release").unwrap();
        self.wait_for_exit(true);
    }

    fn kill_and_wait(&mut self) {
        if let Some(child) = self.child.as_mut() {
            child.kill().unwrap();
        }
        std::fs::write(&self.release, b"killed").unwrap();
        self.wait_for_exit(false);
    }

    fn wait_for_exit(&mut self, expect_success: bool) {
        let deadline = Instant::now() + PROCESS_WAIT;
        let child = self.child.as_mut().unwrap();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                if expect_success {
                    assert!(status.success(), "sync lock holder failed: {status}");
                }
                self.child = None;
                return;
            }
            assert!(Instant::now() < deadline, "sync lock holder did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ProcessLockFixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.ready);
    }
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + PROCESS_WAIT;
    while !path.exists() {
        assert!(Instant::now() < deadline, "sync lock holder did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn start_parent_session(
    database: Database,
    policy: SyncLockPolicy,
) -> anyhow::Result<SyncSession> {
    SyncSession::start_with_lock_policy(database, TEST_SERVER.to_string(), None, None, policy).await
}

#[tokio::test]
async fn another_process_reports_contention_without_persistence_noise() {
    let fixture = ProcessLockFixture::spawn().await;
    let before = fixture.database.sync_persistence_status().await.unwrap();

    let error = match start_parent_session(fixture.database.clone(), SyncLockPolicy::Defer).await {
        Ok(_) => panic!("contended session must not start"),
        Err(error) => error,
    };

    assert_eq!(
        error.downcast_ref::<SyncSessionBusy>().unwrap().owner_pid(),
        Some(fixture.owner_pid)
    );
    assert_eq!(
        fixture.database.sync_persistence_status().await.unwrap(),
        before
    );
}

#[tokio::test]
async fn another_process_releases_coordination_after_normal_session_drop() {
    let mut fixture = ProcessLockFixture::spawn().await;
    fixture.release_and_wait();

    assert!(
        start_parent_session(fixture.database.clone(), SyncLockPolicy::Defer)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn operating_system_releases_coordination_after_process_exit() {
    let mut fixture = ProcessLockFixture::spawn().await;
    fixture.kill_and_wait();

    assert!(
        start_parent_session(fixture.database.clone(), SyncLockPolicy::Defer)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn manual_contention_waits_for_normal_release() {
    let mut fixture = ProcessLockFixture::spawn().await;
    let release = fixture.release.clone();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(release, b"release").unwrap();
    });
    let started = Instant::now();

    let session = start_parent_session(fixture.database.clone(), SyncLockPolicy::Manual).await;

    releaser.join().unwrap();
    fixture.wait_for_exit(true);
    assert!(session.is_ok());
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn manual_contention_has_a_bounded_active_owner_error() {
    let fixture = ProcessLockFixture::spawn().await;
    let started = Instant::now();

    let error = match start_parent_session(fixture.database.clone(), SyncLockPolicy::Manual).await {
        Ok(_) => panic!("contended manual session must not start"),
        Err(error) => error,
    };

    assert!(started.elapsed() >= Duration::from_millis(1_900));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(
        error.downcast_ref::<SyncSessionBusy>().unwrap().owner_pid(),
        Some(fixture.owner_pid)
    );
}

#[tokio::test]
async fn canonical_path_aliases_share_coordination() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aven.sqlite");
    let alias_directory = directory.path().join("alias");
    std::fs::create_dir(&alias_directory).unwrap();
    let alias = alias_directory.join("..").join("aven.sqlite");
    let first = Database::open(&path).await.unwrap();
    let second = Database::open(&alias).await.unwrap();
    let _active = SyncSession::start(first, TEST_SERVER.to_string(), None, None)
        .await
        .unwrap();

    let error = match start_parent_session(second, SyncLockPolicy::Defer).await {
        Ok(_) => panic!("canonical database aliases must contend"),
        Err(error) => error,
    };

    assert!(error.downcast_ref::<SyncSessionBusy>().is_some());
}

#[tokio::test]
async fn distinct_file_databases_coordinate_independently() {
    let directory = tempfile::tempdir().unwrap();
    let first = Database::open(&directory.path().join("first.sqlite"))
        .await
        .unwrap();
    let second = Database::open(&directory.path().join("second.sqlite"))
        .await
        .unwrap();
    let _active = SyncSession::start(first, TEST_SERVER.to_string(), None, None)
        .await
        .unwrap();

    assert!(
        start_parent_session(second, SyncLockPolicy::Defer)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn in_memory_sessions_retain_concurrent_behavior() {
    let database = Database::open(Path::new(":memory:")).await.unwrap();
    let _first = SyncSession::start(database.clone(), TEST_SERVER.to_string(), None, None)
        .await
        .unwrap();

    assert!(
        start_parent_session(database, SyncLockPolicy::Defer)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn completed_session_releases_coordination_while_retained() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();
    let mut completed = SyncSession::start(database.clone(), TEST_SERVER.to_string(), None, None)
        .await
        .unwrap();
    let prepared = completed.prepare_request().await.unwrap().unwrap();
    completed
        .accept_response(
            &prepared.context,
            SyncHttpResponse {
                status: 200,
                headers: Vec::new(),
                body: format!(
                    "{{\"protocol_version\":{},\"cursor\":0,\"has_more\":false,\"push_acks\":[],\"changes\":[]}}",
                    aven_core::sync::wire::SYNC_PROTOCOL_VERSION
                )
                .into_bytes(),
            },
        )
        .await
        .unwrap();

    assert!(completed.summary().complete);
    assert!(
        start_parent_session(database, SyncLockPolicy::Defer)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn failed_session_releases_coordination_while_retained() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();
    let mut failed = SyncSession::start(database.clone(), TEST_SERVER.to_string(), None, None)
        .await
        .unwrap();
    failed.fail("terminal test failure").await.unwrap();

    assert!(
        start_parent_session(database, SyncLockPolicy::Defer)
            .await
            .is_ok()
    );
}

#[tokio::test]
#[ignore = "spawned by multiprocess session coordination tests"]
async fn sync_lock_holder_process() {
    let database_path = PathBuf::from(std::env::var_os(ENV_DATABASE).unwrap());
    let ready = PathBuf::from(std::env::var_os(ENV_READY).unwrap());
    let release = PathBuf::from(std::env::var_os(ENV_RELEASE).unwrap());
    let database = Database::open(&database_path).await.unwrap();
    let _session = SyncSession::start(database, TEST_SERVER.to_string(), None, None)
        .await
        .unwrap();
    std::fs::write(&ready, std::process::id().to_string()).unwrap();

    while !release.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
