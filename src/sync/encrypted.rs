//! End-to-end encrypted sync commands over the isolated encrypted transports.
//!
//! The engine lives in `aven_core::sync::client::engine`; this module supplies
//! the desktop host (configuration policy, protected storage, blob paths and
//! the computer name), drives engine sessions over HTTP, and prompts and
//! prints for the CLI.
use std::io::{IsTerminal, Read, Write};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use aven_core::db::Database;
use aven_core::sync::client::engine;
pub(crate) use aven_core::sync::client::engine::{
    Admission, AssociationStatus, Cancellation, DaemonRound, InvitationStatus, LocalPhase, Outcome,
    PendingInvitation, SetupPreview, Stage, StatusReport, local_phase, unix_now,
};
use aven_core::sync::client::keys::{ProtectedStorage, StoreResult};
use aven_core::sync::client::{ClientHost, Step};
use zeroize::Zeroizing;

use crate::cli::{JoinArgs, SetupArgs};
use crate::config::{self, AppConfig};
use crate::render::print_json_pretty;
use crate::sync_http::HttpDriver;

mod devices;
#[cfg(test)]
pub(crate) use aven_core::sync::client::engine::ROUND_LIMIT;
#[cfg(test)]
pub(crate) use aven_core::sync::client::invitation::sample_invitations;
pub(super) use aven_core::sync::client::server_origin;
pub(crate) use aven_core::sync::client::{DeviceInvitation, InvitationCheck, SetupInvitation};
pub(crate) use devices::{
    Device, DeviceListing, Removal, finish_removal, list as list_devices, load_devices,
    remove as remove_device, remove_other_device,
};

#[cfg(test)]
mod tests;

const INPUT_LIMIT: u64 = 8192;

const INVITATION_CANCELLED: &str = "error sync-invitation-cancelled hint=\"another command cancelled this invitation; run `aven sync invite` again to add a device\"";

/// The desktop's answers to what the engine asks of its host.
pub(crate) struct DesktopHost<'a>(pub(crate) &'a AppConfig);

impl ClientHost for DesktopHost<'_> {
    fn ensure_sync_allowed(&self) -> Result<()> {
        self.0.ensure_sync_allowed()
    }

    fn protected_storage(&self) -> StoreResult<Arc<dyn ProtectedStorage>> {
        crate::protected_local_keys::storage()
    }

    fn blob_dir(&self, database: &Database) -> Result<PathBuf> {
        config::resolve_blob_dir(database.path(), self.0)
    }

    fn device_label(&self) -> Option<String> {
        super::device_label::automatic_label()
    }
}

fn driver() -> Result<HttpDriver> {
    HttpDriver::new()
}

pub(crate) async fn setup_preview(database: &Database, config: &AppConfig) -> Result<SetupPreview> {
    engine::setup_preview(database, &DesktopHost(config)).await
}

pub(crate) async fn ensure_setup_available(database: &Database, config: &AppConfig) -> Result<()> {
    engine::ensure_setup_available(database, &DesktopHost(config)).await
}

pub(crate) async fn run_setup(
    database: &Database,
    config: &AppConfig,
    invitation: &SetupInvitation,
    progress: &(dyn Fn(Stage) + Sync),
) -> Result<Outcome> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::run_setup(link, database, &host, invitation, progress))
        .await
}

pub(crate) async fn create_invitation(
    database: &Database,
    config: &AppConfig,
) -> Result<PendingInvitation> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::create_invitation(link, database, &host))
        .await
}

pub(crate) async fn association_status(
    database: &Database,
    config: &AppConfig,
) -> Result<AssociationStatus> {
    engine::association_status(database, &DesktopHost(config)).await
}

pub(crate) async fn invitation_status(
    database: &Database,
    config: &AppConfig,
) -> Result<Option<InvitationStatus>> {
    engine::invitation_status(database, &DesktopHost(config)).await
}

pub(crate) async fn cancel_invitation(
    database: &Database,
    config: &AppConfig,
) -> Result<Cancellation> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::cancel_invitation(link, database, &host))
        .await
}

pub(crate) async fn await_admission(
    database: &Database,
    config: &AppConfig,
    invitation: &PendingInvitation,
) -> Result<Admission> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::await_admission(link, &host, database, invitation))
        .await
}

