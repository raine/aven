//! The toast on screen and the ones waiting behind it. The rules for how long a
//! toast stays and what may interrupt it live here, so a toast shown from
//! anywhere expires the same way. It has no GPUI types, and takes the time from
//! its caller.
use std::time::{Duration, Instant};

use crate::adapter::{Command, Go};

/// A toast with an action stays this long, so there is time to reach it.
pub const ACTION_LIFETIME: Duration = Duration::from_secs(8);
pub const WARN_LIFETIME: Duration = Duration::from_secs(6);
pub const CONFIRMATION_LIFETIME: Duration = Duration::from_secs(3);
/// A toast that comes back after an interruption has at least this long left.
pub const RESUMED_LIFETIME: Duration = Duration::from_millis(2500);
/// How long an ended toast takes to fade out.
pub const LEAVE: Duration = Duration::from_millis(160);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Done,
    Info,
    Warn,
    Error,
    /// Work under way, which a later toast resolves.
    Progress,
}

/// The task a toast is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub reference: String,
    pub project_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastAction {
    /// Runs a command that did not save.
    Retry(Command),
    /// Reloads after a save whose refresh failed. It never repeats the write.
    Refresh,
    /// Moves somewhere the write made interesting.
    Go(Go),
    /// Brings a discarded draft back, which the window holds.
    RestoreDraft,
    /// Opens a link in the browser.
    Open(String),
}

impl ToastAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Retry(_) => "Retry",
            Self::Refresh => "Refresh",
            Self::Go(go) => go.label,
            Self::RestoreDraft => "Restore",
            Self::Open(_) => "Open",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub tone: Tone,
    pub title: String,
    pub subject: Option<Subject>,
    /// A second line in muted text.
    pub detail: Option<String>,
    pub action: Option<ToastAction>,
    /// Offers the store's undo, which takes back the change this toast is about.
    pub undo: bool,
    /// How many times in a row this message was shown.
    pub times: usize,
    /// Keys the entrance, which plays once per toast.
    pub run: usize,
    /// Keys the nudge a repeat gives, or 0 before any repeat.
    pub bump: usize,
}

impl Toast {
    fn new(tone: Tone, title: impl Into<String>) -> Self {
        Self {
            tone,
            title: title.into(),
            subject: None,
            detail: None,
            action: None,
            undo: false,
            times: 1,
            run: 0,
            bump: 0,
        }
    }

    pub fn done(title: impl Into<String>) -> Self {
        Self::new(Tone::Done, title)
    }

    pub fn info(title: impl Into<String>) -> Self {
        Self::new(Tone::Info, title)
    }

    pub fn warn(title: impl Into<String>) -> Self {
        Self::new(Tone::Warn, title)
    }

    pub fn error(title: impl Into<String>) -> Self {
        Self::new(Tone::Error, title)
    }

    pub fn progress(title: impl Into<String>) -> Self {
        Self::new(Tone::Progress, title)
    }

    pub fn subject(self, subject: Option<Subject>) -> Self {
        Self { subject, ..self }
    }

    pub fn detail(self, detail: impl Into<String>) -> Self {
        Self {
            detail: Some(detail.into()).filter(|detail| !detail.is_empty()),
            ..self
        }
    }

    pub fn action(self, action: Option<ToastAction>) -> Self {
        Self { action, ..self }
    }

    pub fn undo(self, undo: bool) -> Self {
        Self { undo, ..self }
    }

    /// Whether the toast offers anything to do.
    pub fn actionable(&self) -> bool {
        self.undo || self.action.is_some()
    }

    /// A dismiss button stands in for actions, except on errors, which keep one.
    pub fn dismissable(&self) -> bool {
        !self.actionable() || self.tone == Tone::Error
    }

    /// How long it stays, or `None` until it is resolved or dismissed.
    pub fn lifetime(&self) -> Option<Duration> {
        match self.tone {
            Tone::Error | Tone::Progress => None,
            _ if self.actionable() => Some(ACTION_LIFETIME),
            Tone::Warn => Some(WARN_LIFETIME),
            Tone::Done | Tone::Info => Some(CONFIRMATION_LIFETIME),
        }
    }

    /// A plain confirmation is 1; only it may interrupt a higher one.
    fn rank(&self) -> u8 {
        match self.tone {
            Tone::Error => 4,
            Tone::Progress => 3,
            _ if self.actionable() => 3,
            Tone::Warn => 2,
            Tone::Done | Tone::Info => 1,
        }
    }

