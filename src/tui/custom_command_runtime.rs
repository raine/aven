use std::io;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio::task::{JoinError, JoinHandle};

use crate::config::{CustomTuiCommandExecution, CustomTuiCommandSuccess};
use crate::tui::custom_command::CustomCommandInvocation;

const OUTPUT_LIMIT: usize = 16 * 1024;
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);
const BACKGROUND_INPUT_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINATION_GRACE: Duration = Duration::from_millis(500);
const IO_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CustomCommandInvocationId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InvocationPhase {
    Starting,
    DeliveringInput,
    Running,
    Canceling,
}

#[derive(Clone, Debug)]
struct CancellationHandle {
    sender: watch::Sender<bool>,
}

impl CancellationHandle {
    fn cancel(&self) {
        self.sender.send_replace(true);
    }
}

struct PendingInvocation {
    id: CustomCommandInvocationId,
    name: String,
    started_at: Instant,
    phase: InvocationPhase,
    phase_updates: watch::Receiver<InvocationPhase>,
    task: JoinHandle<CustomCommandCompletion>,
    cancel: CancellationHandle,
    process: ProcessIdentity,
}

#[derive(Debug)]
pub(crate) struct CustomCommandCompletion {
    pub(crate) id: CustomCommandInvocationId,
    pub(crate) name: String,
    pub(crate) on_success: CustomTuiCommandSuccess,
    pub(crate) result: Result<(), String>,
}

pub(crate) struct CustomCommandController {
    next_id: u64,
    pending: Vec<PendingInvocation>,
}

impl Default for CustomCommandController {
    fn default() -> Self {
        Self {
            next_id: 1,
            pending: Vec::new(),
        }
    }
}

impl Drop for CustomCommandController {
    fn drop(&mut self) {
        for pending in &self.pending {
            if pending.task.is_finished() {
                continue;
            }
            pending.cancel.cancel();
            terminate_process_tree_now(pending.process);
            pending.task.abort();
        }
    }
}

impl CustomCommandController {
    pub(crate) fn launch(
        &mut self,
        invocation: CustomCommandInvocation,
    ) -> Result<CustomCommandInvocationId> {
        self.launch_with_timeouts(invocation, WAIT_TIMEOUT, BACKGROUND_INPUT_TIMEOUT)
    }

    fn launch_with_timeouts(
        &mut self,
        invocation: CustomCommandInvocation,
        wait_timeout: Duration,
        background_input_timeout: Duration,
    ) -> Result<CustomCommandInvocationId> {
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
        command.process_group(0);
        command.kill_on_drop(invocation.execution == CustomTuiCommandExecution::Wait);
        let child = command.spawn().with_context(|| {
            format!(
                "could not start custom command {} ({})",
                invocation.name,
                invocation.program.display()
            )
        })?;
        let process = ProcessIdentity::from_child(&child)
            .context("custom command process identity unavailable after spawn")?;
        let id = CustomCommandInvocationId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        let (phase_sender, phase_updates) = watch::channel(InvocationPhase::Starting);
        let name = invocation.name.clone();
        let started_at = Instant::now();
        let task = match invocation.execution {
            CustomTuiCommandExecution::Background => tokio::spawn(run_background(
                id,
                invocation,
                child,
                process,
                background_input_timeout,
                cancel_receiver,
                phase_sender,
            )),
            CustomTuiCommandExecution::Wait => tokio::spawn(run_waiting(
                id,
                invocation,
                child,
                process,
                wait_timeout,
                cancel_receiver,
                phase_sender,
            )),
        };
        self.pending.push(PendingInvocation {
            id,
            name,
            started_at,
            phase: InvocationPhase::Starting,
            phase_updates,
            task,
            cancel: CancellationHandle {
                sender: cancel_sender,
            },
            process,
        });
        Ok(id)
    }

