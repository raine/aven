use std::io;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::config::{CustomTuiCommandExecution, CustomTuiCommandSuccess};
use crate::tui::custom_command::CustomCommandInvocation;

const OUTPUT_LIMIT: usize = 16 * 1024;
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub(crate) struct CustomCommandCompletion {
    pub(crate) name: String,
    pub(crate) on_success: CustomTuiCommandSuccess,
    pub(crate) result: Result<(), String>,
}

#[derive(Default)]
pub(crate) struct CustomCommandController {
    pending: Vec<JoinHandle<CustomCommandCompletion>>,
}

impl Drop for CustomCommandController {
    fn drop(&mut self) {
        for pending in &self.pending {
            pending.abort();
        }
    }
}

impl CustomCommandController {
    pub(crate) fn launch(&mut self, invocation: CustomCommandInvocation) -> Result<()> {
        let mut command = Command::new(&invocation.program);
        command
            .args(&invocation.args)
            .current_dir(&invocation.cwd)
            .stdin(Stdio::piped());
        match invocation.execution {
            CustomTuiCommandExecution::Background => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
            CustomTuiCommandExecution::Wait => {
                command.stdout(Stdio::piped()).stderr(Stdio::piped());
            }
        }
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        command.kill_on_drop(invocation.execution == CustomTuiCommandExecution::Wait);
        let child = command.spawn().with_context(|| {
            format!(
                "could not start custom command {} ({})",
                invocation.name,
                invocation.program.display()
            )
        })?;
        let handle = match invocation.execution {
            CustomTuiCommandExecution::Background => {
                tokio::spawn(run_background(invocation, child))
            }
            CustomTuiCommandExecution::Wait => tokio::spawn(run_waiting(invocation, child)),
        };
        self.pending.push(handle);
        Ok(())
    }

    pub(crate) async fn poll(&mut self) -> Vec<CustomCommandCompletion> {
        let mut completed = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            if !self.pending[index].is_finished() {
                index += 1;
                continue;
            }
            let handle = self.pending.swap_remove(index);
            match handle.await {
                Ok(completion) => completed.push(completion),
                Err(error) => completed.push(CustomCommandCompletion {
                    name: "custom command".to_string(),
                    on_success: CustomTuiCommandSuccess::Stay,
                    result: Err(format!("custom command worker failed: {error}")),
                }),
            }
        }
        completed
    }
}

async fn run_background(
    invocation: CustomCommandInvocation,
    mut child: tokio::process::Child,
) -> CustomCommandCompletion {
    let result = write_input(&mut child, &invocation.stdin_json)
        .await
        .map_err(|error| format!("could not send input to :{}: {error}", invocation.name));
    CustomCommandCompletion {
        name: invocation.name,
        on_success: CustomTuiCommandSuccess::Stay,
        result,
    }
}

async fn run_waiting(
    invocation: CustomCommandInvocation,
    mut child: tokio::process::Child,
) -> CustomCommandCompletion {
    let name = invocation.name.clone();
    let result = async {
        write_input(&mut child, &invocation.stdin_json)
            .await
            .with_context(|| format!("could not send input to :{name}"))?;
        let stdout = child
            .stdout
            .take()
            .context("custom command stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("custom command stderr unavailable")?;
        let stdout_reader = tokio::spawn(read_bounded(stdout));
        let stderr_reader = tokio::spawn(read_bounded(stderr));
        let status = match tokio::time::timeout(WAIT_TIMEOUT, child.wait()).await {
            Ok(status) => status.context("could not wait for custom command")?,
            Err(_) => {
                let _ = child.kill().await;
                anyhow::bail!("custom command :{name} timed out");
            }
        };
        let stdout_truncated = stdout_reader
            .await
            .context("custom command stdout reader failed")??;
        let stderr_truncated = stderr_reader
            .await
            .context("custom command stderr reader failed")??;
        if stdout_truncated || stderr_truncated {
            anyhow::bail!("custom command :{name} output exceeded {OUTPUT_LIMIT} bytes");
        }
        if status.success() {
            Ok(())
        } else if let Some(code) = status.code() {
            anyhow::bail!("custom command :{name} exited with status {code}")
        } else {
            anyhow::bail!("custom command :{name} terminated without an exit code")
        }
    }
    .await
    .map_err(|error: anyhow::Error| format!("{error:#}"));
    CustomCommandCompletion {
        name: invocation.name,
        on_success: invocation.on_success,
        result,
    }
}

async fn write_input(child: &mut tokio::process::Child, input: &[u8]) -> io::Result<()> {
    let mut stdin = child.stdin.take().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            "custom command stdin unavailable",
        )
    })?;
    stdin.write_all(input).await?;
    stdin.shutdown().await
}