    fn repeats(&self, earlier: &Self) -> bool {
        !self.actionable()
            && !earlier.actionable()
            && self.tone == earlier.tone
            && self.title == earlier.title
            && self.subject == earlier.subject
            && self.detail == earlier.detail
    }
}

#[derive(Debug, Clone)]
struct Shown {
    toast: Toast,
    /// Time left, or `None` for a toast that stays.
    left: Option<Duration>,
    /// When the clock last started, while it runs.
    since: Option<Instant>,
}

impl Shown {
    fn new(toast: Toast) -> Self {
        Self {
            left: toast.lifetime(),
            toast,
            since: None,
        }
    }

    fn stop(&mut self, now: Instant) {
        if let (Some(left), Some(since)) = (self.left, self.since.take()) {
            self.left = Some(left.saturating_sub(now.saturating_duration_since(since)));
        }
    }

    fn deadline(&self) -> Option<Instant> {
        Some(self.since? + self.left?)
    }
}

/// Why the clock is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// The pointer is over the toast.
    Hover,
    /// The window is not the active one.
    Away,
}

#[derive(Debug, Default)]
pub struct Toasts {
    current: Option<Shown>,
    /// Toasts a confirmation interrupted, highest rank first.
    held: Vec<Shown>,
    /// The toast that last ended with nothing after it, and when it ended.
    leaving: Option<(Toast, Instant)>,
    hover: bool,
    away: bool,
}

impl Toasts {
    pub fn current(&self) -> Option<&Toast> {
        self.current.as_ref().map(|shown| &shown.toast)
    }

    pub fn held(&self) -> impl Iterator<Item = &Toast> {
        self.held.iter().map(|shown| &shown.toast)
    }

    /// When the toast on screen expires, while its clock runs.
    pub fn deadline(&self) -> Option<Instant> {
        self.current.as_ref()?.deadline()
    }

    /// The toast still fading out after it ended, while nothing replaced it.
    pub fn leaving(&self) -> Option<&Toast> {
        self.leaving.as_ref().map(|(toast, _)| toast)
    }

    /// When something next changes on its own: an expiry or a finished exit.
    pub fn wake(&self) -> Option<Instant> {
        let left = self.leaving.as_ref().map(|(_, at)| *at + LEAVE);
        match (self.deadline(), left) {
            (Some(deadline), Some(left)) => Some(deadline.min(left)),
            (deadline, left) => deadline.or(left),
        }
    }

    fn paused(&self) -> bool {
        self.hover || self.away
    }

    /// Puts `shown` on screen, with its clock running unless paused.
    fn display(&mut self, mut shown: Shown, now: Instant) {
        // A new toast is a new element, which the pointer has not entered.
        self.hover = false;
        self.leaving = None;
        shown.since = (!self.paused()).then_some(now);
        self.current = Some(shown);
    }

    /// Shows a toast. A plain confirmation over anything else holds that one
    /// back to return afterwards; otherwise the new toast replaces what shows.
    /// The same message again restarts it and counts the repeat.
    pub fn show(&mut self, mut toast: Toast, now: Instant) {
        if let Some(shown) = &mut self.current
            && toast.repeats(&shown.toast)
        {
            shown.toast.times += 1;
            shown.toast.bump = toast.run;
            shown.left = shown.toast.lifetime();
            shown.since = (!(self.hover || self.away)).then_some(now);
            return;
        }
        // Only the newest change is what undo takes back.
        if toast.undo {
            self.held.retain(|held| !held.toast.undo);
        }
        if let Some(mut shown) = self.current.take()
            && toast.rank() == 1
            && shown.toast.rank() > 1
        {
            shown.stop(now);
            let at = self
                .held
                .iter()
                .position(|held| held.toast.rank() < shown.toast.rank())
                .unwrap_or(self.held.len());
            self.held.insert(at, shown);
        }
        toast.times = 1;
        self.display(Shown::new(toast), now);
    }

    /// Resolves the toast for work under way in place, keeping its entrance.
    /// With none left on screen, the result shows as a new toast.
    pub fn settle(&mut self, toast: Toast, now: Instant) {
        if let Some(shown) = self
            .current
            .as_mut()
            .filter(|shown| shown.toast.tone == Tone::Progress)
        {
            let run = shown.toast.run;
            *shown = Shown::new(Toast { run, ..toast });
            shown.since = (!(self.hover || self.away)).then_some(now);
            return;
        }
        self.held.retain(|held| held.toast.tone != Tone::Progress);
        self.show(toast, now);
    }