    pub(crate) fn work_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) async fn poll(&mut self) -> Vec<CustomCommandCompletion> {
        for pending in &mut self.pending {
            pending.phase = *pending.phase_updates.borrow_and_update();
        }
        let mut completed = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            if !self.pending[index].task.is_finished() {
                index += 1;
                continue;
            }
            let pending = self.pending.swap_remove(index);
            completed.push(join_completion(pending).await);
        }
        completed
    }

    pub(crate) async fn shutdown(&mut self) {
        for pending in &self.pending {
            if !pending.task.is_finished() {
                pending.cancel.cancel();
            }
        }
        while let Some(pending) = self.pending.pop() {
            let _ = join_completion(pending).await;
        }
    }
}

async fn join_completion(mut pending: PendingInvocation) -> CustomCommandCompletion {
    pending.phase = *pending.phase_updates.borrow_and_update();
    match pending.task.await {
        Ok(completion) => {
            debug_assert_eq!(completion.id, pending.id);
            completion
        }
        Err(error) => CustomCommandCompletion {
            id: pending.id,
            name: pending.name,
            on_success: CustomTuiCommandSuccess::Stay,
            result: Err(format!(
                "custom command worker failed after {:?}: {error}",
                pending.started_at.elapsed()
            )),
        },
    }
}

async fn run_background(
    id: CustomCommandInvocationId,
    invocation: CustomCommandInvocation,
    mut child: Child,
    process: ProcessIdentity,
    input_timeout: Duration,
    mut cancel: watch::Receiver<bool>,
    phase: watch::Sender<InvocationPhase>,
) -> CustomCommandCompletion {
    phase.send_replace(InvocationPhase::DeliveringInput);
    let stdin = child.stdin.take();
    let input = invocation.stdin_json;
    let writer = async move {
        let stdin = stdin.ok_or_else(stdin_unavailable)?;
        write_input(stdin, &input).await
    };
    let deadline = tokio::time::Instant::now() + input_timeout;
    let result = tokio::select! {
        biased;
        result = tokio::time::timeout_at(deadline, writer) => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                phase.send_replace(InvocationPhase::Canceling);
                let cleanup = terminate_and_reap(&mut child, process).await.err();
                Err(with_cleanup(
                    format!("could not send input to :{}: {error}", invocation.name),
                    cleanup.as_ref(),
                ))
            }
            Err(_) => {
                phase.send_replace(InvocationPhase::Canceling);
                let cleanup = terminate_and_reap(&mut child, process).await.err();
                Err(with_cleanup(
                    format!("could not send input to :{}: input delivery timed out", invocation.name),
                    cleanup.as_ref(),
                ))
            }
        },
        _ = canceled(&mut cancel) => {
            phase.send_replace(InvocationPhase::Canceling);
            let cleanup = terminate_and_reap(&mut child, process).await.err();
            Err(with_cleanup(
                format!("custom command :{} was canceled", invocation.name),
                cleanup.as_ref(),
            ))
        }
    };
    CustomCommandCompletion {
        id,
        name: invocation.name,
        on_success: CustomTuiCommandSuccess::Stay,
        result,
    }
}

