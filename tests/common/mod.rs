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

    pub fn start_daemon(env: &TestEnv) -> Self {
        let child = command()
            .env(
                "ATM_CONFIG_DIR",
                env.config_dir().join("agentic-task-manager"),
            )
            .env_remove("ATM_DB")
            .env_remove("ATM_SYNC_SERVER")
            .args(["daemon", "run"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn atm daemon");
        let process = Self::capture(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let output = process.output();
            if output.contains("daemon db=") {
                return process;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("daemon did not start\n{}", process.output());
    }

    pub fn output(&self) -> String {
        self.output.lock().expect("process output lock").clone()
    }
}

impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
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
        let output = Arc::new(Mutex::new(String::new()));
        let (url_tx, url_rx) = mpsc::channel();
        let mut child = command()
            .args([
                "server",
                "--bind",
                "127.0.0.1:0",
                "--data",
                env.path("server.sqlite").to_str().expect("utf8 temp path"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn atm server");

        let stdout = child.stdout.take().expect("server stdout");
        let stdout_output = Arc::clone(&output);
        let stdout_thread = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(url) = line.strip_prefix("listening url=") {
                    let _ = url_tx.send(url.to_string());
                }
                let mut output = stdout_output.lock().expect("server output lock");
                output.push_str(&line);
                output.push('\n');
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
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}