async fn read_bounded(mut reader: impl AsyncRead + Unpin) -> io::Result<bool> {
    let mut retained = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 4096];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(truncated);
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
        truncated |= read > remaining;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn invocation(
        program: PathBuf,
        args: Vec<String>,
        execution: CustomTuiCommandExecution,
        on_success: CustomTuiCommandSuccess,
    ) -> CustomCommandInvocation {
        CustomCommandInvocation {
            name: "dispatch".to_string(),
            program,
            args,
            cwd: std::env::current_dir().unwrap(),
            stdin_json: br#"{"version":1}"#.to_vec(),
            execution,
            on_success,
        }
    }

    async fn completion(controller: &mut CustomCommandController) -> CustomCommandCompletion {
        loop {
            let mut completed = controller.poll().await;
            if let Some(completion) = completed.pop() {
                return completion;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn waiting_command_writes_stdin_and_reports_success_policy() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("input.json");
        let mut controller = CustomCommandController::default();
        controller
            .launch(invocation(
                PathBuf::from("/usr/bin/tee"),
                vec![output.to_string_lossy().into_owned()],
                CustomTuiCommandExecution::Wait,
                CustomTuiCommandSuccess::Quit,
            ))
            .unwrap();

        let completion = completion(&mut controller).await;
        assert!(completion.result.is_ok());
        assert_eq!(completion.on_success, CustomTuiCommandSuccess::Quit);
        assert_eq!(std::fs::read(output).unwrap(), br#"{"version":1}"#);
    }

    #[tokio::test]
    async fn failed_waiting_command_reports_failure_and_stay_policy() {
        let mut controller = CustomCommandController::default();
        controller
            .launch(invocation(
                PathBuf::from("/usr/bin/false"),
                vec![],
                CustomTuiCommandExecution::Wait,
                CustomTuiCommandSuccess::Quit,
            ))
            .unwrap();

        let completion = completion(&mut controller).await;
        assert!(completion.result.is_err());
        assert_eq!(completion.on_success, CustomTuiCommandSuccess::Quit);
    }

    #[tokio::test]
    async fn missing_program_reports_spawn_failure() {
        let mut controller = CustomCommandController::default();
        let error = controller
            .launch(invocation(
                PathBuf::from("/definitely/missing/aven-command"),
                vec![],
                CustomTuiCommandExecution::Wait,
                CustomTuiCommandSuccess::Stay,
            ))
            .unwrap_err();

        assert!(error.to_string().contains("could not start custom command"));
    }

    #[tokio::test]
    async fn waiting_command_reports_output_truncation() {
        let mut controller = CustomCommandController::default();
        controller
            .launch(invocation(
                PathBuf::from("/bin/sh"),
                vec![
                    "-c".to_string(),
                    "cat >/dev/null; head -c 20000 /dev/zero".to_string(),
                ],
                CustomTuiCommandExecution::Wait,
                CustomTuiCommandSuccess::Stay,
            ))
            .unwrap();

        let completion = completion(&mut controller).await;
        assert!(completion.result.unwrap_err().contains("output exceeded"));
    }

    #[tokio::test]
    async fn background_command_reports_launch_without_quit_policy() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("background.json");
        let mut controller = CustomCommandController::default();
        controller
            .launch(invocation(
                PathBuf::from("/usr/bin/tee"),
                vec![output.to_string_lossy().into_owned()],
                CustomTuiCommandExecution::Background,
                CustomTuiCommandSuccess::Stay,
            ))
            .unwrap();

        let completion = completion(&mut controller).await;
        assert!(completion.result.is_ok());
        assert_eq!(completion.on_success, CustomTuiCommandSuccess::Stay);
    }
}
