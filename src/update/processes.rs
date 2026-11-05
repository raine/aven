//! Waiting for the other processes of an installation before replacing it.

use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use anyhow::Result;
use aven_host::running_processes::{ProcessKind, blocking_processes, running_processes};
use tokio::sync::watch;

use super::{UpdatePhase, UpdateProgress, ensure_not_cancelled};

const POLL: Duration = Duration::from_secs(1);
const DAEMON_POLL: Duration = Duration::from_millis(250);
/// Longer than the daemon takes to notice a replaced executable and finish a
/// sync round in progress.
const DAEMON_LIMIT: Duration = Duration::from_secs(30);

/// Waits until no process other than this one and the daemon runs
/// `executable`, showing the ones that do. It never stops them.
pub(super) async fn wait_for_exit(
    executable: &Path,
    progress: &watch::Sender<UpdateProgress>,
    cancelled: &Arc<AtomicBool>,
) -> Result<()> {
    loop {
        ensure_not_cancelled(cancelled)?;
        let blockers = blocking_processes(executable);
        if blockers.is_empty() {
            return Ok(());
        }
        progress.send_if_modified(|progress| {
            let changed = progress.phase != UpdatePhase::Waiting || progress.blockers != blockers;
            progress.phase = UpdatePhase::Waiting;
            progress.blockers = blockers;
            changed
        });
        tokio::time::sleep(POLL).await;
    }
}

/// Waits for daemons still running a replaced executable, now at
/// `executable`, to notice the replacement and exit, so none of them writes
/// to the database after the new version migrates it. Returns whether they
/// did before the limit; a daemon still busy is left to exit by itself.
pub(super) async fn wait_for_daemon_exit(executable: &Path) -> bool {
    let exited = wait_until(DAEMON_LIMIT, DAEMON_POLL, || {
        !running_processes(executable)
            .iter()
            .any(|process| process.kind == ProcessKind::Daemon)
    })
    .await;
    if !exited {
        tracing::warn!(executable = %executable.display(), "the previous daemon is still running");
    }
    exited
}

async fn wait_until(limit: Duration, poll: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while !done() {
        if started.elapsed() > limit {
            return false;
        }
        tokio::time::sleep(poll).await;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn waiting_ends_when_the_daemon_is_gone() {
        let mut looks = 0;
        let exited = wait_until(Duration::from_secs(5), Duration::from_millis(1), || {
            looks += 1;
            looks == 3
        })
        .await;
        assert!(exited);
        assert_eq!(looks, 3);
    }

    #[tokio::test]
    async fn waiting_gives_up_on_a_daemon_that_stays() {
        let exited = wait_until(Duration::from_millis(20), Duration::from_millis(1), || {
            false
        })
        .await;
        assert!(!exited);
    }

    #[tokio::test]
    async fn nothing_to_wait_for_without_a_daemon() {
        let temp = tempfile::tempdir().unwrap();
        assert!(wait_for_daemon_exit(&temp.path().join("aven")).await);
    }
}