/// Like [`await_admission`], but `None` reports an interrupt. The interrupt
/// is acted on only between polls, after each has released the coordination
/// lock.
async fn await_admission_until_interrupt(
    database: &Database,
    config: &AppConfig,
    invitation: &PendingInvitation,
) -> Result<Option<Admission>> {
    let host = DesktopHost(config);
    let driver = driver()?;
    let mut session = aven_core::sync::client::Session::new(|link| {
        engine::await_admission(link, &host, database, invitation)
    });
    let mut interrupt = tokio::spawn(tokio::signal::ctrl_c());
    let result = loop {
        match session.next().await {
            Ok(Step::Done(admission)) => break Ok(Some(admission)),
            Ok(Step::Wait(delay)) => tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = &mut interrupt => return Ok(None),
            },
            Ok(Step::Request(request)) => {
                if let Err(error) = driver.answer(&mut session, request).await {
                    break Err(error);
                }
            }
            Err(error) => break Err(error),
        }
    };
    interrupt.abort();
    result
}

/// QR presentation of the invitation text.
pub(crate) fn presentation(
    invitation: &PendingInvitation,
    glyphs: crate::pairing::QrGlyphs,
) -> Result<crate::pairing::PairingPresentation> {
    crate::pairing::PairingPresentation::new(invitation.server(), invitation.text(), glyphs)
}

pub(crate) fn tui_presentation(
    invitation: &PendingInvitation,
    glyphs: crate::pairing::QrGlyphs,
) -> Result<crate::pairing::PairingPresentation> {
    crate::pairing::PairingPresentation::new_tui(
        invitation.server(),
        invitation.text(),
        invitation.expires_at(),
        glyphs,
    )
}

pub(crate) async fn ensure_join_available(database: &Database, config: &AppConfig) -> Result<()> {
    engine::ensure_join_available(database, &DesktopHost(config)).await
}

pub(crate) async fn run_join(
    database: &Database,
    config: &AppConfig,
    invitation: impl FnOnce() -> Result<Option<DeviceInvitation>> + Send,
    replace: bool,
    progress: &(dyn Fn(Stage) + Sync),
) -> Result<(String, Outcome)> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::run_join(link, database, &host, invitation, replace, progress))
        .await
}

pub(crate) async fn run_to_completion(database: &Database, config: &AppConfig) -> Result<Outcome> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::run_to_completion(link, database, &host))
        .await
}

pub(crate) async fn daemon_round(
    database: &Database,
    config: &AppConfig,
    round_limit: usize,
) -> Result<DaemonRound> {
    let host = DesktopHost(config);
    driver()?
        .run(|link| engine::daemon_round(link, database, &host, round_limit))
        .await
}

pub(crate) async fn status_report(database: &Database, config: &AppConfig) -> Result<StatusReport> {
    engine::status_report(database, &DesktopHost(config)).await
}

/// Drains over a desktop tail client with the default configuration.
#[cfg(test)]
pub(crate) async fn drain(
    client: &crate::encrypted_tail_http::Client,
    store: &crate::protected_local_keys::ProtectedLocalKeyStore,
    database: &Database,
    blob_dir: &Path,
    round_limit: usize,
) -> Result<Outcome> {
    let config = AppConfig::default();
    let host = DesktopHost(&config);
    client
        .transport
        .driver
        .run(|link| async move {
            let client = aven_core::sync::client::tail::Client::new(&client.locator, link)?;
            engine::drain(&client, store, database, &host, blob_dir, round_limit).await
        })
        .await
}

/// Waits for a join over a desktop enrollment client.
#[cfg(test)]
pub(crate) async fn await_join(
    client: &crate::peer_enrollment_http::Client,
    store: &crate::protected_local_keys::ProtectedLocalKeyStore,
    database: &Database,
    invitation: Option<aven_core::sync::seed_claim::membership::Invitation>,
    replace: bool,
    deadline: std::time::Instant,
    progress: &(dyn Fn(Stage) + Sync),
) -> Result<()> {
    client
        .transport
        .driver
        .run(|link| async move {
            let client = aven_core::sync::client::enrollment::Client::new(&client.locator, link)?;
            engine::await_join(
                &client, store, database, invitation, replace, deadline, progress,
            )
            .await
        })
        .await
}

fn read_invitation(prompt: &str) -> Result<Zeroizing<String>> {
    let stdin = std::io::stdin();
    let mut text = Zeroizing::new(String::new());
    if stdin.is_terminal() {
        eprint!("{prompt}");
        std::io::stderr().flush()?;
        #[cfg(unix)]
        let _echo = TerminalEchoGuard::disable()?;
        stdin.read_line(&mut text)?;
        eprintln!();
    } else {
        stdin.lock().take(INPUT_LIMIT).read_to_string(&mut text)?;
    }
    Ok(text)
}

#[cfg(unix)]
struct TerminalEchoGuard(libc::termios);

