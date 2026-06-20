#![allow(dead_code)]

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection as _, SqliteConnection};
use tempfile::TempDir;

pub struct TestEnv {
    temp: TempDir,
}

impl TestEnv {
    pub fn new() -> Self {
        Self {
            temp: tempfile::tempdir().expect("create temp dir"),
        }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    pub fn db(&self, name: &str) -> PathBuf {
        self.path(name)
    }

    pub fn config_dir(&self) -> PathBuf {
        self.path("config")
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir()
            .join("agentic-task-manager")
            .join("config.toml")
    }

    pub fn free_loopback_addr(&self) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind free loopback port");
        let addr = listener.local_addr().expect("free loopback addr");
        addr.to_string()
    }

    pub fn write_config(&self, text: &str) {
        let path = self.config_file();
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config dir");
        std::fs::write(path, text).expect("write config");
    }

    pub fn write_daemon_config(
        &self,
        db: &Path,
        server: &TestServer,
        wake_addr: &str,
        interval: u64,
    ) {
        self.write_daemon_config_with_auth(db, server, wake_addr, interval, None);
    }

    pub fn write_daemon_config_with_auth(
        &self,
        db: &Path,
        server: &TestServer,
        wake_addr: &str,
        interval: u64,
        auth_token: Option<&str>,
    ) {
        let auth_line = match auth_token {
            Some(token) => format!("auth_token = \"{token}\"\n"),
            None => String::new(),
        };
        self.write_config(&format!(
            r#"
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "{}"
interval_seconds = {}
{auth_line}
[daemon]
wake_addr = "{}"
"#,
            db.display(),
            server.url,
            interval,
            wake_addr
        ));
    }

    pub fn atm_config<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = command();
        command
            .env(
                "ATM_CONFIG_DIR",
                self.config_dir().join("agentic-task-manager"),
            )
            .env_remove("ATM_DB")
            .env_remove("ATM_SYNC_SERVER");
        command.args(args).output().expect("run atm with config")
    }

    pub fn atm_config_stdin<I, S>(&self, args: I, input: &str) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut child = command();
        child
            .env(
                "ATM_CONFIG_DIR",
                self.config_dir().join("agentic-task-manager"),
            )
            .env_remove("ATM_DB")
            .env_remove("ATM_SYNC_SERVER")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = child.spawn().expect("spawn atm with config stdin");
        child
            .stdin
            .as_mut()
            .expect("stdin pipe")
            .write_all(input.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait for atm")
    }

    pub fn atm<I, S>(&self, db: &Path, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        command_with_db(db).args(args).output().expect("run atm")
    }

    pub fn atm_in<I, S>(&self, db: &Path, cwd: &Path, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        command_with_db(db)
            .current_dir(cwd)
            .args(args)
            .output()
            .expect("run atm in cwd")
    }

    pub fn atm_stdin<I, S>(&self, db: &Path, args: I, input: &str) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut child = command_with_db(db)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn atm with stdin");
        child
            .stdin
            .as_mut()
            .expect("stdin pipe")
            .write_all(input.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait for atm")
    }
}

pub fn bin() -> PathBuf {
    cargo_bin("atm")
}

pub fn command() -> Command {
    Command::new(bin())
}

pub fn command_with_db(db: &Path) -> Command {
    let mut command = command();
    command.arg("--db").arg(db);
    command
}

pub fn ok(output: Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "expected success\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout,
        stderr
    );
    stdout
}

pub fn fail(output: Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "expected failure\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    format!("{stdout}{stderr}")
}

pub fn extract_ref(output: &str) -> String {
    output
        .split_whitespace()
        .nth(1)
        .expect("mutation output ref")
        .to_string()
}

pub fn suffix(task_ref: &str) -> String {
    task_ref
        .split_once('-')
        .map(|(_, suffix)| suffix.to_string())
        .unwrap_or_else(|| task_ref.to_string())
}

