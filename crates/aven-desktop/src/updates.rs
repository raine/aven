//! The app's own updates: a check now and then, a download the owner starts,
//! and a restart onto the new version once nothing else runs the old one.
//!
//! The work itself happens behind [`Updates`], which the `aven` binary
//! provides. Its methods block, so they run on the background executor.
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use accesskit::Role;
use gpui::{
    AnyElement, App, Context, FocusHandle, FontWeight, FutureExt as _, Global, Stateful, Window,
    div, prelude::*, relative,
};

use crate::dialogs::Dialog;
use crate::kit::*;
use crate::shell::Root;
use crate::theme::{Theme, theme};
use crate::toast::Toast;
use crate::workflows::tab_stop;

/// How long after launch the first check waits, so it stays out of startup.
const FIRST_CHECK: Duration = Duration::from_secs(5);
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const PROGRESS_POLL: Duration = Duration::from_millis(150);
const BLOCKER_POLL: Duration = Duration::from_secs(1);
/// How long a check may take before the app gives up on it, so the owner can
/// check again. The service's own requests time out well before this.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// A newer release on this installation's channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub version: String,
    pub notes_url: String,
    /// Why this installation cannot replace itself, if it cannot.
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// The download is complete and being checked and unpacked.
    pub verifying: bool,
}

impl Progress {
    fn percent(&self) -> Option<u64> {
        let total = self.total.filter(|total| *total > 0)?;
        Some((self.downloaded.min(total) * 100) / total)
    }
}

/// Another process still running this installation's executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub command: String,
}

/// What became of the update this launch finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relaunched {
    Updated { version: String },
    Restored { failed: String, restored: String },
}

pub trait Updates: Send + Sync {
    /// Whether this installation updates itself in the app.
    fn supported(&self) -> bool;
    /// A newer release, if there is one. Without `force` a recent answer
    /// may come from the cache.
    fn check(&self, force: bool) -> anyhow::Result<Option<Available>>;
    /// Downloads and verifies `version` next to the installed app, ready
    /// for [`Updates::restart`].
    fn download(
        &self,
        version: &str,
        progress: &(dyn Fn(Progress) + Send + Sync),
        cancelled: Arc<AtomicBool>,
    ) -> anyhow::Result<()>;
    /// Removes a downloaded update that will not be installed.
    fn discard(&self);
    /// The other processes the swap waits for. Never stopped by the app.
    fn blockers(&self) -> Vec<Process>;
    /// Swaps the downloaded app in and arranges for it to open once this
    /// process exits. The caller quits next.
    fn restart(&self) -> anyhow::Result<()>;
    /// Confirms the update this launch finished, if any.
    fn relaunched(&self) -> anyhow::Result<Option<Relaunched>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Checking,
    Current,
    CheckFailed(String),
    Available(Available),
    Downloading(Available, Progress),
    Failed {
        available: Available,
        error: String,
        installing: bool,
    },
    Ready(Available),
    Waiting(Available, Vec<Process>),
    Restarting(Available),
}

impl Phase {
    /// Whether a check may replace this phase: nothing is under way.
    fn settled(&self) -> bool {
        matches!(
            self,
            Self::Idle | Self::Checking | Self::Current | Self::CheckFailed(_) | Self::Available(_)
        )
    }
}

pub(crate) struct Updater {
    service: Arc<dyn Updates>,
    automatic: bool,
    pub(crate) phase: Phase,
    /// Stops the download or wait under way.
    cancelled: Option<Arc<AtomicBool>>,
}

impl Global for Updater {}

/// The running Aven release, which the app crate cannot know on its own.
struct Version(String);

impl Global for Version {}

/// Makes the app's version and updates available. Without a service the app
/// only shows its version.
pub(crate) fn install(
    version: String,
    service: Option<Arc<dyn Updates>>,
    automatic: bool,
    cx: &mut App,
) {
    cx.set_global(Version(version));
    if let Some(service) = service {
        cx.set_global(Updater {
            service,
            automatic,
            phase: Phase::Idle,
            cancelled: None,
        });
    }
}

fn updater(cx: &App) -> Option<&Updater> {
    cx.try_global::<Updater>()
        .filter(|updater| updater.service.supported())
}

fn set_phase(phase: Phase, cx: &mut App) {
    if let Some(updater) = cx.try_global::<Updater>()
        && updater.phase != phase
    {
        cx.global_mut::<Updater>().phase = phase;
        // The root view redraws only when told to, and a global is not an
        // entity. Many phases start from the root view's own listeners.
        crate::app::redraw_root(cx);
    }
}

fn phase(cx: &App) -> Phase {
    updater(cx).map_or(Phase::Idle, |updater| updater.phase.clone())
}

/// The running version.
pub(crate) fn version(cx: &App) -> String {
    cx.global::<Version>().0.clone()
}

