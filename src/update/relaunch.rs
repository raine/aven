//! Finishing an update the app installed itself: the previous bundle stays
//! beside the new one until the new one has launched, and comes back if it
//! does not launch and the database is still the one the previous version
//! knew. A database the new version migrated is never handed back.

use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use semver::Version;
use serde::{Deserialize, Serialize};

use aven_host::running_processes::{ProcessKind, running_processes};

use super::bundle::{StagedBundle, rename_with};
use crate::app_bundle::{BUNDLE_EXECUTABLE, enclosing_bundle};

/// The directory beside the bundle that holds the previous bundle and the
/// record of the update.
const PREVIOUS_DIR: &str = ".aven-update-previous";
const MARKER: &str = "update.json";
const EXIT_LIMIT: Duration = Duration::from_secs(120);
/// How long the new app has to appear, and then to confirm its launch.
const APPEAR_LIMIT: Duration = Duration::from_secs(30);
const LAUNCH_LIMIT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PendingUpdate {
    pub(super) from: Version,
    pub(super) to: Version,
    pub(super) bundle: PathBuf,
    pub(super) database: Option<PathBuf>,
    /// The database's migrations before the update. Unknown never restores.
    pub(super) schema: Option<SchemaMark>,
    pub(super) state: UpdateState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum SchemaMark {
    Missing,
    Applied { latest: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum UpdateState {
    /// Swapped in, waiting for the new app to confirm its launch.
    Pending,
    /// The new app did not launch and the previous one is back.
    Restored,
    /// The new app did not launch after migrating the database, so it stays.
    Kept,
}

/// What a launch finds of an update that just finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchOutcome {
    Updated { from: Version, to: Version },
    Restored { failed: Version, restored: Version },
}

fn previous_dir(bundle: &Path) -> Result<PathBuf> {
    Ok(bundle
        .parent()
        .context("Aven.app has no parent directory")?
        .join(PREVIOUS_DIR))
}

fn previous_bundle(bundle: &Path) -> Result<PathBuf> {
    Ok(previous_dir(bundle)?.join(bundle.file_name().context("Aven.app has no name")?))
}

pub(super) fn marker_path(bundle: &Path) -> Result<PathBuf> {
    Ok(previous_dir(bundle)?.join(MARKER))
}

fn read_marker(path: &Path) -> Result<Option<PendingUpdate>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("parse update record")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

/// Whether `executable` is in a bundle this version was just swapped into and
/// whose app has not confirmed its launch yet. Until it does, the update can
/// still be undone, which needs the database as the previous version left it.
pub(crate) fn launch_pending(executable: &Path, current: &Version) -> bool {
    let Some(marker) = enclosing_bundle(executable).and_then(|bundle| marker_path(&bundle).ok())
    else {
        return false;
    };
    matches!(
        read_marker(&marker),
        Ok(Some(update)) if update.state == UpdateState::Pending && update.to == *current
    )
}

fn write_marker(path: &Path, update: &PendingUpdate) -> Result<()> {
    let parent = path.parent().context("update record has no directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).context("stage update record")?;
    serde_json::to_writer(&mut file, update).context("write update record")?;
    file.as_file().sync_all().context("sync update record")?;
    file.persist(path)
        .map_err(|error| error.error)
        .context("save update record")?;
    Ok(())
}

/// Removes the previous-bundle directory, which only this module creates.
fn remove_previous(bundle: &Path) -> Result<()> {
    let dir = previous_dir(bundle)?;
    match std::fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.is_dir() => {
            std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))
        }
        Ok(_) => bail!("{} is not a directory", dir.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", dir.display())),
    }
}

/// Swaps `staged` in and keeps the bundle it replaces, with a record of the
/// update, until the new app confirms its launch. Returns the record's path.
pub(super) fn activate_keeping_previous(
    staged: StagedBundle,
    update: PendingUpdate,
) -> Result<PathBuf> {
    let bundle = staged.bundle().to_path_buf();
    remove_previous(&bundle)?;
    let dir = previous_dir(&bundle)?;
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(&dir)
        .with_context(|| format!("create {}", dir.display()))?;
    let marker = marker_path(&bundle)?;
    if let Err(error) = write_marker(&marker, &update) {
        let _ = remove_previous(&bundle);
        return Err(error);
    }
    let staging = match staged.activate() {
        Ok(staging) => staging,
        Err(error) => {
            let _ = remove_previous(&bundle);
            return Err(error);
        }
    };
    let replaced = staging.path().join(crate::app_bundle::BUNDLE_NAME);
    let kept = previous_bundle(&bundle)?;
    if let Err(error) = rename_with(&replaced, &kept, libc::RENAME_EXCL) {
        // Without a kept copy there is nothing to come back to, so undo.
        let undone = rename_with(&replaced, &bundle, libc::RENAME_SWAP);
        let _ = remove_previous(&bundle);
        return Err(anyhow::Error::from(error).context(match undone {
            Ok(()) => "keep the previous Aven.app".to_string(),
            Err(undo) => format!("keep the previous Aven.app; restoring it also failed: {undo}"),
        }));
    }
    Ok(marker)
}

/// Puts the kept bundle back in place of the new one, before anything ran it.
pub(super) fn restore_previous(bundle: &Path) -> Result<()> {
    rename_with(&previous_bundle(bundle)?, bundle, libc::RENAME_SWAP)
        .context("restore the previous Aven.app")?;
    remove_previous(bundle)
}

/// Starts the previous version's executable to relaunch the app once `pid`
/// exits, in its own session so the app's exit does not take it along.
pub(super) fn spawn_finisher(marker: &Path, bundle: &Path, pid: u32) -> Result<()> {
    let executable = previous_bundle(bundle)?.join(BUNDLE_EXECUTABLE);
    let log = std::fs::File::create(previous_dir(bundle)?.join("finish.log"))
        .context("create update log")?;
    let mut command = std::process::Command::new(executable);
    command
        .args(["internal", "finish-desktop-update", "--marker"])
        .arg(marker)
        .arg("--pid")
        .arg(pid.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().context("open update log")?)
        .stderr(log);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn().context("start the relaunch")?;
    Ok(())
}

/// Runs in the previous version after the app quits: opens the new app and
/// waits for it to confirm its launch, restoring the previous app if it does
/// not and the database is unchanged.
pub(crate) async fn finish_desktop_update(marker: &Path, pid: u32) -> Result<()> {
    wait_for_exit(pid).await?;
    let update = read_marker(marker)?.context("the update record is missing")?;
    if update.state != UpdateState::Pending {
        return Ok(());
    }
    let previous = previous_bundle(&update.bundle)?;
    if !super::processes::wait_for_daemon_exit(&previous.join(BUNDLE_EXECUTABLE)).await {
        println!("update-daemon-still-running");
    }
    match launch(marker, &update.bundle, open_app).await? {
        Launch::Confirmed => {
            println!("update-launched version={}", update.to);
            Ok(())
        }
        Launch::StillStarting => {
            println!("update-launch-unconfirmed version={}", update.to);
            Ok(())
        }
        Launch::Failed => recover(marker, update, true).await,
    }
}

async fn wait_for_exit(pid: u32) -> Result<()> {
    let started = Instant::now();
    while process_alive(pid) {
        if started.elapsed() > EXIT_LIMIT {
            bail!("the app did not quit");
        }
        tokio::time::sleep(POLL).await;
    }
    Ok(())
}

fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Opens the new app and watches it start. An app Launch Services refuses to
/// open has failed like one that quits at once.
async fn launch(
    marker: &Path,
    bundle: &Path,
    open: impl FnOnce(&Path) -> Result<()>,
) -> Result<Launch> {
    if let Err(error) = open(bundle) {
        println!("update-open-failed error={error:#}");
        return Ok(Launch::Failed);
    }
    watch_launch(marker, bundle).await
}

#[derive(Debug, PartialEq, Eq)]
enum Launch {
    Confirmed,
    StillStarting,
    Failed,
}

async fn watch_launch(marker: &Path, bundle: &Path) -> Result<Launch> {
    let executable = bundle.join(BUNDLE_EXECUTABLE);
    let started = Instant::now();
    let mut seen = false;
    loop {
        if read_marker(marker)?.is_none() {
            return Ok(Launch::Confirmed);
        }
        let running = running_processes(&executable)
            .iter()
            .any(|process| process.kind == ProcessKind::App);
        if running {
            seen = true;
        } else if seen || started.elapsed() > APPEAR_LIMIT {
            // The marker may have gone between the two looks.
            if read_marker(marker)?.is_none() {
                return Ok(Launch::Confirmed);
            }
            return Ok(Launch::Failed);
        }
        if started.elapsed() > LAUNCH_LIMIT {
            return Ok(Launch::StillStarting);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// `ui` opens the restored app or shows why nothing was restored.
async fn recover(marker: &Path, mut update: PendingUpdate, ui: bool) -> Result<()> {
    let now = match &update.database {
        Some(database) => schema_mark(database).await.ok(),
        None => None,
    };
    if update.schema.is_some() && now == update.schema {
        let previous = previous_bundle(&update.bundle)?;
        rename_with(&previous, &update.bundle, libc::RENAME_SWAP)
            .context("restore the previous Aven.app")?;
        update.state = UpdateState::Restored;
        write_marker(marker, &update)?;
        println!(
            "update-restored failed={} restored={}",
            update.to, update.from
        );
        return if ui { open_app(&update.bundle) } else { Ok(()) };
    }
    update.state = UpdateState::Kept;
    write_marker(marker, &update)?;
    println!("update-kept version={} schema-changed=true", update.to);
    if !ui {
        return Ok(());
    }
    alert(
        &format!("Aven {} didn\u{2019}t start", update.to),
        &format!(
            "It already upgraded your tasks database, so Aven {} can\u{2019}t be put back. \
             Download Aven {} again and replace Aven in Applications. Your tasks are kept.",
            update.from, update.to
        ),
    );
    Ok(())
}

/// The database's latest applied migration, read from a private snapshot so
/// nothing in the user's files changes.
pub(super) async fn schema_mark(database: &Path) -> Result<SchemaMark> {
    if !database.exists() {
        return Ok(SchemaMark::Missing);
    }
    let inspected = crate::db::Database::inspect(database).await;
    if let Some(error) = inspected.inspection.open_error {
        bail!("inspect {}: {error}", database.display());
    }
    let latest = inspected
        .inspection
        .schema_version
        .context("the database has no schema version")?;
    Ok(SchemaMark::Applied { latest })
}

/// Opens the app through Launch Services. It starts with the user session's
/// environment, so the settings that point Aven at its data come along.
fn open_app(bundle: &Path) -> Result<()> {
    let status = std::process::Command::new("/usr/bin/open")
        .args(aven_host::app_bundle::open_environment_args())
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .status()
        .context("run open")?;
    if !status.success() {
        bail!("open {} failed: {status}", bundle.display());
    }
    Ok(())
}

fn alert(title: &str, message: &str) {
    let shown = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display alert (item 1 of argv) message (item 2 of argv) as critical",
            "-e",
            "end run",
            title,
            message,
        ])
        .stdin(std::process::Stdio::null())
        .status();
    if let Err(error) = shown {
        eprintln!("could not show the alert: {error}");
    }
}

/// Called by the app once it is up: confirms an update it was launched for and
/// removes the previous bundle, or reports one that was rolled back.
pub(crate) fn confirm_launch(bundle: &Path, current: &Version) -> Result<Option<LaunchOutcome>> {
    let marker = marker_path(bundle)?;
    let Some(update) = read_marker(&marker)? else {
        return Ok(None);
    };
    let outcome = match update.state {
        UpdateState::Pending | UpdateState::Kept if update.to == *current => {
            LaunchOutcome::Updated {
                from: update.from,
                to: update.to,
            }
        }
        UpdateState::Restored if update.from == *current => LaunchOutcome::Restored {
            failed: update.to,
            restored: update.from,
        },
        _ => return Ok(None),
    };
    remove_previous(bundle)?;
    Ok(Some(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bundle(bundle: &Path, body: &[u8]) {
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join(BUNDLE_EXECUTABLE), body).unwrap();
    }

    fn update(bundle: &Path, state: UpdateState) -> PendingUpdate {
        PendingUpdate {
            from: Version::new(1, 2, 0),
            to: Version::new(1, 3, 0),
            bundle: bundle.to_path_buf(),
            database: None,
            schema: Some(SchemaMark::Missing),
            state,
        }
    }

    #[test]
    fn the_new_app_confirms_its_launch_and_drops_the_previous_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("Aven.app");
        write_bundle(&bundle, b"new");
        write_bundle(&previous_bundle(&bundle).unwrap(), b"old");
        let marker = marker_path(&bundle).unwrap();
        write_marker(&marker, &update(&bundle, UpdateState::Pending)).unwrap();

        assert_eq!(
            confirm_launch(&bundle, &Version::new(1, 2, 0)).unwrap(),
            None,
            "another version leaves the record alone"
        );
        assert!(marker.exists());
        assert_eq!(
            confirm_launch(&bundle, &Version::new(1, 3, 0)).unwrap(),
            Some(LaunchOutcome::Updated {
                from: Version::new(1, 2, 0),
                to: Version::new(1, 3, 0),
            })
        );
        assert!(!previous_dir(&bundle).unwrap().exists());
        assert_eq!(
            std::fs::read(bundle.join(BUNDLE_EXECUTABLE)).unwrap(),
            b"new"
        );
        assert_eq!(
            confirm_launch(&bundle, &Version::new(1, 3, 0)).unwrap(),
            None
        );
    }

    #[test]
    fn a_restored_app_reports_the_version_that_failed() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("Aven.app");
        write_bundle(&bundle, b"old");
        write_bundle(&previous_bundle(&bundle).unwrap(), b"failed new");
        let marker = marker_path(&bundle).unwrap();
        write_marker(&marker, &update(&bundle, UpdateState::Restored)).unwrap();

        assert_eq!(
            confirm_launch(&bundle, &Version::new(1, 2, 0)).unwrap(),
            Some(LaunchOutcome::Restored {
                failed: Version::new(1, 3, 0),
                restored: Version::new(1, 2, 0),
            })
        );
        assert!(!previous_dir(&bundle).unwrap().exists());
    }

    #[test]
    fn only_the_version_awaiting_its_launch_is_pending() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("Aven.app");
        write_bundle(&bundle, b"new");
        let executable = bundle.join(BUNDLE_EXECUTABLE);
        let (old, new) = (Version::new(1, 2, 0), Version::new(1, 3, 0));
        assert!(!launch_pending(&executable, &new), "no update under way");

        std::fs::create_dir(previous_dir(&bundle).unwrap()).unwrap();
        let marker = marker_path(&bundle).unwrap();
        write_marker(&marker, &update(&bundle, UpdateState::Pending)).unwrap();
        assert!(launch_pending(&executable, &new));
        assert!(!launch_pending(&executable, &old));
        assert!(!launch_pending(Path::new("/usr/local/bin/aven"), &new));

        write_marker(&marker, &update(&bundle, UpdateState::Restored)).unwrap();
        assert!(!launch_pending(&executable, &new));
    }

    #[tokio::test]
    async fn an_app_that_will_not_open_has_failed() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("Aven.app");
        write_bundle(&bundle, b"new");
        let marker = marker_path(&bundle).unwrap();

        let launched = launch(&marker, &bundle, |_| bail!("open failed")).await;

        assert_eq!(launched.unwrap(), Launch::Failed);
    }

    #[tokio::test]
    async fn a_failed_launch_restores_only_an_unchanged_database() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("Aven.app");
        let database = temp.path().join("db.sqlite");
        let unopened = schema_mark(&database).await.unwrap();
        assert_eq!(unopened, SchemaMark::Missing);
        crate::db::Database::open(&database).await.unwrap();
        let migrated = schema_mark(&database).await.unwrap();
        assert!(matches!(migrated, SchemaMark::Applied { latest } if latest > 0));

        for (before, restored) in [(migrated, true), (unopened, false)] {
            write_bundle(&bundle, b"new");
            write_bundle(&previous_bundle(&bundle).unwrap(), b"old");
            let marker = marker_path(&bundle).unwrap();
            let mut pending = update(&bundle, UpdateState::Pending);
            pending.database = Some(database.clone());
            pending.schema = Some(before);
            write_marker(&marker, &pending).unwrap();
            recover(&marker, pending, false).await.unwrap();

            let state = read_marker(&marker).unwrap().unwrap().state;
            let running = std::fs::read(bundle.join(BUNDLE_EXECUTABLE)).unwrap();
            if restored {
                assert_eq!(state, UpdateState::Restored);
                assert_eq!(running, b"old");
            } else {
                assert_eq!(state, UpdateState::Kept, "a migrated database stays new");
                assert_eq!(running, b"new");
            }
            remove_previous(&bundle).unwrap();
            std::fs::remove_dir_all(&bundle).unwrap();
        }
    }
}