#[cfg(unix)]
impl TerminalEchoGuard {
    fn disable() -> Result<Self> {
        let mut settings = std::mem::MaybeUninit::<libc::termios>::uninit();
        ensure!(unsafe { libc::tcgetattr(libc::STDIN_FILENO, settings.as_mut_ptr()) } == 0);
        let original = unsafe { settings.assume_init() };
        let mut hidden = original;
        hidden.c_lflag &= !libc::ECHO;
        ensure!(unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &hidden) } == 0);
        Ok(Self(original))
    }
}

#[cfg(unix)]
impl Drop for TerminalEchoGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
        }
    }
}

fn confirm_action(
    yes: bool,
    prompt: &str,
    required_error: &str,
    canceled_error: &str,
) -> Result<()> {
    if yes {
        return Ok(());
    }
    let stdin = std::io::stdin();
    ensure!(stdin.is_terminal(), "{required_error}");
    eprint!("{prompt} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    stdin.read_line(&mut answer)?;
    ensure!(
        matches!(answer.trim(), "y" | "Y" | "yes"),
        "{canceled_error}"
    );
    Ok(())
}

fn confirm_setup(yes: bool) -> Result<()> {
    confirm_action(
        yes,
        "Set up sync from this database?",
        "error sync-setup-confirmation-required hint=\"rerun with --yes to confirm\"",
        "error sync-setup-canceled",
    )
}

async fn print_setup_preview(database: &Database, config: &AppConfig, server: &str) -> Result<()> {
    let preview = setup_preview(database, config).await?;
    eprintln!("Set up sync from this database");
    eprintln!("  Database: {}", database.path().display());
    eprintln!("  Server: {server}");
    eprintln!(
        "  Workspaces: {}, tasks: {} (including scheduled and recurring)",
        preview.workspaces, preview.tasks
    );
    if preview.missing_images > 0 {
        eprintln!(
            "  Images missing on this computer: {} (synced as unavailable)",
            preview.missing_images
        );
    }
    if preview.leaves_unencrypted_server {
        eprintln!("  This database stops using its previous unencrypted sync server.");
    }
    eprintln!("Every other device starts from this data.");
    Ok(())
}

pub(crate) async fn setup(database: &Database, config: &AppConfig, args: SetupArgs) -> Result<()> {
    ensure_setup_available(database, config).await?;
    let text = read_invitation("Setup invitation: ")?;
    let invitation = SetupInvitation::decode(&text).map_err(|error| {
        if DeviceInvitation::decode(&text).is_ok() {
            error.context("error sync-setup-invitation-device")
        } else {
            error
        }
    })?;
    let resuming = local_phase(database).await? == LocalPhase::SetupIncomplete;
    if !resuming {
        print_setup_preview(database, config, &invitation.server).await?;
        confirm_setup(args.yes)?;
    }
    let outcome = run_setup(database, config, &invitation, &|stage| {
        if stage == Stage::UploadingData {
            eprintln!("Uploading encrypted data...");
        }
    })
    .await?;
    println!("Sync set up with {}", invitation.server);
    print_outcome(&outcome);
    print_automatic_sync_hint(config);
    Ok(())
}

pub(crate) async fn invite(database: &Database, config: &AppConfig) -> Result<()> {
    let invitation = create_invitation(database, config).await?;
    if invitation.resumed() {
        eprintln!(
            "Resuming the open invitation; it expires in {}.",
            format_duration(invitation.expires_at().saturating_sub(unix_now()?))
        );
    }
    let labels = InvitationLabels::for_streams(
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
    );
    if let Some(label) = labels.invitation {
        eprintln!("{label}");
    }
    println!("{}", invitation.text());
    std::io::stdout().flush()?;
    if let Some(label) = labels.qr {
        print_invitation_qr(
            &invitation,
            label,
            crate::pairing::qr_glyphs(config.sync.qr_glyphs),
        );
    }
    eprintln!("Anyone with this invitation can access all your synced data and manage devices.");
    eprintln!(
        "On the other device, open Aven and scan the QR code or paste the invitation. Waiting for the other device to join..."
    );
    match await_admission_until_interrupt(database, config, &invitation).await? {
        Some(Admission::Admitted) => {
            eprintln!("Device added");
            return Ok(());
        }
        Some(Admission::Cancelled) => bail!(INVITATION_CANCELLED),
        None => {}
    }
    let cancellation = tokio::select! {
        result = cancel_invitation(database, config) => result?,
        _ = tokio::signal::ctrl_c() => {
            eprintln!(
                "Invitation remains open until {}.",
                format_expiry(invitation.expires_at())
            );
            return Ok(());
        }
    };
    eprintln!("{}", cancellation_message(cancellation));
    Ok(())
}

/// Cancels the open invitation for `aven sync invite --cancel`.
pub(crate) async fn cancel(database: &Database, config: &AppConfig) -> Result<()> {
    println!(
        "{}",
        cancellation_message(cancel_invitation(database, config).await?)
    );
    Ok(())
}

fn cancellation_message(cancellation: Cancellation) -> String {
    match cancellation {
        Cancellation::Cancelled => "Invitation cancelled.".to_string(),
        Cancellation::KeysMayHaveBeenSent { expires_at } => format!(
            "Keys may already have been sent. The invitation remains open until {}; the next sync then changes keys.",
            format_expiry(expires_at)
        ),
        Cancellation::None => "No invitation is open.".to_string(),
    }
}

fn format_duration(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60);
    if minutes == 1 {
        "1 minute".to_string()
    } else {
        format!("{minutes} minutes")
    }
}