    /// Ends the toast on screen, letting the next held one return.
    pub fn dismiss(&mut self, now: Instant) -> Option<Toast> {
        let ended = self.current.take()?;
        if self.held.is_empty() {
            self.leaving = Some((ended.toast.clone(), now));
        } else {
            let mut next = self.held.remove(0);
            next.left = next.left.map(|left| left.max(RESUMED_LIFETIME));
            self.display(next, now);
        }
        Some(ended.toast)
    }

    /// Ends the toast on screen if its time is up, and forgets one that has
    /// finished fading out. Returns whether anything changed.
    pub fn expire(&mut self, now: Instant) -> bool {
        let faded = self.leaving.take_if(|(_, at)| *at + LEAVE <= now).is_some();
        if self.deadline().is_some_and(|deadline| deadline <= now) {
            self.dismiss(now);
            true
        } else {
            faded
        }
    }

    /// A later change means undo would take back something else.
    pub fn drop_undo(&mut self, now: Instant) {
        self.held.retain(|held| !held.toast.undo);
        if self.current().is_some_and(|toast| toast.undo) {
            self.dismiss(now);
        }
    }

    pub fn set_paused(&mut self, pause: Pause, on: bool, now: Instant) {
        let was = self.paused();
        match pause {
            Pause::Hover => self.hover = on,
            // An inactive window may not hear the pointer leave.
            Pause::Away => {
                self.away = on;
                self.hover &= !on;
            }
        }
        let paused = self.paused();
        if let Some(shown) = &mut self.current {
            if paused && !was {
                shown.stop(now);
            } else if was && !paused {
                shown.since = Some(now);
            }
        }
    }

    pub fn clear(&mut self) {
        self.current = None;
        self.held.clear();
        self.leaving = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Command;

    fn seconds(at: Instant, seconds: f32) -> Instant {
        at + Duration::from_secs_f32(seconds)
    }

    fn titled(toasts: &Toasts) -> Option<&str> {
        toasts.current().map(|toast| toast.title.as_str())
    }

    fn retry() -> Option<ToastAction> {
        Some(ToastAction::Retry(Command::Refresh))
    }

    #[test]
    fn each_kind_of_toast_stays_its_own_time() {
        assert_eq!(
            Toast::done("Copied").lifetime(),
            Some(CONFIRMATION_LIFETIME)
        );
        assert_eq!(
            Toast::info("No blocker").lifetime(),
            Some(CONFIRMATION_LIFETIME)
        );
        assert_eq!(
            Toast::warn("Invitation copied").lifetime(),
            Some(WARN_LIFETIME)
        );
        assert_eq!(
            Toast::done("Done").undo(true).lifetime(),
            Some(ACTION_LIFETIME)
        );
        let go = Toast::done("Created").action(Some(ToastAction::Refresh));
        assert_eq!(go.lifetime(), Some(ACTION_LIFETIME));
        assert_eq!(
            Toast::error("Could not save").action(retry()).lifetime(),
            None
        );
        assert_eq!(Toast::progress("Publishing").lifetime(), None);
    }

    #[test]
    fn a_shown_toast_expires_on_its_own() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.show(Toast::done("Copied"), now);
        assert!(!toasts.expire(seconds(now, 2.9)));
        assert!(toasts.expire(seconds(now, 3.0)));
        assert!(toasts.current().is_none());

        toasts.show(Toast::error("Could not save"), now);
        assert_eq!(toasts.deadline(), None);
        assert!(!toasts.expire(seconds(now, 600.)));
    }

    #[test]
    fn an_ended_toast_fades_out_unless_another_replaces_it() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.show(Toast::done("Copied"), now);
        assert!(toasts.expire(seconds(now, 3.0)));
        assert_eq!(
            toasts.leaving().map(|toast| toast.title.as_str()),
            Some("Copied")
        );
        assert_eq!(toasts.wake(), Some(seconds(now, 3.0) + LEAVE));
        assert!(toasts.expire(seconds(now, 3.0) + LEAVE));
        assert!(toasts.leaving().is_none());
        assert_eq!(toasts.wake(), None);

