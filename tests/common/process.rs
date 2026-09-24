use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{TestEnv, command};

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
        Self::start_daemon_with_env(env, std::iter::empty::<(&str, &str)>())
    }

    pub fn start_daemon_with_env<I, K, V>(env: &TestEnv, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let mut command = command();
        env.configure_command(&mut command);
        command
            .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
            .env_remove("AVEN_DB");
        for (key, value) in envs {
            command.env(key, value);
        }
        let child = command
            .args(["daemon"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn aven daemon");
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

pub(super) fn kill_child_and_join_threads(
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