pub(crate) fn format_expiry(expires_at: u64) -> String {
    chrono::DateTime::from_timestamp(expires_at as i64, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|| expires_at.to_string())
}

/// Labels written to standard error around `aven sync invite` output. Standard
/// output keeps only the invitation text, so labels never reach scripts.
#[derive(Debug, PartialEq, Eq)]
struct InvitationLabels {
    invitation: Option<&'static str>,
    qr: Option<&'static str>,
}

impl InvitationLabels {
    fn for_streams(stdout_is_terminal: bool, stderr_is_terminal: bool) -> Self {
        match (stdout_is_terminal, stderr_is_terminal) {
            (true, true) => Self {
                invitation: Some("Invitation — paste this into Aven on the other device:"),
                qr: Some("Or scan this QR code:"),
            },
            (false, true) => Self {
                invitation: None,
                qr: Some("Scan this QR code on the other device:"),
            },
            (_, false) => Self {
                invitation: None,
                qr: None,
            },
        }
    }
}

/// Shows the invitation QR on an interactive standard error.
fn print_invitation_qr(
    invitation: &PendingInvitation,
    label: &str,
    glyphs: crate::pairing::QrGlyphs,
) {
    let (columns, styled) =
        crate::pairing::output_options(true, std::env::var_os("NO_COLOR").is_some(), || {
            crossterm::terminal::size().ok().map(|(columns, _)| columns)
        });
    match presentation(invitation, glyphs).and_then(|presentation| {
        crate::pairing::render_terminal_qr(presentation.qr(), columns, styled)
    }) {
        Ok(qr) => {
            eprintln!();
            eprintln!("{label}");
            eprint!("{qr}");
        }
        Err(error) => eprintln!("QR code unavailable: {error:#}"),
    }
}

pub(crate) async fn join(database: &Database, config: &AppConfig, args: JoinArgs) -> Result<()> {
    ensure_join_available(database, config).await?;
    let resuming = local_phase(database).await? == LocalPhase::JoinIncomplete;
    let invitation = if resuming && !args.new_invitation {
        None
    } else {
        let text = read_invitation("Device invitation: ")?;
        ensure!(
            !text.trim().is_empty(),
            "error sync-join-invitation-required hint=\"paste the invitation from `aven sync invite`\""
        );
        let invitation = DeviceInvitation::decode(&text).map_err(|error| {
            if SetupInvitation::decode(&text).is_ok() {
                error.context("error sync-device-invitation-setup")
            } else {
                error
            }
        })?;
        eprintln!("Server: {}", invitation.server);
        confirm_action(
            args.yes,
            "Join this sync?",
            "error sync-join-confirmation-required hint=\"rerun with --yes to confirm\"",
            "error sync-join-canceled",
        )?;
        Some(invitation)
    };
    let (server, outcome) = run_join(
        database,
        config,
        || Ok(invitation),
        args.new_invitation,
        &|stage| match stage {
            Stage::WaitingForInviter => eprintln!("Waiting for the inviting device..."),
            Stage::DownloadingTasks => eprintln!("Downloading synced data..."),
            _ => {}
        },
    )
    .await
    .context("error sync-join-command")?;
    println!("Joined sync with {server}");
    print_outcome(&outcome);
    print_automatic_sync_hint(config);
    Ok(())
}

pub(crate) async fn sync(database: &Database, config: &AppConfig, json: bool) -> Result<()> {
    let outcome = run_to_completion(database, config).await?;
    if json {
        print_json_pretty(&outcome)
    } else {
        print_outcome(&outcome);
        Ok(())
    }
}