/// Whether the app is quitting onto a new version it installed.
pub(crate) fn restarting(cx: &App) -> bool {
    updater(cx).is_some_and(|updater| matches!(updater.phase, Phase::Restarting(_)))
}

/// Confirms the update this launch finished, then checks now and then.
pub(crate) fn start(cx: &mut App) {
    let Some(updater) = updater(cx) else {
        return;
    };
    let service = updater.service.clone();
    let automatic = updater.automatic;
    cx.spawn(async move |cx| {
        let relaunched = cx
            .background_executor()
            .spawn(async move { service.relaunched() })
            .await;
        cx.update(|cx| match relaunched {
            Ok(Some(relaunched)) => show_relaunched(relaunched, cx),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(error = %format_args!("{error:#}"), "could not confirm the update")
            }
        });
        if !automatic {
            return;
        }
        cx.background_executor().timer(FIRST_CHECK).await;
        loop {
            cx.update(|cx| check(false, cx));
            cx.background_executor().timer(CHECK_INTERVAL).await;
        }
    })
    .detach();
}

fn show_relaunched(relaunched: Relaunched, cx: &mut App) {
    let toast = match relaunched {
        Relaunched::Updated { version } => Toast::done(format!("Updated to Aven {version}")),
        Relaunched::Restored { failed, restored } => Toast::error(format!(
            "Aven {failed} didn\u{2019}t start, so {restored} was restored"
        ))
        .detail("Nothing in your tasks changed."),
    };
    crate::app::with_root(cx, |root, _, cx| {
        root.session.show_toast(toast);
        cx.notify();
    });
}

/// Looks for a newer release. A forced check is the owner asking, so it
/// shows that it runs and why it failed.
pub(crate) fn check(force: bool, cx: &mut App) {
    let Some(updater) = updater(cx) else {
        return;
    };
    if !may_check(&updater.phase) {
        if force {
            tracing::info!(phase = ?updater.phase, "update check not started");
        }
        return;
    }
    let service = updater.service.clone();
    let before = updater.phase.clone();
    tracing::info!(force, "checking for updates");
    if force {
        set_phase(Phase::Checking, cx);
    }
    cx.spawn(async move |cx| {
        let executor = cx.background_executor().clone();
        let checked = executor
            .spawn(async move { service.check(force) })
            .with_timeout(CHECK_TIMEOUT, &executor)
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the check didn\u{2019}t finish in time")));
        cx.update(|cx| {
            if !phase(cx).settled() {
                return;
            }
            if force
                && matches!(before, Phase::Available(_))
                && let Err(error) = &checked
            {
                let detail = format!("{error:#}");
                crate::app::with_root(cx, |root, _, cx| {
                    root.session.show_toast(
                        Toast::error("Couldn\u{2019}t check for updates").detail(detail),
                    );
                    cx.notify();
                });
            }
            set_phase(after_check(checked, force, before), cx);
        });
    })
    .detach();
}

/// Whether a check may start: nothing is under way, a check included.
fn may_check(phase: &Phase) -> bool {
    phase.settled() && *phase != Phase::Checking
}

