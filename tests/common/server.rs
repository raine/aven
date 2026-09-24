use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::process::kill_child_and_join_threads;
use super::{TestEnv, command};

pub struct TestServer {
    child: Child,
    output: Arc<Mutex<String>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    pub url: String,
}

impl TestServer {
    /// Prepares `data` with `server setup` and serves it on a free loopback
    /// port, using the environment's configuration directory.
    pub fn start_configured_with_env<I, K, V>(env: &TestEnv, data: &str, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let config_dir = env.config_dir().join("aven");
        let data = env.path(data);
        let bind = env.free_loopback_addr();
        let setup = command()
            .env("AVEN_CONFIG_DIR", &config_dir)
            .args(["server", "setup", "--data"])
            .arg(&data)
            .args(["--url", &format!("http://{bind}")])
            .output()
            .expect("run aven server setup");
        assert!(
            setup.status.success(),
            "server setup failed\n{}",
            String::from_utf8_lossy(&setup.stderr)
        );
        let output = Arc::new(Mutex::new(String::new()));
        let (url_tx, url_rx) = mpsc::channel();
        let mut command = command();
        env.configure_command(&mut command);
        command
            .args(["server", "--bind", &bind, "--data"])
            .arg(&data)
            .env("AVEN_CONFIG_DIR", config_dir)
            .env_remove("AVEN_DB");
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn aven server");

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
