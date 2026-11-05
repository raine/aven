//! Waiting for the other processes of an installation before replacing it.

use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use anyhow::Result;
use aven_host::running_processes::blocking_processes;
use tokio::sync::watch;

use super::{UpdatePhase, UpdateProgress, ensure_not_cancelled};

const POLL: Duration = Duration::from_secs(1);

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