async fn run_waiting(
    id: CustomCommandInvocationId,
    invocation: CustomCommandInvocation,
    mut child: Child,
    process: ProcessIdentity,
    operation_timeout: Duration,
    mut cancel: watch::Receiver<bool>,
    phase: watch::Sender<InvocationPhase>,
) -> CustomCommandCompletion {
    let name = invocation.name.clone();
    let result = supervise_waiting(
        &name,
        &mut child,
        process,
        invocation.stdin_json,
        operation_timeout,
        &mut cancel,
        &phase,
    )
    .await;
    CustomCommandCompletion {
        id,
        name: invocation.name,
        on_success: invocation.on_success,
        result,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StopReason {
    Timeout,
    Canceled,
    InputFailure,
}

async fn supervise_waiting(
    name: &str,
    child: &mut Child,
    process: ProcessIdentity,
    input: Vec<u8>,
    operation_timeout: Duration,
    cancel: &mut watch::Receiver<bool>,
    phase: &watch::Sender<InvocationPhase>,
) -> Result<(), String> {
    let stdin = child.stdin.take().ok_or_else(stdin_unavailable);
    let stdout = child
        .stdout
        .take()
        .context("custom command stdout unavailable");
    let stderr = child
        .stderr
        .take()
        .context("custom command stderr unavailable");
    let (stdin, stdout, stderr) = match (stdin, stdout, stderr) {
        (Ok(stdin), Ok(stdout), Ok(stderr)) => (stdin, stdout, stderr),
        (stdin, stdout, stderr) => {
            phase.send_replace(InvocationPhase::Canceling);
            let cleanup = terminate_and_reap(child, process).await.err();
            let error = stdin
                .err()
                .map(anyhow::Error::from)
                .or_else(|| stdout.err())
                .or_else(|| stderr.err())
                .expect("one command pipe is unavailable");
            return Err(with_cleanup(format!("{error:#}"), cleanup.as_ref()));
        }
    };

    phase.send_replace(InvocationPhase::DeliveringInput);
    let mut stdin_task = Some(tokio::spawn(
        async move { write_input(stdin, &input).await },
    ));
    let mut stdout_task = Some(tokio::spawn(read_bounded(stdout)));
    let mut stderr_task = Some(tokio::spawn(read_bounded(stderr)));
    let mut stdin_result = None;
    let mut stdout_result = None;
    let mut stderr_result = None;
    let mut status_result = None;
    let deadline = tokio::time::Instant::now() + operation_timeout;
    let deadline_sleep = tokio::time::sleep_until(deadline);
    tokio::pin!(deadline_sleep);
    let mut stop_reason = None;

    while stdin_result.is_none()
        || stdout_result.is_none()
        || stderr_result.is_none()
        || status_result.is_none()
    {
        tokio::select! {
            _ = &mut deadline_sleep => {
                stop_reason = Some(StopReason::Timeout);
                break;
            }
            _ = canceled(cancel) => {
                stop_reason = Some(StopReason::Canceled);
                break;
            }
            result = stdin_task.as_mut().expect("stdin task exists"), if stdin_result.is_none() => {
                let failed = !matches!(&result, Ok(Ok(())));
                stdin_result = Some(result);
                if failed {
                    stop_reason = Some(StopReason::InputFailure);
                    break;
                }
                phase.send_replace(InvocationPhase::Running);
            }
            result = stdout_task.as_mut().expect("stdout task exists"), if stdout_result.is_none() => {
                stdout_result = Some(result);
            }
            result = stderr_task.as_mut().expect("stderr task exists"), if stderr_result.is_none() => {
                stderr_result = Some(result);
            }
            result = child.wait(), if status_result.is_none() => {
                status_result = Some(result);
            }
        }
    }

    let cleanup = if stop_reason.is_some() {
        phase.send_replace(InvocationPhase::Canceling);
        terminate_and_reap(child, process).await.err()
    } else {
        None
    };

    collect_task(&mut stdin_task, &mut stdin_result).await;
    collect_task(&mut stdout_task, &mut stdout_result).await;
    collect_task(&mut stderr_task, &mut stderr_result).await;
    if status_result.is_none() {
        status_result = Some(child.wait().await);
    }

    let stdin_result = flatten_task(stdin_result.expect("stdin result collected"));
    let stdout_result = flatten_task(stdout_result.expect("stdout result collected"));
    let stderr_result = flatten_task(stderr_result.expect("stderr result collected"));
    let status_result = status_result.expect("status result collected");

    if let Err(error) = stdin_result
        && stop_reason != Some(StopReason::Timeout)
        && stop_reason != Some(StopReason::Canceled)
    {
        return Err(with_cleanup(
            format!("could not send input to :{name}: {error}"),
            cleanup.as_ref(),
        ));
    }
    if stop_reason.is_none()
        && let Ok(status) = &status_result
        && !status.success()
    {
        let error = if let Some(code) = status.code() {
            format!("custom command :{name} exited with status {code}")
        } else {
            format!("custom command :{name} terminated without an exit code")
        };
        return Err(with_cleanup(error, cleanup.as_ref()));
    }
    if stop_reason == Some(StopReason::Timeout) {
        return Err(with_cleanup(
            format!("custom command :{name} timed out"),
            cleanup.as_ref(),
        ));
    }
    if stop_reason == Some(StopReason::Canceled) {
        return Err(with_cleanup(
            format!("custom command :{name} was canceled"),
            cleanup.as_ref(),
        ));
    }
    if let Err(error) = status_result {
        return Err(with_cleanup(
            format!("could not wait for custom command: {error}"),
            cleanup.as_ref(),
        ));
    }
    let stdout_truncated = stdout_result.map_err(|error| {
        with_cleanup(
            format!("custom command stdout read failed: {error}"),
            cleanup.as_ref(),
        )
    })?;
    let stderr_truncated = stderr_result.map_err(|error| {
        with_cleanup(
            format!("custom command stderr read failed: {error}"),
            cleanup.as_ref(),
        )
    })?;
    if stdout_truncated || stderr_truncated {
        return Err(with_cleanup(
            format!("custom command :{name} output exceeded {OUTPUT_LIMIT} bytes"),
            cleanup.as_ref(),
        ));
    }
    Ok(())
}

async fn collect_task<T>(
    task: &mut Option<JoinHandle<io::Result<T>>>,
    result: &mut Option<Result<io::Result<T>, JoinError>>,
) {
    if result.is_some() {
        task.take();
        return;
    }
    let Some(mut task_handle) = task.take() else {
        return;
    };
    match tokio::time::timeout(IO_CLEANUP_TIMEOUT, &mut task_handle).await {
        Ok(task_result) => *result = Some(task_result),
        Err(_) => {
            task_handle.abort();
            *result = Some(task_handle.await);
        }
    }
}

fn flatten_task<T>(result: Result<io::Result<T>, JoinError>) -> io::Result<T> {
    result.map_err(io::Error::other)?
}

async fn canceled(receiver: &mut watch::Receiver<bool>) {
    if *receiver.borrow() {
        return;
    }
    while receiver.changed().await.is_ok() {
        if *receiver.borrow_and_update() {
            return;
        }
    }
}

async fn write_input(mut stdin: impl AsyncWrite + Unpin, input: &[u8]) -> io::Result<()> {
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

fn stdin_unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "custom command stdin unavailable",
    )
}

fn with_cleanup(message: String, cleanup: Option<&anyhow::Error>) -> String {
    match cleanup {
        Some(error) => format!("{message}; cleanup failed: {error:#}"),
        None => message,
    }
}

#[derive(Clone, Copy, Debug)]
struct ProcessIdentity {
    pid: u32,
}

impl ProcessIdentity {
    fn from_child(child: &Child) -> Result<Self> {
        let pid = child.id().context("custom command PID unavailable")?;
        Ok(Self { pid })
    }
}

#[cfg(unix)]
async fn terminate_and_reap(child: &mut Child, process: ProcessIdentity) -> Result<()> {
    signal_process_group(process, libc::SIGTERM)
        .context("terminate custom command process group")?;
    tokio::time::sleep(TERMINATION_GRACE).await;
    signal_process_group(process, libc::SIGKILL).context("kill custom command process group")?;
    child
        .wait()
        .await
        .context("reap custom command after forced termination")?;
    Ok(())
}

#[cfg(not(unix))]
async fn terminate_and_reap(child: &mut Child, _process: ProcessIdentity) -> Result<()> {
    child
        .start_kill()
        .context("terminate custom command process")?;
    child
        .wait()
        .await
        .context("reap custom command after termination")?;
    Ok(())
}

#[cfg(unix)]
fn signal_process_group(process: ProcessIdentity, signal: libc::c_int) -> io::Result<()> {
    let process_group = i32::try_from(process.pid)
        .map_err(|_| io::Error::other("custom command PID exceeds platform process ID range"))?;
    // Aven assigns the child PID as its process group ID before the child starts.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn terminate_process_tree_now(process: ProcessIdentity) {
    let _ = signal_process_group(process, libc::SIGKILL);
}

#[cfg(not(unix))]
fn terminate_process_tree_now(_process: ProcessIdentity) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    static PROCESS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn invocation(
        program: PathBuf,
        args: Vec<String>,
        input: Vec<u8>,
        execution: CustomTuiCommandExecution,
        on_success: CustomTuiCommandSuccess,
    ) -> CustomCommandInvocation {
        CustomCommandInvocation {
            name: "dispatch".to_string(),
            program,
            args,
            cwd: std::env::current_dir().unwrap(),
            stdin_json: input,
            execution,
            on_success,
        }
    }

    fn fixture_invocation(
        fixture: &Path,
        mode: &str,
        input: Vec<u8>,
        execution: CustomTuiCommandExecution,
    ) -> CustomCommandInvocation {
        invocation(
            fixture.to_path_buf(),
            vec![mode.to_string()],
            input,
            execution,
            CustomTuiCommandSuccess::Stay,
        )
    }

    fn compile_fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/custom_command_process.rs");
        let executable = dir.path().join("custom-command-process");
        let status = std::process::Command::new("rustc")
            .args(["--edition=2024", "-o"])
            .arg(&executable)
            .arg(source)
            .status()
            .unwrap();
        assert!(status.success());
        (dir, executable)
    }

    fn process_is_running(pid: i32) -> bool {
        #[cfg(target_os = "linux")]
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            return stat
                .rsplit_once(") ")
                .and_then(|(_, fields)| fields.chars().next())
                .is_some_and(|state| state != 'Z');
        }
        (unsafe { libc::kill(pid, 0) }) == 0
    }

    async fn assert_process_stopped(pid: i32) {
        for _ in 0..100 {
            if !process_is_running(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("descendant {pid} survived cleanup");
    }

    async fn completion(controller: &mut CustomCommandController) -> CustomCommandCompletion {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut completed = controller.poll().await;
                if let Some(completion) = completed.pop() {
                    return completion;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("custom command completion timed out")
    }

    #[tokio::test]
    async fn waiting_command_drains_output_while_delivering_input() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let input = vec![b'i'; 256 * 1024];
        let mut controller = CustomCommandController::default();
        controller
            .launch_with_timeouts(
                fixture_invocation(
                    &fixture,
                    "write-before-read",
                    input,
                    CustomTuiCommandExecution::Wait,
                ),
                Duration::from_secs(2),
                Duration::from_secs(1),
            )
            .unwrap();

        let result = completion(&mut controller).await.result.unwrap_err();
        assert!(result.contains("output exceeded"), "{result}");
    }

    #[tokio::test]
    async fn waiting_timeout_covers_blocked_input_delivery() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let mut controller = CustomCommandController::default();
        let started = Instant::now();
        controller
            .launch_with_timeouts(
                fixture_invocation(
                    &fixture,
                    "never-read",
                    vec![b'i'; 256 * 1024],
                    CustomTuiCommandExecution::Wait,
                ),
                Duration::from_millis(100),
                Duration::from_secs(1),
            )
            .unwrap();

        let result = completion(&mut controller).await.result.unwrap_err();
        assert!(result.contains("timed out"), "{result}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn waiting_command_reports_closed_stdin_as_input_failure() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let mut controller = CustomCommandController::default();
        controller
            .launch_with_timeouts(
                fixture_invocation(
                    &fixture,
                    "close-stdin",
                    vec![b'i'; 256 * 1024],
                    CustomTuiCommandExecution::Wait,
                ),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .unwrap();

        let result = completion(&mut controller).await.result.unwrap_err();
        assert!(result.contains("could not send input"), "{result}");
    }

    #[tokio::test]
    async fn timeout_terminates_descendants_holding_output_open() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let state = tempfile::tempdir().unwrap();
        let pid_file = state.path().join("descendant.pid");
        let mut command = fixture_invocation(
            &fixture,
            "descendant-holds-stdout",
            Vec::new(),
            CustomTuiCommandExecution::Wait,
        );
        command.args.push(pid_file.to_string_lossy().into_owned());
        let mut controller = CustomCommandController::default();
        controller
            .launch_with_timeouts(command, Duration::from_secs(1), Duration::from_secs(1))
            .unwrap();

        let result = completion(&mut controller).await.result.unwrap_err();
        assert!(result.contains("timed out"), "{result}");
        let pid = std::fs::read_to_string(pid_file).unwrap();
        let pid = pid.trim().parse::<i32>().unwrap();
        assert_process_stopped(pid).await;
    }

    #[tokio::test]
    async fn shutdown_terminates_waiting_process_tree() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let state = tempfile::tempdir().unwrap();
        let pid_file = state.path().join("descendant.pid");
        let mut command = fixture_invocation(
            &fixture,
            "descendant-sleeps",
            Vec::new(),
            CustomTuiCommandExecution::Wait,
        );
        command.args.push(pid_file.to_string_lossy().into_owned());
        let mut controller = CustomCommandController::default();
        controller.launch(command).unwrap();
        for _ in 0..100 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        controller.shutdown().await;

        assert!(!controller.work_pending());
        let pid = std::fs::read_to_string(pid_file).unwrap();
        let pid = pid.trim().parse::<i32>().unwrap();
        assert_process_stopped(pid).await;
    }

    #[tokio::test]
    async fn background_input_delivery_is_bounded_and_cleans_up() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let mut controller = CustomCommandController::default();
        controller
            .launch_with_timeouts(
                fixture_invocation(
                    &fixture,
                    "never-read",
                    vec![b'i'; 256 * 1024],
                    CustomTuiCommandExecution::Background,
                ),
                Duration::from_secs(1),
                Duration::from_millis(100),
            )
            .unwrap();

        let result = completion(&mut controller).await.result.unwrap_err();
        assert!(result.contains("input delivery timed out"), "{result}");
    }

    #[tokio::test]
    async fn background_closed_stdin_reports_handoff_failure() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let mut controller = CustomCommandController::default();
        controller
            .launch_with_timeouts(
                fixture_invocation(
                    &fixture,
                    "close-stdin",
                    vec![b'i'; 2 * 1024 * 1024],
                    CustomTuiCommandExecution::Background,
                ),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .unwrap();

        let result = completion(&mut controller).await.result.unwrap_err();
        assert!(result.contains("could not send input"), "{result}");
        assert!(!controller.work_pending());
    }

    #[tokio::test]
    async fn successful_background_handoff_survives_controller_shutdown() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let state = tempfile::tempdir().unwrap();
        let pid_file = state.path().join("background.pid");
        let mut command = fixture_invocation(
            &fixture,
            "record-pid-and-sleep",
            Vec::new(),
            CustomTuiCommandExecution::Background,
        );
        command.args.push(pid_file.to_string_lossy().into_owned());
        let mut controller = CustomCommandController::default();
        controller.launch(command).unwrap();
        for _ in 0..100 {
            if controller.pending[0].task.is_finished() && pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        controller.shutdown().await;

        let pid = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        assert!(process_is_running(pid));
        signal_process_group(ProcessIdentity { pid: pid as u32 }, libc::SIGKILL).unwrap();
        assert_process_stopped(pid).await;
    }

    #[tokio::test]
    async fn background_command_reports_complete_input_handoff() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let state = tempfile::tempdir().unwrap();
        let output = state.path().join("background.json");
        let input = br#"{"version":1}"#.to_vec();
        let mut command = fixture_invocation(
            &fixture,
            "copy-stdin",
            input.clone(),
            CustomTuiCommandExecution::Background,
        );
        command.args.push(output.to_string_lossy().into_owned());
        let mut controller = CustomCommandController::default();
        controller.launch(command).unwrap();

        assert!(completion(&mut controller).await.result.is_ok());
        for _ in 0..100 {
            if output.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(std::fs::read(output).unwrap(), input);
    }

    #[tokio::test]
    async fn completions_preserve_invocation_identity_out_of_launch_order() {
        let _test_guard = PROCESS_TEST_LOCK.lock().await;
        let (_dir, fixture) = compile_fixture();
        let mut controller = CustomCommandController::default();
        let slow = controller
            .launch(fixture_invocation(
                &fixture,
                "sleep",
                b"200".to_vec(),
                CustomTuiCommandExecution::Wait,
            ))
            .unwrap();
        let fast = controller
            .launch(fixture_invocation(
                &fixture,
                "copy-stdin-null",
                Vec::new(),
                CustomTuiCommandExecution::Wait,
            ))
            .unwrap();

        let first = completion(&mut controller).await;
        let second = completion(&mut controller).await;
        assert_eq!(first.id, fast);
        assert_eq!(second.id, slow);
    }
}
