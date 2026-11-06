//! Delivery through a short-lived child of the bundle executable, `aven
//! internal notify`, which reads a [`Request`] as JSON on stdin, delivers it
//! in its own process and writes the [`Report`] as JSON on stdout.
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::{Deliver, Report, Request};

/// The child's arguments.
pub const ARGS: [&str; 2] = ["internal", "notify"];
/// How long a child may take before it is stopped. Its posts stay
/// `submitting`, for the next delivery to check.
const TIMEOUT: Duration = Duration::from_secs(60);
/// How long a child that asks the user may wait for their answer, a little
/// longer than [`super::ASK_TIMEOUT`] so it can report a missing answer.
const ASK_CHILD_TIMEOUT: Duration = Duration::from_secs(super::ASK_TIMEOUT.as_secs() + 15);
const POLL: Duration = Duration::from_millis(20);

pub struct ChildProcess {
    pub executable: PathBuf,
}

impl Deliver for ChildProcess {
    fn deliver(&self, request: &Request) -> Result<Report> {
        let mut child = Command::new(&self.executable)
            .args(ARGS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("start {}", self.executable.display()))?;
        let input = serde_json::to_vec(request)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&input)
                .context("write the notify request")?;
        }
        let mut stdout = child
            .stdout
            .take()
            .context("the notify child has no stdout")?;
        let reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).map(|_| output)
        });
        let timeout = if request.ask {
            ASK_CHILD_TIMEOUT
        } else {
            TIMEOUT
        };
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("the notify child did not finish in {} s", timeout.as_secs());
            }
            std::thread::sleep(POLL);
        };
        let output = reader
            .join()
            .map_err(|_| anyhow::anyhow!("the notify child's output reader panicked"))??;
        if !status.success() {
            bail!("the notify child exited with {status}");
        }
        serde_json::from_slice(&output).context("read the notify child's report")
    }
}

/// The child's side: delivers `input` with `deliver` and returns the report
/// to print.
pub fn serve(input: &[u8], deliver: &dyn Deliver) -> Result<Vec<u8>> {
    let request: Request = serde_json::from_slice(input).context("read the notify request")?;
    let report = deliver.deliver(&request)?;
    Ok(serde_json::to_vec(&report)?)
}