        toasts.show(Toast::done("Copied"), now);
        toasts.dismiss(now);
        toasts.show(Toast::info("No blocker"), now);
        assert!(toasts.leaving().is_none());
    }

    #[test]
    fn a_paused_clock_keeps_the_time_left() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.show(Toast::done("Copied"), now);
        toasts.set_paused(Pause::Hover, true, seconds(now, 2.));
        assert_eq!(toasts.deadline(), None);
        assert!(!toasts.expire(seconds(now, 10.)));
        toasts.set_paused(Pause::Away, true, seconds(now, 11.));
        toasts.set_paused(Pause::Hover, false, seconds(now, 12.));
        assert_eq!(toasts.deadline(), None, "the window is still away");
        toasts.set_paused(Pause::Away, false, seconds(now, 20.));
        assert_eq!(toasts.deadline(), Some(seconds(now, 21.)));
    }

    #[test]
    fn a_confirmation_interrupts_and_what_it_interrupted_returns() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.show(Toast::error("Could not save").action(retry()), now);
        toasts.show(Toast::done("Copied"), seconds(now, 1.));
        assert_eq!(titled(&toasts), Some("Copied"));
        assert!(toasts.expire(seconds(now, 4.)));
        assert_eq!(titled(&toasts), Some("Could not save"));
        assert_eq!(toasts.deadline(), None, "an error still stays");

        toasts.clear();
        toasts.show(Toast::done("Moved to Done").undo(true), now);
        toasts.show(Toast::done("Copied"), seconds(now, 7.));
        toasts.expire(seconds(now, 10.));
        assert_eq!(titled(&toasts), Some("Moved to Done"));
        assert_eq!(
            toasts.deadline(),
            Some(seconds(now, 10.) + RESUMED_LIFETIME),
            "one second was left, so it gets the minimum"
        );
    }

    #[test]
    fn anything_but_a_confirmation_replaces_what_shows() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.show(Toast::error("Could not save"), now);
        toasts.show(Toast::warn("Invitation copied"), now);
        toasts.dismiss(now);
        assert!(toasts.current().is_none());

        toasts.show(Toast::done("Copied"), now);
        toasts.show(Toast::done("Version copied"), now);
        toasts.dismiss(now);
        assert!(toasts.current().is_none(), "a confirmation is never held");
    }

    #[test]
    fn a_new_change_drops_an_older_undo() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.show(Toast::done("Moved to Done").undo(true), now);
        toasts.show(Toast::done("Copied"), now);
        toasts.show(Toast::done("Priority set to High").undo(true), now);
        assert_eq!(toasts.held().count(), 0);
        toasts.dismiss(now);
        assert!(toasts.current().is_none());

        toasts.show(Toast::done("Moved to Done").undo(true), now);
        toasts.show(Toast::done("Copied"), now);
        toasts.drop_undo(now);
        assert_eq!(toasts.held().count(), 0);
        toasts.show(Toast::done("Moved to Done").undo(true), now);
        toasts.drop_undo(now);
        assert!(toasts.current().is_none());
    }

    #[test]
    fn the_same_message_again_counts_and_restarts() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.show(
            Toast {
                run: 1,
                ..Toast::done("Copied")
            },
            now,
        );
        toasts.show(
            Toast {
                run: 2,
                ..Toast::done("Copied")
            },
            seconds(now, 2.),
        );
        toasts.show(
            Toast {
                run: 3,
                ..Toast::done("Copied")
            },
            seconds(now, 2.5),
        );
        let toast = toasts.current().unwrap();
        assert_eq!((toast.times, toast.run, toast.bump), (3, 1, 3));
        assert_eq!(toasts.deadline(), Some(seconds(now, 5.5)));

        toasts.show(Toast::done("Copied").undo(true), now);
        assert_eq!(
            toasts.current().unwrap().times,
            1,
            "an undo is a new change"
        );
    }

    #[test]
    fn work_under_way_resolves_in_place() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.show(
            Toast {
                run: 4,
                ..Toast::progress("Publishing secret gist")
            },
            now,
        );
        let link = Some(ToastAction::Open("https://gist".to_string()));
        toasts.settle(
            Toast {
                run: 9,
                ..Toast::done("Gist link copied").action(link)
            },
            now,
        );
        let toast = toasts.current().unwrap();
        assert_eq!((toast.title.as_str(), toast.run), ("Gist link copied", 4));
        assert_eq!(toasts.deadline(), Some(now + ACTION_LIFETIME));

        toasts.clear();
        toasts.show(Toast::progress("Publishing secret gist"), now);
        toasts.show(Toast::done("Copied"), now);
        toasts.settle(Toast::error("Could not publish"), now);
        toasts.dismiss(now);
        assert!(toasts.current().is_none(), "the held progress went with it");
    }
}
