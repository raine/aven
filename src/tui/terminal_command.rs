use std::future::Future;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;

use anyhow::Result;
use tempfile::TempDir;
use tokio::process::Command;

use crate::tui::custom_command::CustomCommandInvocation;
use crate::tui::custom_command_runtime::{
    ProcessIdentity, terminate_and_reap, validate_working_directory,
};
use crate::tui::platform::{SuspendedTerminal, TerminalTransition};

pub(crate) const CONTEXT_ENV: &str = "AVEN_COMMAND_CONTEXT";

struct TerminalCommandContext {
    directory: Option<TempDir>,
    path: PathBuf,
}

impl TerminalCommandContext {
    fn create(json: &[u8]) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("aven-command-")
            .tempdir()
            .map_err(|_| anyhow::anyhow!("could not create terminal command context"))?;
        let path = directory.path().join("context.json");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|_| anyhow::anyhow!("could not create terminal command context"))?;
        file.write_all(json)
            .and_then(|()| file.flush())
            .map_err(|_| anyhow::anyhow!("could not write terminal command context"))?;
        drop(file);
        Ok(Self {
            directory: Some(directory),
            path,
        })
    }

    fn cleanup(&mut self) -> Result<()> {
        let Some(directory) = self.directory.take() else {
            return Ok(());
        };
        directory
            .close()
            .map_err(|_| anyhow::anyhow!("could not remove terminal command context"))
    }
}

pub(crate) trait TerminalProcessRunner {
    fn run<'a>(
        &'a mut self,
        invocation: &'a CustomCommandInvocation,
        context_path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), String>> + 'a>>;
}

pub(crate) struct SystemTerminalProcessRunner;

impl TerminalProcessRunner for SystemTerminalProcessRunner {
    fn run<'a>(
        &'a mut self,
        invocation: &'a CustomCommandInvocation,
        context_path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), String>> + 'a>> {
        Box::pin(run_terminal_process(invocation, context_path))
    }
}

pub(crate) async fn execute_terminal_invocation_with<
    T: TerminalTransition,
    R: TerminalProcessRunner,
>(
    transition: &mut T,
    invocation: &CustomCommandInvocation,
    runner: &mut R,
) -> std::result::Result<(), String> {
    validate_working_directory(invocation).map_err(|error| format!("{error:#}"))?;
    let mut context = TerminalCommandContext::create(&invocation.stdin_json)
        .map_err(|error| format!("{error:#}"))?;
    let mut suspended = SuspendedTerminal::suspend(transition)
        .map_err(|error| format!("could not suspend Aven terminal: {error:#}"))?;
    let process_result = runner.run(invocation, &context.path).await;
    let restore_result = suspended
        .restore()
        .map_err(|error| format!("could not restore Aven terminal: {error:#}"));
    drop(suspended);
    let cleanup_result = context.cleanup().map_err(|error| format!("{error:#}"));

    match (process_result, restore_result, cleanup_result) {
        (Err(mut error), restore, cleanup) => {
            if restore.is_err() {
                error.push_str("; terminal restoration failed");
            }
            if cleanup.is_err() {
                error.push_str("; context cleanup failed");
            }
            Err(error)
        }
        (Ok(()), Err(error), _) | (Ok(()), Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(unix)]
struct ForegroundProcessGuard {
    terminal_fd: libc::c_int,
    aven_process_group: libc::pid_t,
    restore_attempted: bool,
}

#[cfg(unix)]
impl ForegroundProcessGuard {
    fn assign(process: ProcessIdentity) -> io::Result<Option<Self>> {
        let terminal_fd = libc::STDIN_FILENO;
        let aven_process_group = unsafe { libc::tcgetpgrp(terminal_fd) };
        if aven_process_group == -1 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ENOTTY) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let child_process_group = i32::try_from(process.pid())
            .map_err(|_| io::Error::other("custom command PID exceeds process group range"))?;
        set_foreground_process_group(terminal_fd, child_process_group)?;
        Ok(Some(Self {
            terminal_fd,
            aven_process_group,
            restore_attempted: false,
        }))
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restore_attempted {
            return Ok(());
        }
        self.restore_attempted = true;
        set_foreground_process_group(self.terminal_fd, self.aven_process_group)
    }
}

#[cfg(unix)]
impl Drop for ForegroundProcessGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(unix)]
fn set_foreground_process_group(
    terminal_fd: libc::c_int,
    process_group: libc::pid_t,
) -> io::Result<()> {
    unsafe {
        let mut blocked = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        if libc::sigemptyset(blocked.as_mut_ptr()) != 0
            || libc::sigaddset(blocked.as_mut_ptr(), libc::SIGTTOU) != 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut previous = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        let mask_result =
            libc::pthread_sigmask(libc::SIG_BLOCK, blocked.as_ptr(), previous.as_mut_ptr());
        if mask_result != 0 {
            return Err(io::Error::from_raw_os_error(mask_result));
        }
        let foreground_result = libc::tcsetpgrp(terminal_fd, process_group);
        let foreground_error = (foreground_result != 0).then(io::Error::last_os_error);
        let restore_result =
            libc::pthread_sigmask(libc::SIG_SETMASK, previous.as_ptr(), std::ptr::null_mut());
        if let Some(error) = foreground_error {
            Err(error)
        } else if restore_result != 0 {
            Err(io::Error::from_raw_os_error(restore_result))
        } else {
            Ok(())
        }
    }
}