fn print_automatic_sync_hint(config: &AppConfig) {
    if !config.sync.enabled {
        println!(
            "To sync automatically, run `aven config set sync.enabled true` and `aven daemon install`."
        );
    }
}

fn print_outcome(outcome: &Outcome) {
    if !outcome.metadata_caught_up {
        println!(
            "Sync incomplete: sent {}, received {}; stopped after {} rounds. Run `aven sync` again.",
            outcome.sent_changes, outcome.received_changes, outcome.rounds
        );
        print_conflict_outcome(outcome);
        if outcome.images == "failed" {
            println!(
                "An image transfer failed, or an image added here is missing from this \
                 computer. Changes after it wait until it uploads."
            );
        }
        return;
    }
    if outcome.sent_changes == 0 && outcome.received_changes == 0 {
        println!("Tasks were already up to date");
    } else {
        println!(
            "Changes: sent {}, received {}",
            outcome.sent_changes, outcome.received_changes
        );
    }
    print_conflict_outcome(outcome);
    match outcome.images {
        "complete" => println!("Images are up to date"),
        "unavailable" => println!("Some images are unavailable on the server"),
        "failed" => println!("Some image transfers failed. Run `aven sync` again to retry."),
        _ => println!("Images are still transferring. Run `aven sync` again."),
    }
}

fn print_conflict_outcome(outcome: &Outcome) {
    if outcome.conflicts == 0 {
        return;
    }
    let qualifier = if outcome.new_conflicts > 0 {
        format!(" ({} new)", outcome.new_conflicts)
    } else {
        String::new()
    };
    println!(
        "{} conflict{} need{} a decision{qualifier}. Run `aven conflict list`.",
        outcome.conflicts,
        if outcome.conflicts == 1 { "" } else { "s" },
        if outcome.conflicts == 1 { "s" } else { "" },
    );
}

pub(crate) fn status_state_words(state: &str) -> &'static str {
    match state {
        "not-set-up" => "not set up",
        "setup-incomplete" => "setup incomplete",
        "join-incomplete" => "joining incomplete",
        "key-change-pending" => "key change pending",
        "access-refused" => "access unconfirmed",
        _ => "ready",
    }
}

pub(crate) async fn status(database: &Database, config: &AppConfig, json: bool) -> Result<()> {
    let report = status_report(database, config).await?;
    if json {
        return print_json_pretty(&report);
    }
    if report.state == "not-set-up" {
        println!("Sync: not set up; this database is local only");
        println!(
            "Run `aven sync setup` with an invitation from `aven server setup`, or \
             `aven sync join` on a new database."
        );
        return Ok(());
    }
    println!("Sync: end-to-end encrypted");
    if let Some(server) = &report.server {
        println!("Server: {server}");
    }
    match report.state {
        "setup-incomplete" => println!("State: setup incomplete. Rerun `aven sync setup`."),
        "join-incomplete" => println!(
            "State: joining incomplete. Rerun `aven sync join`; if its invitation expired, \
             pass a new one from the same device with `aven sync join --new-invitation`."
        ),
        "key-change-pending" => println!(
            "State: an invitation expired after keys may have been sent to a device that \
             never joined. The next `aven sync` changes keys before uploading new changes; \
             data that device may already hold stays readable to it."
        ),
        "access-refused" => println!(
            "State: access unconfirmed. The server refused this device. It may have been \
             removed from sync; check from another device. Local tasks and images stay here."
        ),
        _ => println!("State: ready"),
    }
    if let Some(expires_at) = report.invitation_expires_at {
        println!("Invitation: open, expires {}", format_expiry(expires_at));
    } else {
        println!("Invitation: none");
    }
    if report.conflicts > 0 {
        println!(
            "Conflicts: {} need a decision. Run `aven conflict list`.",
            report.conflicts
        );
    } else {
        println!("Conflicts: none");
    }
    if let Some(pending) = report.local_changes_pending {
        println!(
            "Tasks: {}",
            if pending {
                "local changes waiting to sync"
            } else {
                "no local changes waiting"
            }
        );
        let mut images = Vec::new();
        if report.initial_download_pending == Some(true) {
            images.push("waiting for the initial task download");
        }
        if report.image_uploads_pending == Some(true) {
            images.push("uploads waiting");
        }
        if report.image_downloads_pending == Some(true) {
            images.push("downloads waiting");
        }
        if report.images_unavailable == Some(true) {
            images.push("some unavailable on the server");
        }
        if images.is_empty() {
            images.push("nothing waiting");
        }
        println!("Images: {}", images.join(", "));
        println!("Status is local. Run `aven sync` to exchange changes with the server.");
    }
    Ok(())
}
