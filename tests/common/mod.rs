#![allow(dead_code)]

mod database;
mod process;
mod server;

#[allow(unused_imports)]
pub use database::{execute_sql, insert_task_fixtures, meta_value, scalar_i64};
#[allow(unused_imports)]
pub use process::TestProcess;
#[allow(unused_imports)]
pub use server::TestServer;

use std::ffi::OsStr;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
        self.config_dir().join("aven").join("config.yaml")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.path("state")
    }

    fn configure_command(&self, command: &mut Command) {
        command
            .env("XDG_STATE_HOME", self.state_dir())
            .env("AVEN_CONFIG_DIR", self.config_dir().join("aven"))
            .env_remove("AVEN_DEV_DB")
            .env_remove("AVEN_DB")
            .env_remove("AVEN_SYNC_DISABLED");
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

    pub fn write_daemon_config(&self, db: &Path, wake_addr: &str, interval: u64) {
        self.write_config(&format!(
            r#"
local:
  db_path: "{}"

sync:
  enabled: true
  interval_seconds: {}
daemon:
  wake_addr: "{}"
"#,
            db.display(),
            interval,
            wake_addr
        ));
    }

    pub fn aven_config<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = command();
        self.configure_command(&mut command);
        command
            .env("AVEN_CONFIG_DIR", self.config_dir().join("aven"))
            .env_remove("AVEN_DEV_DB")
            .env_remove("AVEN_DB")
            .env_remove("AVEN_SYNC_SERVER");
        command.args(args).output().expect("run aven with config")
    }

    pub fn aven_config_env<I, S, E, K, V>(&self, args: I, envs: E) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
        E: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let mut command = command();
        self.configure_command(&mut command);
        command.envs(envs);
        command
            .args(args)
            .output()
            .expect("run aven with config and env")
    }

    pub fn aven_config_stdin<I, S>(&self, args: I, input: &str) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut child = command();
        self.configure_command(&mut child);
        child
            .env("AVEN_CONFIG_DIR", self.config_dir().join("aven"))
            .env_remove("AVEN_DB")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = child.spawn().expect("spawn aven with config stdin");
        child
            .stdin
            .as_mut()
            .expect("stdin pipe")
            .write_all(input.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait for aven")
    }

    pub fn aven<I, S>(&self, db: &Path, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = command_with_db(db);
        self.configure_command(&mut command);
        command.args(args).output().expect("run aven")
    }

    pub fn aven_in<I, S>(&self, db: &Path, cwd: &Path, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = command_with_db(db);
        self.configure_command(&mut command);
        command
            .current_dir(cwd)
            .args(args)
            .output()
            .expect("run aven in cwd")
    }

    pub fn aven_stdin<I, S>(&self, db: &Path, args: I, input: &str) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = command_with_db(db);
        self.configure_command(&mut command);
        let mut child = command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn aven with stdin");
        child
            .stdin
            .as_mut()
            .expect("stdin pipe")
            .write_all(input.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait for aven")
    }

    pub fn aven_ok<I, S>(&self, db: &Path, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.aven(db, args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

pub fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_aven"))
}

pub fn command() -> Command {
    let mut command = Command::new(bin());
    command.env_remove("AVEN_DEV_DB");
    command
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

pub fn extract_attachment_id(output: &str) -> String {
    output
        .split_whitespace()
        .find_map(|part| part.strip_prefix("attachment_id="))
        .expect("attachment id in output")
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

pub fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode PNG fixture");
    bytes.into_inner()
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