#[cfg(not(unix))]
struct ForegroundProcessGuard;

#[cfg(not(unix))]
impl ForegroundProcessGuard {
    fn assign(_process: ProcessIdentity) -> io::Result<Option<Self>> {
        Ok(None)
    }
}

#[cfg(unix)]
fn restore_foreground_process(foreground: &mut Option<ForegroundProcessGuard>) -> io::Result<()> {
    match foreground {
        Some(foreground) => foreground.restore(),
        None => Ok(()),
    }
}

#[cfg(not(unix))]
fn restore_foreground_process(_foreground: &mut Option<ForegroundProcessGuard>) -> io::Result<()> {
    Ok(())
}

async fn run_terminal_process(
    invocation: &CustomCommandInvocation,
    context_path: &Path,
) -> std::result::Result<(), String> {
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .current_dir(&invocation.cwd)
        .envs(&invocation.env)
        .env(CONTEXT_ENV, context_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    crate::tui::platform::configure_terminal_child_signals(command.as_std_mut());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| {
        format!(
            "could not start custom command :{}: {}",
            invocation.name,
            bounded_io_error(&error)
        )
    })?;
    let process = ProcessIdentity::from_child(&child)
        .map_err(|_| format!("could not supervise custom command :{}", invocation.name))?;
    let mut foreground = ForegroundProcessGuard::assign(process).map_err(|_| {
        format!(
            "could not give terminal control to custom command :{}",
            invocation.name
        )
    })?;

    let process_result = match invocation.timeout {
        Some(timeout) => match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => terminal_status_result(&invocation.name, status),
            Err(_) => {
                let cleanup = terminate_and_reap(&mut child, process).await.err();
                let mut message = format!("custom command :{} timed out", invocation.name);
                if cleanup.is_some() {
                    message.push_str("; process cleanup failed");
                }
                Err(message)
            }
        },
        None => terminal_status_result(&invocation.name, child.wait().await),
    };
    let foreground_result = restore_foreground_process(&mut foreground);
    match (process_result, foreground_result) {
        (result, Ok(())) => result,
        (Ok(()), Err(_)) => Err(format!(
            "could not reclaim terminal control after custom command :{}",
            invocation.name
        )),
        (Err(mut error), Err(_)) => {
            error.push_str("; terminal control restoration failed");
            Err(error)
        }
    }
}