/// The phase a check ends in. A forced check always leaves `Checking`.
fn after_check(checked: anyhow::Result<Option<Available>>, force: bool, before: Phase) -> Phase {
    match checked {
        Ok(Some(available)) => {
            tracing::info!(version = %available.version, "an update is available");
            Phase::Available(available)
        }
        Ok(None) => {
            tracing::info!("aven is up to date");
            Phase::Current
        }
        Err(error) if force && !matches!(before, Phase::Available(_)) => {
            tracing::warn!(error = %format_args!("{error:#}"), "update check failed");
            Phase::CheckFailed(format!("{error:#}"))
        }
        Err(error) => {
            tracing::debug!(error = %format_args!("{error:#}"), "update check failed");
            before
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DownloadResult {
    Ready,
    Replaced(Option<Available>),
}

/// A download may start only after the server confirms the offered version.
fn checked_download(
    service: &dyn Updates,
    version: &str,
    report: &(dyn Fn(Progress) + Send + Sync),
    cancelled: Arc<AtomicBool>,
) -> anyhow::Result<DownloadResult> {
    checked_update(service, version, &cancelled, || {
        service.download(version, report, cancelled.clone())
    })
}

fn checked_update(
    service: &dyn Updates,
    version: &str,
    cancelled: &AtomicBool,
    operation: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<DownloadResult> {
    let latest = service.check(true)?;
    if latest
        .as_ref()
        .is_none_or(|latest| latest.version != version || latest.blocked.is_some())
    {
        return Ok(DownloadResult::Replaced(latest));
    }
    anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "update cancelled");
    operation()?;
    Ok(DownloadResult::Ready)
}

fn replace_offer(latest: Option<Available>, cx: &mut App) {
    crate::app::with_root(cx, |root, _, cx| {
        root.session.show_toast(
            Toast::error("This release is no longer available")
                .detail("The update offer has been refreshed. Nothing was installed."),
        );
        cx.notify();
    });
    set_phase(latest.map_or(Phase::Current, Phase::Available), cx);
}

/// Downloads the available update. It installs only on a restart.
pub(crate) fn download(cx: &mut App) {
    let Some(updater) = updater(cx) else {
        return;
    };
    let available = match &updater.phase {
        Phase::Available(available) | Phase::Failed { available, .. } => available.clone(),
        _ => return,
    };
    if available.blocked.is_some() {
        return;
    }
    let service = updater.service.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    cx.global_mut::<Updater>().cancelled = Some(cancelled.clone());
    set_phase(
        Phase::Downloading(available.clone(), Progress::default()),
        cx,
    );
    let shared = Arc::new(Mutex::new(Progress::default()));
    let task = cx.background_executor().spawn({
        let (version, shared, cancelled) =
            (available.version.clone(), shared.clone(), cancelled.clone());
        async move {
            let report = move |progress: Progress| {
                if let Ok(mut shared) = shared.lock() {
                    *shared = progress;
                }
            };
            checked_download(service.as_ref(), &version, &report, cancelled)
        }
    });
    let done = Arc::new(AtomicBool::new(false));
    cx.spawn({
        let (available, done) = (available.clone(), done.clone());
        async move |cx| {
            while !done.load(Ordering::Relaxed) {
                cx.background_executor().timer(PROGRESS_POLL).await;
                let progress = *shared.lock().unwrap_or_else(|error| error.into_inner());
                cx.update(|cx| {
                    if matches!(phase(cx), Phase::Downloading(..)) {
                        set_phase(Phase::Downloading(available.clone(), progress), cx);
                    }
                });
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let result = task.await;
        done.store(true, Ordering::Relaxed);
        cx.update(|cx| {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let next = match result {
                Ok(DownloadResult::Ready) => Phase::Ready(available),
                Ok(DownloadResult::Replaced(latest)) => {
                    replace_offer(latest, cx);
                    return;
                }
                Err(error) => Phase::Failed {
                    available,
                    error: format!("{error:#}"),
                    installing: false,
                },
            };
            set_phase(next, cx);
        });
    })
    .detach();
}

/// Stops the download or the wait under way.
fn cancel(cx: &mut App) {
    let Some(updater) = updater(cx) else {
        return;
    };
    let next = match &updater.phase {
        Phase::Downloading(available, _) => Phase::Available(available.clone()),
        Phase::Waiting(available, _) => Phase::Ready(available.clone()),
        _ => return,
    };
    let service = updater.service.clone();
    if let Some(cancelled) = cx.global_mut::<Updater>().cancelled.take() {
        cancelled.store(true, Ordering::Relaxed);
    }
    if matches!(next, Phase::Available(_)) {
        cx.background_executor()
            .spawn(async move { service.discard() })
            .detach();
    }
    set_phase(next, cx);
}

/// Waits for the other processes running this installation, then swaps the
/// update in and quits so it opens.
pub(crate) fn restart(cx: &mut App) {
    let Some(updater) = updater(cx) else {
        return;
    };
    let Phase::Ready(available) = updater.phase.clone() else {
        return;
    };
    let service = updater.service.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    cx.global_mut::<Updater>().cancelled = Some(cancelled.clone());
    set_phase(Phase::Waiting(available.clone(), Vec::new()), cx);
    cx.spawn(async move |cx| {
        loop {
            let blockers = cx
                .background_executor()
                .spawn({
                    let service = service.clone();
                    async move { service.blockers() }
                })
                .await;
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if blockers.is_empty() {
                break;
            }
            cx.update(|cx| set_phase(Phase::Waiting(available.clone(), blockers), cx));
            cx.background_executor().timer(BLOCKER_POLL).await;
        }
        cx.update(|cx| set_phase(Phase::Restarting(available.clone()), cx));
        let version = available.version.clone();
        let restarted = cx
            .background_executor()
            .spawn(async move {
                let result =
                    checked_update(service.as_ref(), &version, &cancelled, || service.restart());
                if matches!(result, Ok(DownloadResult::Replaced(_))) {
                    service.discard();
                }
                result
            })
            .await;
        cx.update(|cx| match restarted {
            Ok(DownloadResult::Ready) => crate::app::with_root(cx, Root::quit),
            Ok(DownloadResult::Replaced(latest)) => replace_offer(latest, cx),
            Err(error) => set_phase(
                Phase::Failed {
                    available,
                    error: format!("{error:#}"),
                    installing: true,
                },
                cx,
            ),
        });
    })
    .detach();
}

/// Drops a downloaded update the app quits without installing.
pub(crate) fn discard(cx: &App) {
    if let Some(updater) = updater(cx)
        && !matches!(updater.phase, Phase::Restarting(_))
    {
        updater.service.discard();
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.)
}

impl Root {
    pub(crate) fn open_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = Dialog::Update {
            buttons: [cx.focus_handle(), cx.focus_handle()],
            phase: None,
        };
        // The first render moves focus onto the phase's buttons.
        self.open_dialog_at(dialog, Vec::new(), Vec::new(), 0, window, cx);
    }

    /// Moves focus with the Update dialog's phase, which changes on its own,
    /// so focus stays on a control the dialog shows and Return and Escape
    /// reach the dialog.
    pub(crate) fn focus_update_phase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = phase(cx);
        let Some(layer) = self.dialog.as_mut() else {
            return;
        };
        let Dialog::Update { buttons, phase } = &mut layer.dialog else {
            return;
        };
        let discriminant = std::mem::discriminant(&current);
        if *phase == Some(discriminant) {
            return;
        }
        *phase = Some(discriminant);
        let (fields, first) = focus_plan(&current);
        layer.fields = fields.iter().map(|&at| buttons[at].clone()).collect();
        let focus = first.map_or_else(|| layer.container.clone(), |at| buttons[at].clone());
        cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
    }

    /// The sidebar row while an update is on offer or under way.
    pub(crate) fn render_update_row(&self, cx: &mut Context<Self>) -> Option<Stateful<gpui::Div>> {
        let t = theme(cx);
        let phase = phase(cx);
        let (path, label, percent, color) = match &phase {
            Phase::Idle | Phase::Checking | Phase::Current | Phase::CheckFailed(_) => return None,
            Phase::Available(_) => ("icons/ic-update.svg", "Update available", None, t.accent),
            Phase::Downloading(_, progress) if progress.verifying => {
                ("icons/ic-update.svg", "Preparing update", None, t.accent)
            }
            Phase::Downloading(_, progress) => (
                "icons/ic-update.svg",
                "Downloading update",
                progress.percent(),
                t.accent,
            ),
            Phase::Failed { .. } => ("icons/toast-warn.svg", "Update failed", None, t.warn),
            Phase::Ready(_) => ("icons/ic-restart.svg", "Restart to update", None, t.accent),
            Phase::Waiting(..) => ("icons/ic-restart.svg", "Waiting to update", None, t.accent),
            Phase::Restarting(_) => ("icons/ic-restart.svg", "Restarting", None, t.accent),
        };
        Some(
            nav_item("update", &t)
                .role(Role::Button)
                .aria_label(label)
                .h(px(28.))
                .mb_0()
                .text_color(color)
                .on_click(cx.listener(|this, _, window, cx| this.open_update(window, cx)))
                .child(icon(path, color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
                .when_some(percent, |row, percent| {
                    row.child(nav_count(format!("{percent}%"), false, &t))
                }),
        )
    }

    fn update_status_text(phase: Phase, current: &str) -> (String, &'static str) {
        match phase {
            Phase::Idle => (format!("Version {current}"), "Check for updates"),
            Phase::Checking => (
                format!("Version {current} \u{b7} Checking\u{2026}"),
                "Check for updates",
            ),
            Phase::Current => (
                format!("Version {current} \u{b7} Up to date"),
                "Check for updates",
            ),
            Phase::CheckFailed(error) => (
                format!("Version {current} \u{b7} Couldn\u{2019}t check for updates: {error}"),
                "Check for updates",
            ),
            Phase::Available(available) if available.blocked.is_some() => (
                format!(
                    "Version {} is available \u{b7} you have {current}",
                    available.version
                ),
                "View update",
            ),
            Phase::Available(available) => (
                format!(
                    "Version {} is available \u{b7} you have {current}",
                    available.version
                ),
                "Download update",
            ),
            Phase::Downloading(available, progress) => (
                match progress.percent() {
                    Some(percent) => {
                        format!("Downloading Aven {} \u{b7} {percent}%", available.version)
                    }
                    None => format!("Downloading Aven {}", available.version),
                },
                "Show progress",
            ),
            Phase::Failed { available, .. } => (
                format!(
                    "Aven {} didn\u{2019}t download \u{b7} you have {current}",
                    available.version
                ),
                "Try again",
            ),
            Phase::Ready(available)
            | Phase::Waiting(available, _)
            | Phase::Restarting(available) => (
                format!(
                    "Aven {} is ready \u{b7} you have {current}",
                    available.version
                ),
                "Restart to update",
            ),
        }
    }

    /// The version line and its button in Settings' About row.
    pub(crate) fn update_status(
        &self,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> (String, Option<Stateful<gpui::Div>>) {
        let current = version(cx);
        if updater(cx).is_none() {
            return (format!("Version {current}"), None);
        }
        let (note, label) = Self::update_status_text(phase(cx), &current);
        let button =
            line_btn("update-action", label, t).on_click(cx.listener(|this, _, window, cx| {
                match phase(cx) {
                    Phase::Idle | Phase::Current | Phase::CheckFailed(_) => check(true, cx),
                    Phase::Checking => {}
                    Phase::Available(available) if available.blocked.is_none() => {
                        download(cx);
                        this.open_update(window, cx);
                    }
                    Phase::Failed {
                        installing: false, ..
                    } => {
                        download(cx);
                        this.open_update(window, cx);
                    }
                    Phase::Ready(_) => {
                        restart(cx);
                        this.open_update(window, cx);
                    }
                    _ => this.open_update(window, cx),
                }
            }));
        (note, Some(button))
    }

    pub(crate) fn render_update(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Dialog::Update {
            buttons: [secondary, primary],
            ..
        }) = self.dialog.as_ref().map(|layer| &layer.dialog)
        else {
            return Vec::new();
        };
        let current = version(cx);
        let footer = |cx: &mut Context<Self>| {
            div()
                .px(px(16.))
                .pb(px(14.))
                .pt(px(4.))
                .flex()
                .gap(px(6.))
                .child(div().flex_1())
                .child(tab_stop(
                    btn("update-close", "Close", t).px(px(8.)).child("Close"),
                    secondary,
                    t,
                    cx,
                    |this, window, cx| this.close_dialog(window, cx),
                ))
        };
        let (heading, body, actions): (String, Vec<AnyElement>, gpui::Div) = match phase(cx) {
            Phase::Idle => (
                "Check for updates".into(),
                vec![paragraph(format!("You have Aven {current}."), t)],
                footer(cx),
            ),
            Phase::Checking => (
                "Checking for updates\u{2026}".into(),
                vec![paragraph(format!("You have Aven {current}."), t)],
                footer(cx),
            ),
            Phase::CheckFailed(error) => (
                "Couldn\u{2019}t check for updates".into(),
                vec![paragraph(error, t)],
                footer(cx),
            ),
            Phase::Current => (
                "Aven is up to date".into(),
                vec![paragraph(format!("You have Aven {current}."), t)],
                footer(cx),
            ),
            Phase::Available(available) => {
                let notes = available.notes_url.clone();
                let body = match &available.blocked {
                    Some(reason) => vec![paragraph(reason.clone(), t)],
                    None => vec![
                        paragraph(
                            format!(
                                "You have {current}. Download it now, then restart Aven when it suits you."
                            ),
                            t,
                        ),
                        div()
                            .pt(px(8.))
                            .child(
                                bare_btn("update-notes", "Release notes", t)
                                    .role(Role::Link)
                                    .px(px(0.))
                                    .h(px(20.))
                                    .text_color(t.accent)
                                    .on_click({
                                        let notes = notes.clone();
                                        move |_, _, cx| cx.open_url(&notes)
                                    })
                                    .child("Release notes"),
                            )
                            .into_any_element(),
                    ],
                };
                let blocked = available.blocked.is_some();
                (
                    format!("Aven {} is available", available.version),
                    body,
                    two_buttons(
                        (
                            "Later",
                            secondary,
                            Rc::new(
                                |this: &mut Root, window: &mut Window, cx: &mut Context<Root>| {
                                    this.close_dialog(window, cx)
                                },
                            ),
                        ),
                        if blocked {
                            ("Open download page", primary, Rc::new(move |this: &mut Root, window: &mut Window, cx: &mut Context<Root>| {
                                cx.open_url(&notes);
                                this.close_dialog(window, cx);
                            }))
                        } else {
                            (
                                "Download update",
                                primary,
                                Rc::new(|_: &mut Root, _: &mut Window, cx: &mut Context<Root>| {
                                    download(cx)
                                }),
                            )
                        },
                        false,
                        t,
                        cx,
                    ),
                )
            }
            Phase::Downloading(available, progress) => {
                let fraction = if progress.verifying {
                    1.
                } else {
                    progress
                        .percent()
                        .map_or(0., |percent| percent as f32 / 100.)
                };
                let (left, right) = if progress.verifying {
                    ("Checking the download".to_string(), String::new())
                } else {
                    match (progress.total, progress.percent()) {
                        (Some(total), Some(percent)) => (
                            format!(
                                "{} of {} MB",
                                megabytes(progress.downloaded),
                                megabytes(total)
                            ),
                            format!("{percent}%"),
                        ),
                        _ => (
                            format!("{} MB", megabytes(progress.downloaded)),
                            String::new(),
                        ),
                    }
                };
                (
                    format!("Downloading Aven {}", available.version),
                    vec![
                        div()
                            .mt(px(4.))
                            .h(px(6.))
                            .rounded(px(3.))
                            .bg(t.line)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .rounded(px(3.))
                                    .bg(t.accent),
                            )
                            .into_any_element(),
                        div()
                            .flex()
                            .justify_between()
                            .mt(px(6.))
                            .text(Text::Secondary)
                            .text_color(t.muted)
                            .child(left)
                            .child(right)
                            .into_any_element(),
                        div()
                            .mt(px(8.))
                            .text(Text::Secondary)
                            .text_color(t.faint)
                            .child("You can keep working. Aven tells you when it\u{2019}s ready.")
                            .into_any_element(),
                    ],
                    two_buttons(
                        (
                            "Cancel download",
                            secondary,
                            Rc::new(|_: &mut Root, _: &mut Window, cx: &mut Context<Root>| {
                                cancel(cx)
                            }),
                        ),
                        (
                            "Hide",
                            primary,
                            Rc::new(
                                |this: &mut Root, window: &mut Window, cx: &mut Context<Root>| {
                                    this.close_dialog(window, cx)
                                },
                            ),
                        ),
                        true,
                        t,
                        cx,
                    ),
                )
            }
            Phase::Failed {
                error, installing, ..
            } => (
                if installing {
                    "The update didn\u{2019}t install".into()
                } else {
                    "The update didn\u{2019}t download".into()
                },
                vec![
                    paragraph(
                        format!("Nothing was changed. Aven {current} keeps running."),
                        t,
                    ),
                    div()
                        .mt(px(8.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(RADIUS_CONTROL))
                        .bg(t.raised)
                        .font_family(MONO)
                        .text(Text::Secondary)
                        .text_color(t.muted)
                        .child(error)
                        .into_any_element(),
                ],
                two_buttons(
                    (
                        "Close",
                        secondary,
                        Rc::new(
                            |this: &mut Root, window: &mut Window, cx: &mut Context<Root>| {
                                this.close_dialog(window, cx)
                            },
                        ),
                    ),
                    (
                        "Try again",
                        primary,
                        Rc::new(|_: &mut Root, _: &mut Window, cx: &mut Context<Root>| {
                            download(cx)
                        }),
                    ),
                    false,
                    t,
                    cx,
                ),
            ),
            Phase::Ready(available) => (
                format!("Aven {} is ready", available.version),
                vec![paragraph(
                    "Restart Aven to finish updating. Your tasks and settings stay as they are."
                        .to_string(),
                    t,
                )],
                two_buttons(
                    (
                        "Later",
                        secondary,
                        Rc::new(
                            |this: &mut Root, window: &mut Window, cx: &mut Context<Root>| {
                                this.close_dialog(window, cx)
                            },
                        ),
                    ),
                    (
                        "Restart to update",
                        primary,
                        Rc::new(|_: &mut Root, _: &mut Window, cx: &mut Context<Root>| restart(cx)),
                    ),
                    false,
                    t,
                    cx,
                ),
            ),
            Phase::Waiting(_, blockers) => {
                let rows = blockers.into_iter().map(|process| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(26.))
                        .child(div().size(px(6.)).rounded(px(3.)).bg(t.warn))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family(MONO)
                                .text(Text::Secondary)
                                .text_color(t.ink)
                                .child(process.command),
                        )
                        .child(
                            div()
                                .text(Text::Secondary)
                                .text_color(t.faint)
                                .child(format!("pid {}", process.pid)),
                        )
                });
                (
                    "Waiting for other Aven processes".into(),
                    vec![
                        paragraph(
                            format!(
                                "These still run Aven {current}. The update can upgrade your tasks \
                                 database, and a version that is already running keeps writing to \
                                 it, so Aven restarts once they exit. Quit them yourself; Aven \
                                 won\u{2019}t stop them."
                            ),
                            t,
                        ),
                        div()
                            .mt(px(8.))
                            .px(px(10.))
                            .py(px(2.))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(t.raised)
                            .children(rows)
                            .into_any_element(),
                    ],
                    div()
                        .px(px(16.))
                        .pb(px(14.))
                        .pt(px(4.))
                        .flex()
                        .justify_end()
                        .child(tab_stop(
                            line_btn("update-cancel", "Cancel", t),
                            secondary,
                            t,
                            cx,
                            |_, _, cx| cancel(cx),
                        )),
                )
            }
            Phase::Restarting(available) => (
                "Restarting Aven".into(),
                vec![paragraph(
                    format!("Aven opens again on {} in a moment.", available.version),
                    t,
                )],
                div().pb(px(10.)),
            ),
        };
        vec![
            div()
                .px(px(16.))
                .pt(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.ink)
                .child(heading)
                .into_any_element(),
            div()
                .px(px(16.))
                .py(px(10.))
                .text_color(t.body)
                .children(body)
                .into_any_element(),
            actions.into_any_element(),
        ]
    }
}

/// The Update dialog's buttons a phase shows, as indexes into its secondary
/// and primary pair in Tab order, and the one that takes focus. Without one,
/// the dialog itself takes focus: Return there closes it, where on Cancel it
/// would stop the wait for other processes.
fn focus_plan(phase: &Phase) -> (&'static [usize], Option<usize>) {
    match phase {
        Phase::Idle | Phase::Checking | Phase::Current | Phase::CheckFailed(_) => (&[0], Some(0)),
        Phase::Available(_) | Phase::Downloading(..) | Phase::Failed { .. } | Phase::Ready(_) => {
            (&[0, 1], Some(1))
        }
        Phase::Waiting(..) => (&[0], None),
        Phase::Restarting(_) => (&[], None),
    }
}

type Run = Rc<dyn Fn(&mut Root, &mut Window, &mut Context<Root>)>;

/// A secondary and a primary button. A `split` footer puts the secondary
/// one on the left, apart from the primary.
fn two_buttons(
    (secondary_label, secondary, run_secondary): (&'static str, &FocusHandle, Run),
    (primary_label, primary, run_primary): (&'static str, &FocusHandle, Run),
    split: bool,
    t: &Theme,
    cx: &mut Context<Root>,
) -> gpui::Div {
    div()
        .px(px(16.))
        .pb(px(14.))
        .pt(px(4.))
        .flex()
        .gap(px(6.))
        .child(tab_stop(
            line_btn(secondary_label, secondary_label, t),
            secondary,
            t,
            cx,
            move |this, window, cx| run_secondary(this, window, cx),
        ))
        .when(split, |footer| footer.child(div().flex_1()))
        .when(!split, |footer| footer.justify_end())
        .child(tab_stop(
            fill_btn(primary_label, primary_label, t),
            primary,
            t,
            cx,
            move |this, window, cx| run_primary(this, window, cx),
        ))
}

fn paragraph(text: String, t: &Theme) -> AnyElement {
    div().text_color(t.body).child(text).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available() -> Available {
        Available {
            version: "9.9.9".into(),
            notes_url: String::new(),
            blocked: None,
        }
    }

    #[test]
    fn settings_shows_the_check_failure_reason_and_keeps_retry_available() {
        for reason in [
            "rate limit exceeded",
            "request timed out",
            "missing Desktop asset",
        ] {
            let (note, action) =
                Root::update_status_text(Phase::CheckFailed(reason.into()), "1.2.3");
            assert!(note.contains(reason), "{note}");
            assert!(!note.contains("Up to date"));
            assert_eq!(action, "Check for updates");
        }
    }

    struct TestUpdates {
        answer: Result<Option<Available>, &'static str>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl Updates for TestUpdates {
        fn supported(&self) -> bool {
            true
        }
        fn check(&self, force: bool) -> anyhow::Result<Option<Available>> {
            assert!(force, "install-time checks must bypass the TTL");
            self.calls.lock().unwrap().push("check");
            self.answer.clone().map_err(anyhow::Error::msg)
        }
        fn download(
            &self,
            _: &str,
            _: &(dyn Fn(Progress) + Send + Sync),
            _: Arc<AtomicBool>,
        ) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push("download");
            Ok(())
        }
        fn discard(&self) {
            unreachable!()
        }
        fn blockers(&self) -> Vec<Process> {
            unreachable!()
        }
        fn restart(&self) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push("restart");
            Ok(())
        }
        fn relaunched(&self) -> anyhow::Result<Option<Relaunched>> {
            unreachable!()
        }
    }

    #[test]
    fn installation_rechecks_before_downloading_and_replaces_withdrawn_offers() {
        let offered = available();
        let older = Available {
            version: "9.9.8".into(),
            ..offered.clone()
        };
        let newer = Available {
            version: "9.9.10".into(),
            ..offered.clone()
        };
        for latest in [Some(offered.clone()), Some(older), Some(newer), None] {
            let service = TestUpdates {
                answer: Ok(latest.clone()),
                calls: Mutex::new(Vec::new()),
            };
            let result = checked_download(
                &service,
                &offered.version,
                &|_| {},
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
            if latest == Some(offered.clone()) {
                assert_eq!(result, DownloadResult::Ready);
                assert_eq!(*service.calls.lock().unwrap(), ["check", "download"]);
            } else {
                assert_eq!(result, DownloadResult::Replaced(latest));
                assert_eq!(*service.calls.lock().unwrap(), ["check"]);
            }
        }
    }

    #[test]
    fn a_staged_update_is_revalidated_before_the_final_swap() {
        for answer in [Ok(Some(available())), Ok(None), Err("timeout")] {
            let service = TestUpdates {
                answer: answer.clone(),
                calls: Mutex::new(Vec::new()),
            };
            let result = checked_update(&service, "9.9.9", &AtomicBool::new(false), || {
                service.restart()
            });
            match answer {
                Ok(Some(_)) => {
                    assert_eq!(result.unwrap(), DownloadResult::Ready);
                    assert_eq!(*service.calls.lock().unwrap(), ["check", "restart"]);
                }
                Ok(None) => {
                    assert_eq!(result.unwrap(), DownloadResult::Replaced(None));
                    assert_eq!(*service.calls.lock().unwrap(), ["check"]);
                }
                Err(_) => {
                    assert!(result.is_err());
                    assert_eq!(*service.calls.lock().unwrap(), ["check"]);
                }
            }
        }
    }

    #[test]
    fn transient_failures_keep_offers_but_never_start_an_install() {
        for reason in ["network error", "timeout", "HTTP 503", "rate limit"] {
            let service = TestUpdates {
                answer: Err(reason),
                calls: Mutex::new(Vec::new()),
            };
            assert!(
                checked_download(&service, "9.9.9", &|_| {}, Arc::new(AtomicBool::new(false)))
                    .is_err()
            );
            assert_eq!(*service.calls.lock().unwrap(), ["check"]);
            for force in [false, true] {
                let before = Phase::Available(available());
                assert_eq!(
                    after_check(Err(anyhow::anyhow!(reason)), force, before.clone()),
                    before
                );
            }
        }
    }

    #[test]
    fn cancelling_during_revalidation_prevents_download() {
        let service = TestUpdates {
            answer: Ok(Some(available())),
            calls: Mutex::new(Vec::new()),
        };
        assert!(
            checked_download(&service, "9.9.9", &|_| {}, Arc::new(AtomicBool::new(true))).is_err()
        );
        assert_eq!(*service.calls.lock().unwrap(), ["check"]);
    }

    #[test]
    fn a_forced_check_always_leaves_checking() {
        let results = [
            Ok(Some(available())),
            Ok(None),
            Err(anyhow::anyhow!("offline")),
        ];
        for checked in results {
            let next = after_check(checked, true, Phase::Idle);
            assert_ne!(next, Phase::Checking);
            assert!(may_check(&next), "{next:?} lets the owner check again");
        }
    }

    #[test]
    fn a_failed_forced_check_says_why_and_a_quiet_one_keeps_its_phase() {
        let failed = || Err(anyhow::anyhow!("offline"));
        assert_eq!(
            after_check(failed(), true, Phase::Current),
            Phase::CheckFailed("offline".into())
        );
        assert_eq!(
            after_check(failed(), false, Phase::Available(available())),
            Phase::Available(available())
        );
    }

    #[test]
    fn a_check_waits_for_one_under_way_and_for_an_update() {
        assert!(!may_check(&Phase::Checking));
        assert!(!may_check(&Phase::Ready(available())));
        assert!(may_check(&Phase::CheckFailed("offline".into())));
    }

    #[test]
    fn progress_reports_a_percentage_only_with_a_known_size() {
        let progress = |downloaded, total| Progress {
            downloaded,
            total,
            verifying: false,
        };
        assert_eq!(progress(5, None).percent(), None);
        assert_eq!(progress(5, Some(0)).percent(), None);
        assert_eq!(progress(21, Some(50)).percent(), Some(42));
        assert_eq!(progress(80, Some(50)).percent(), Some(100));
    }

    #[test]
    fn only_a_settled_phase_takes_a_check() {
        let available = Available {
            version: "1.3.0".into(),
            notes_url: String::new(),
            blocked: None,
        };
        assert!(Phase::Current.settled());
        assert!(Phase::Available(available.clone()).settled());
        assert!(!Phase::Ready(available.clone()).settled());
        assert!(!Phase::Downloading(available, Progress::default()).settled());
    }

    #[test]
    fn every_phase_focuses_a_button_it_shows_or_the_dialog() {
        let available = Available {
            version: "1.3.0".into(),
            notes_url: String::new(),
            blocked: None,
        };
        let phases = [
            Phase::Idle,
            Phase::Checking,
            Phase::Current,
            Phase::CheckFailed("offline".into()),
            Phase::Available(available.clone()),
            Phase::Downloading(available.clone(), Progress::default()),
            Phase::Failed {
                available: available.clone(),
                error: "offline".into(),
                installing: false,
            },
            Phase::Ready(available.clone()),
            Phase::Waiting(available.clone(), Vec::new()),
            Phase::Restarting(available),
        ];
        for phase in &phases {
            let (fields, first) = focus_plan(phase);
            if let Some(first) = first {
                assert!(fields.contains(&first), "{phase:?}");
            }
        }
        assert_eq!(focus_plan(&Phase::Current), (&[0][..], Some(0)));
        assert_eq!(focus_plan(&phases[7]), (&[0, 1][..], Some(1)));
        assert_eq!(focus_plan(&phases[8]), (&[0][..], None));
        assert_eq!(focus_plan(&phases[9]), (&[][..], None));
    }
}