pub fn contains_all(text: &str, needles: &[&str]) {
    for needle in needles {
        assert!(text.contains(needle), "missing {needle:?}\ntext:\n{text}");
    }
}

pub fn contains_none(text: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            !text.contains(needle),
            "unexpected {needle:?}\ntext:\n{text}"
        );
    }
}

pub struct TestProcess {
    child: Child,
    output: Arc<Mutex<String>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
}

impl TestProcess {
    fn capture(mut child: Child) -> Self {
        let output = Arc::new(Mutex::new(String::new()));
        let stdout = child.stdout.take().expect("process stdout");
        let stdout_output = Arc::clone(&output);
        let stdout_thread = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let mut output = stdout_output.lock().expect("process output lock");
                output.push_str(&line);
                output.push('\n');
            }
        });

        let stderr = child.stderr.take().expect("process stderr");
        let stderr_output = Arc::clone(&output);
        let stderr_thread = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let mut output = stderr_output.lock().expect("process output lock");
                output.push_str(&line);
                output.push('\n');
            }
        });

        Self {
            child,
            output,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
        }
    }

    pub fn start_server<I, S>(env: &TestEnv, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let child = command()
            .env(
                "ATM_CONFIG_DIR",
                env.config_dir().join("agentic-task-manager"),
            )
            .env_remove("ATM_DB")
            .env_remove("ATM_SYNC_SERVER")
            .arg("server")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn atm server");
        Self::capture(child)
    }

    pub fn start_daemon(env: &TestEnv) -> Self {
        Self::start_daemon_with_env(env, std::iter::empty::<(&str, &str)>())
    }

    pub fn start_daemon_with_env<I, K, V>(env: &TestEnv, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let mut command = command();
        command
            .env(
                "ATM_CONFIG_DIR",
                env.config_dir().join("agentic-task-manager"),
            )
            .env_remove("ATM_DB")
            .env_remove("ATM_SYNC_SERVER");
        for (key, value) in envs {
            command.env(key, value);
        }
        let child = command
            .args(["daemon", "run"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn atm daemon");
        let process = Self::capture(child);
        process.wait_for_log("daemon db=", Duration::from_secs(10));
        process
    }

    pub fn output(&self) -> String {
        self.output.lock().expect("process output lock").clone()
    }

    pub fn log_mark(&self) -> usize {
        self.output().len()
    }

    pub fn wait_for_log(&self, pattern: &str, timeout: Duration) {
        self.wait_for_log_after(0, pattern, timeout);
    }

    pub fn wait_for_log_after(&self, mark: usize, pattern: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let output = self.output();
            if output
                .get(mark..)
                .is_some_and(|text| text.contains(pattern))
            {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {pattern:?}\n{}", self.output());
    }
}

fn kill_child_and_join_threads(
    child: &mut Child,
    stdout_thread: &mut Option<JoinHandle<()>>,
    stderr_thread: &mut Option<JoinHandle<()>>,
) {
    let _ = child.kill();
    let _ = child.wait();
    if let Some(thread) = stdout_thread.take() {
        let _ = thread.join();
    }
    if let Some(thread) = stderr_thread.take() {
        let _ = thread.join();
    }
}

impl Drop for TestProcess {
    fn drop(&mut self) {
        kill_child_and_join_threads(
            &mut self.child,
            &mut self.stdout_thread,
            &mut self.stderr_thread,
        );
    }
}

pub fn eventually<F>(timeout: Duration, mut check: F)
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(check(), "condition was not met within {timeout:?}");
}

pub fn meta_value(db: &Path, key: &str) -> Option<String> {
    let runtime = test_runtime();
    runtime.block_on(async {
        let mut conn = open_test_db(db).await;
        sqlx::query_scalar::<_, String>("SELECT value FROM meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&mut conn)
            .await
            .expect("read meta value")
    })
}

pub fn scalar_i64(db: &Path, sql: &'static str) -> i64 {
    let runtime = test_runtime();
    runtime.block_on(async {
        let mut conn = open_test_db(db).await;
        sqlx::query_scalar::<_, i64>(sql)
            .fetch_one(&mut conn)
            .await
            .expect("read scalar value")
    })
}

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create tokio runtime")
}

pub async fn insert_task_fixtures(pool: &sqlx::SqlitePool, fixtures: &[(&str, &str, &str)]) {
    for (id, title, project_key) in fixtures {
        sqlx::query(
            "INSERT INTO tasks(id,title,description,project_key,status,priority,created_at,updated_at)
             VALUES (?, ?, '', ?, 'inbox', 'none', 't', 't')",
        )
        .bind(id)
        .bind(title)
        .bind(project_key)
        .execute(pool)
        .await
        .expect("insert task fixture");
    }
}

async fn open_test_db(db: &Path) -> SqliteConnection {
    let options = SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(false)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    SqliteConnection::connect_with(&options)
        .await
        .expect("open sqlite db")
}

pub struct TestServer {
    child: Child,
    output: Arc<Mutex<String>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    pub url: String,
}

impl TestServer {
    pub fn start(env: &TestEnv) -> Self {
        Self::start_with_data(env, "server.sqlite")
    }

    pub fn start_configured(env: &TestEnv, data: &str) -> Self {
        Self::start_configured_with_env(env, data, std::iter::empty::<(&str, &str)>())
    }

    pub fn start_configured_with_env<I, K, V>(env: &TestEnv, data: &str, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        Self::start_with_data_and_config(
            env,
            data,
            Some(env.config_dir().join("agentic-task-manager")),
            envs,
        )
    }

    pub fn start_with_data(env: &TestEnv, data: &str) -> Self {
        Self::start_with_data_and_config(env, data, None, std::iter::empty::<(&str, &str)>())
    }

    fn start_with_data_and_config<I, K, V>(
        env: &TestEnv,
        data: &str,
        config_dir: Option<PathBuf>,
        envs: I,
    ) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let output = Arc::new(Mutex::new(String::new()));
        let (url_tx, url_rx) = mpsc::channel();
        let mut command = command();
        command.args([
            "server",
            "--bind",
            "127.0.0.1:0",
            "--data",
            env.path(data).to_str().expect("utf8 temp path"),
        ]);
        if let Some(config_dir) = config_dir {
            command
                .env("ATM_CONFIG_DIR", config_dir)
                .env_remove("ATM_DB")
                .env_remove("ATM_SYNC_SERVER");
        }
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn atm server");

        let stdout = child.stdout.take().expect("server stdout");
        let stdout_output = Arc::clone(&output);
        let stdout_thread = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                {
                    let mut output = stdout_output.lock().expect("server output lock");
                    output.push_str(&line);
                    output.push('\n');
                }
                if let Some(rest) = line.strip_prefix("listening url=") {
                    let url = rest.split_whitespace().next().expect("listening url value");
                    let _ = url_tx.send(url.to_string());
                }
            }
        });

        let stderr = child.stderr.take().expect("server stderr");
        let stderr_output = Arc::clone(&output);
        let stderr_thread = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let mut output = stderr_output.lock().expect("server output lock");
                output.push_str(&line);
                output.push('\n');
            }
        });

        let deadline = Instant::now() + Duration::from_secs(10);
        let url = loop {
            if let Ok(url) = url_rx.try_recv() {
                break url;
            }
            if let Some(status) = child.try_wait().expect("check server status") {
                panic!(
                    "server exited during startup: {status}\n{}",
                    output.lock().expect("server output lock")
                );
            }
            assert!(
                Instant::now() < deadline,
                "server did not print listening url\n{}",
                output.lock().expect("server output lock")
            );
            thread::sleep(Duration::from_millis(50));
        };

        Self {
            child,
            output,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            url,
        }
    }

    pub fn output(&self) -> String {
        self.output.lock().expect("server output lock").clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        kill_child_and_join_threads(
            &mut self.child,
            &mut self.stdout_thread,
            &mut self.stderr_thread,
        );
    }
}