fn terminal_status_result(
    name: &str,
    status: io::Result<std::process::ExitStatus>,
) -> std::result::Result<(), String> {
    let status = status.map_err(|error| {
        format!(
            "could not wait for custom command :{name}: {}",
            bounded_io_error(&error)
        )
    })?;
    if status.success() {
        Ok(())
    } else if let Some(code) = status.code() {
        Err(format!("custom command :{name} exited with status {code}"))
    } else {
        Err(format!(
            "custom command :{name} terminated without an exit code"
        ))
    }
}

fn bounded_io_error(error: &io::Error) -> String {
    error
        .to_string()
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::io::IsTerminal;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::config::{CustomTuiCommandExecution, CustomTuiCommandSuccess};

    #[derive(Default)]
    struct FakeTransition {
        suspend_count: usize,
        restore_count: usize,
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_suspend: bool,
    }

    impl TerminalTransition for FakeTransition {
        fn suspend(&mut self) -> Result<()> {
            self.suspend_count += 1;
            self.events.lock().unwrap().push("suspend");
            if self.fail_suspend {
                anyhow::bail!("fixture suspension failure");
            }
            Ok(())
        }

        fn restore(&mut self) -> Result<()> {
            self.restore_count += 1;
            self.events.lock().unwrap().push("restore");
            Ok(())
        }
    }

    struct FakeRunner {
        result: std::result::Result<(), String>,
        expected_json: Vec<u8>,
        events: Arc<Mutex<Vec<&'static str>>>,
        observed_path: Option<PathBuf>,
        observed_mode: Option<u32>,
    }

    impl TerminalProcessRunner for FakeRunner {
        fn run<'a>(
            &'a mut self,
            _invocation: &'a CustomCommandInvocation,
            context_path: &'a Path,
        ) -> Pin<Box<dyn Future<Output = std::result::Result<(), String>> + 'a>> {
            let contents = std::fs::read(context_path).unwrap();
            assert_eq!(contents, self.expected_json);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                self.observed_mode = Some(
                    std::fs::metadata(context_path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                );
            }
            self.observed_path = Some(context_path.to_path_buf());
            self.events.lock().unwrap().push("run");
            let result = self.result.clone();
            Box::pin(async move { result })
        }
    }

    fn invocation() -> CustomCommandInvocation {
        CustomCommandInvocation {
            name: "agent".into(),
            program: "fixture".into(),
            args: vec!["--interactive".into()],
            cwd: std::env::current_dir().unwrap(),
            env: BTreeMap::<OsString, OsString>::new(),
            timeout: None,
            stdin_json: b"{\"version\":1}\n".to_vec(),
            execution: CustomTuiCommandExecution::Terminal,
            on_success: CustomTuiCommandSuccess::Stay,
        }
    }

    #[tokio::test]
    async fn context_lifecycle_and_restoration_wrap_every_runner_result() {
        for result in [
            Ok(()),
            Err("nonzero".into()),
            Err("signal".into()),
            Err("timeout".into()),
            Err("wait".into()),
            Err("spawn".into()),
        ] {
            let invocation = invocation();
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut transition = FakeTransition {
                events: Arc::clone(&events),
                ..FakeTransition::default()
            };
            let mut runner = FakeRunner {
                result: result.clone(),
                expected_json: invocation.stdin_json.clone(),
                events: Arc::clone(&events),
                observed_path: None,
                observed_mode: None,
            };
            let actual =
                execute_terminal_invocation_with(&mut transition, &invocation, &mut runner).await;
            assert_eq!(actual, result);
            assert_eq!(transition.suspend_count, 1);
            assert_eq!(transition.restore_count, 1);
            assert_eq!(*events.lock().unwrap(), ["suspend", "run", "restore"]);
            assert!(!runner.observed_path.unwrap().exists());
            #[cfg(unix)]
            assert_eq!(runner.observed_mode, Some(0o600));
        }
    }

    #[test]
    fn suspension_failure_attempts_one_restoration() {
        let mut transition = FakeTransition {
            fail_suspend: true,
            ..FakeTransition::default()
        };
        let error = SuspendedTerminal::suspend(&mut transition)
            .err()
            .expect("suspension fails");
        assert!(format!("{error:#}").contains("fixture suspension failure"));
        assert_eq!(transition.suspend_count, 1);
        assert_eq!(transition.restore_count, 1);
    }

    #[test]
    fn suspended_terminal_restores_during_unwinding() {
        let mut transition = FakeTransition::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _suspended = SuspendedTerminal::suspend(&mut transition).unwrap();
            panic!("fixture panic");
        }));
        assert!(result.is_err());
        assert_eq!(transition.suspend_count, 1);
        assert_eq!(transition.restore_count, 1);
    }

    fn compile_terminal_fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("terminal_fixture.rs");
        let executable = directory.path().join("terminal-fixture");
        std::fs::write(
            &source,
            r#"
use std::io::IsTerminal;

fn main() {
    let context = std::env::var_os("AVEN_COMMAND_CONTEXT").expect("context environment");
    let argument = std::env::args().nth(1).unwrap_or_default();
    if argument == "--sleep" {
        std::thread::sleep(std::time::Duration::from_secs(30));
        return;
    }
    let output = std::env::var_os("FIXTURE_OUTPUT").expect("output environment");
    let expected_arg = argument == "--interactive";
    let json = std::fs::read(&context).expect("context contents");
    let report = format!(
        "{}|{}|{}|{}|{}|{}",
        String::from_utf8_lossy(&json).contains("\"version\""),
        expected_arg,
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
        std::path::Path::new(&context).display(),
    );
    std::fs::write(output, report).expect("fixture report");
}
"#,
        )
        .unwrap();
        let status = std::process::Command::new("rustc")
            .args(["--edition=2024", "-o"])
            .arg(&executable)
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        (directory, executable)
    }

    #[tokio::test]
    async fn real_runner_inherits_streams_and_delivers_static_process_settings() {
        let (_fixture_directory, executable) = compile_terminal_fixture();
        let output_directory = tempfile::tempdir().unwrap();
        let output = output_directory.path().join("report");
        let mut invocation = invocation();
        invocation.program = executable;
        invocation.env.insert(
            OsString::from("FIXTURE_OUTPUT"),
            output.clone().into_os_string(),
        );
        let mut transition = FakeTransition::default();

        execute_terminal_invocation_with(
            &mut transition,
            &invocation,
            &mut SystemTerminalProcessRunner,
        )
        .await
        .unwrap();

        let report = std::fs::read_to_string(output).unwrap();
        let fields = report.split('|').collect::<Vec<_>>();
        assert_eq!(fields[..2], ["true", "true"]);
        assert_eq!(fields[2], std::io::stdin().is_terminal().to_string());
        assert_eq!(fields[3], std::io::stdout().is_terminal().to_string());
        assert_eq!(fields[4], std::io::stderr().is_terminal().to_string());
        assert!(!Path::new(fields[5]).exists());
        assert_eq!(transition.suspend_count, 1);
        assert_eq!(transition.restore_count, 1);
    }

    #[tokio::test]
    async fn configured_timeout_restores_terminal_and_cleans_context() {
        let (_fixture_directory, executable) = compile_terminal_fixture();
        let mut invocation = invocation();
        invocation.program = executable;
        invocation.args = vec!["--sleep".into()];
        invocation.timeout = Some(Duration::from_millis(10));
        let mut transition = FakeTransition::default();

        let error = execute_terminal_invocation_with(
            &mut transition,
            &invocation,
            &mut SystemTerminalProcessRunner,
        )
        .await
        .unwrap_err();

        assert_eq!(error, "custom command :agent timed out");
        assert_eq!(transition.suspend_count, 1);
        assert_eq!(transition.restore_count, 1);
    }

    #[test]
    fn configured_timeout_is_distinct_from_unlimited_terminal_default() {
        let mut invocation = invocation();
        assert_eq!(invocation.timeout, None);
        invocation.timeout = Some(Duration::from_secs(30));
        assert_eq!(invocation.timeout, Some(Duration::from_secs(30)));
    }
}
