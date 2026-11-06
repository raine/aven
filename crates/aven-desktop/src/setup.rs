//! Setup: before the database opens, makes sure this app is the one installation
//! that runs the `aven` command and the daemon. A terminal installation found
//! here is switched over only with the user's consent; until then the app
//! stays on this screen, so two versions never share the database.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aven_host::desktop_install::{
    self, Blocked, CASK_RELINK, FORMULA_UNINSTALL, Layout, Outcome, Plan, Report, Switch, System,
};
use gpui::{
    App, ClipboardItem, Context, Div, FocusHandle, FontWeight, KeyDownEvent, SharedString, Window,
    div, img, prelude::*,
};

use crate::kit::*;
use crate::notifications::{self, Permission};
use crate::theme::{Theme, theme};

const COLUMN: f32 = 500.;
const LOGO: f32 = 48.;

/// Where setup is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stage {
    Checking,
    MoveToApplications {
        running: PathBuf,
    },
    Confirm(Switch),
    Declined(Switch),
    Working,
    RemoveFormula {
        cask: bool,
        sync_paused: bool,
        still_installed: bool,
    },
    Blocked(Blocked),
    Failed(String),
    Done(Report),
}

type OnDone = Box<dyn FnOnce(&mut Window, &mut App)>;

pub(crate) struct Setup {
    stage: Stage,
    layout: Layout,
    system: Arc<dyn System>,
    on_done: Option<OnDone>,
    /// The command last copied, so its button can say so.
    copied: Option<SharedString>,
    focus: FocusHandle,
    /// The config turns reminders on, so the agent sends them too.
    reminders: bool,
    /// Whether macOS lets Aven show notifications, read before the switch
    /// screen shows.
    permission: Permission,
}

impl Setup {
    pub(crate) fn new(
        layout: Layout,
        system: Arc<dyn System>,
        on_done: OnDone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut setup = Self {
            stage: Stage::Checking,
            layout,
            system,
            on_done: Some(on_done),
            copied: None,
            focus,
            reminders: aven_host::config::AppConfig::load()
                .is_ok_and(|config| config.reminders.enabled),
            permission: Permission::Unavailable,
        };
        setup.check(window, cx);
        setup
    }

    /// Surveys the installation and moves to what it calls for. A switch the
    /// user already agreed to carries on without asking again.
    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_waiting = matches!(self.stage, Stage::RemoveFormula { .. });
        self.stage = Stage::Checking;
        self.copied = None;
        cx.notify();
        let layout = self.layout.clone();
        let system = self.system.clone();
        cx.spawn_in(window, async move |this, cx| {
            let stage = cx
                .background_spawn(async move { checked(layout, system.as_ref(), was_waiting) })
                .await;
            let permission = if matches!(stage, Some(Stage::Confirm(_))) {
                Some(notifications::permission().await)
            } else {
                None
            };
            this.update_in(cx, |this, window, cx| {
                if let Some(permission) = permission {
                    this.permission = permission;
                }
                this.arrive(stage, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn agree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Stage::Confirm(switch) | Stage::Declined(switch)) = &self.stage else {
            return;
        };
        let switch = switch.clone();
        // The daemon keeps sending reminders from this app, which only it can
        // ask macOS to allow. macOS asks only while the choice is undecided.
        if self.reminders
            && switch.agent.as_ref().is_some_and(|agent| !agent.disabled)
            && !matches!(self.permission, Permission::Allowed | Permission::Denied)
        {
            cx.spawn(async move |_, _| {
                notifications::request().await;
            })
            .detach();
        }
        self.stage = Stage::Working;
        cx.notify();
        let layout = self.layout.clone();
        let system = self.system.clone();
        cx.spawn_in(window, async move |this, cx| {
            let stage = cx
                .background_spawn(async move {
                    let layout = desktop_install::with_shell_path(layout);
                    switched(
                        desktop_install::switch(&layout, system.as_ref(), &switch),
                        false,
                    )
                })
                .await;
            this.update_in(cx, |this, window, cx| this.arrive(stage, window, cx))
                .ok();
        })
        .detach();
    }

    fn arrive(&mut self, stage: Option<Stage>, window: &mut Window, cx: &mut Context<Self>) {
        match stage {
            Some(stage) => {
                self.stage = stage;
                cx.notify();
            }
            None => self.open(window, cx),
        }
    }

    fn decline(&mut self, cx: &mut Context<Self>) {
        if let Stage::Confirm(switch) = &self.stage {
            self.stage = Stage::Declined(switch.clone());
            cx.notify();
        }
    }

    /// Back to the switch screen, with the permission read again since the
    /// user may have changed it meanwhile.
    fn review(&mut self, cx: &mut Context<Self>) {
        let Stage::Declined(switch) = &self.stage else {
            return;
        };
        let switch = switch.clone();
        cx.spawn(async move |this, cx| {
            let permission = notifications::permission().await;
            this.update(cx, |this, cx| {
                if matches!(this.stage, Stage::Declined(_)) {
                    this.permission = permission;
                    this.stage = Stage::Confirm(switch);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(on_done) = self.on_done.take() {
            on_done(window, cx);
        }
    }

    fn copy(&mut self, command: SharedString, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
        self.copied = Some(command);
        cx.notify();
    }

    /// Return runs the screen's main button.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "enter" || event.keystroke.modifiers.modified() {
            return;
        }
        match &self.stage {
            Stage::Confirm(_) => self.agree(window, cx),
            Stage::Declined(_) => self.review(cx),
            Stage::RemoveFormula { .. } | Stage::Blocked(_) | Stage::Failed(_) => {
                self.check(window, cx)
            }
            Stage::Done(_) => self.open(window, cx),
            Stage::MoveToApplications { .. } => cx.quit(),
            Stage::Checking | Stage::Working => return,
        }
        cx.stop_propagation();
    }
}

/// The stage a survey leads to, or `None` to open the app straight away.
fn checked(layout: Layout, system: &dyn System, was_waiting: bool) -> Option<Stage> {
    let layout = desktop_install::with_shell_path(layout);
    let survey = match desktop_install::survey(&layout, system) {
        Ok(survey) => survey,
        Err(error) => return Some(Stage::Failed(format!("{error:#}"))),
    };
    match desktop_install::plan(&layout, &survey) {
        Plan::Ready => match desktop_install::complete(&layout, system) {
            Ok(report) if report.needs_attention() => Some(Stage::Done(report)),
            Ok(_) => None,
            Err(error) => Some(Stage::Failed(format!("{error:#}"))),
        },
        Plan::MoveToApplications { running } => Some(Stage::MoveToApplications { running }),
        Plan::Blocked(blocked) => Some(Stage::Blocked(blocked)),
        Plan::Switch(switch) if switch.resuming => switched(
            desktop_install::switch(&layout, system, &switch),
            was_waiting,
        ),
        Plan::Switch(switch) => Some(Stage::Confirm(switch)),
    }
}

fn switched(outcome: anyhow::Result<Outcome>, was_waiting: bool) -> Option<Stage> {
    Some(match outcome {
        Ok(Outcome::AwaitFormula { cask, sync_paused }) => Stage::RemoveFormula {
            cask,
            sync_paused,
            still_installed: was_waiting,
        },
        Ok(Outcome::Done(report)) => Stage::Done(report),
        Err(error) => Stage::Failed(format!("{error:#}")),
    })
}

/// What the switch screen says about notifications, for reminders the
/// daemon goes on sending from this app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NotificationLine {
    text: &'static str,
    /// Whether the line offers System Settings, where a denied permission is
    /// changed: macOS doesn't ask again.
    settings: bool,
}

fn notification_line(permission: Permission) -> Option<NotificationLine> {
    let (text, settings) = match permission {
        Permission::Allowed => ("Notifications from Aven are on", false),
        Permission::NotAsked => ("macOS asks you to allow notifications from Aven", false),
        Permission::Denied => (
            "Notifications are off for Aven, so reminders don\u{2019}t appear until you \
             allow them in System Settings",
            true,
        ),
        Permission::Unavailable => return None,
    };
    Some(NotificationLine { text, settings })
}

/// `path` with the home directory as `~`.
fn shown(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// A path or command quoted for the shell.
fn quoted(path: &Path) -> String {
    let text = path.display().to_string();
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c))
    {
        text
    } else {
        format!("\"{}\"", text.replace('"', "\\\""))
    }
}

/// What a screen shows.
struct Screen {
    title: &'static str,
    body: Vec<Div>,
    primary: Option<(&'static str, Button)>,
    secondary: Option<(&'static str, Button)>,
    note: Option<SharedString>,
}

#[derive(Clone, Copy)]
enum Button {
    Agree,
    Decline,
    Review,
    Check,
    Open,
    Quit,
}

impl Setup {
    fn screen(&self, cx: &mut Context<Self>, t: &Theme) -> Screen {
        let home = self.layout.home.clone();
        let exe = self.layout.executable();
        let at = |path: &Path| shown(path, &home);
        match &self.stage {
            Stage::Checking => Screen {
                title: "Setting up Aven",
                body: vec![lead(
                    "Checking how Aven is installed on this Mac\u{2026}",
                    t,
                )],
                primary: None,
                secondary: None,
                note: None,
            },
            Stage::Working => Screen {
                title: "Switching to Aven Desktop",
                body: vec![lead(
                    "Moving the aven command and background sync to this app. Your tasks \
                     aren\u{2019}t touched.",
                    t,
                )],
                primary: None,
                secondary: None,
                note: None,
            },
            Stage::MoveToApplications { running } => Screen {
                title: "Move Aven to Applications",
                body: vec![
                    lead(
                        "Aven runs the aven command and background sync from the copy in \
                         Applications. Quit, drag Aven into the Applications folder, and open \
                         it from there.",
                        t,
                    ),
                    card(t).child(row(
                        "This copy",
                        at(running
                            .parent()
                            .and_then(Path::parent)
                            .and_then(Path::parent)
                            .unwrap_or(running)),
                        true,
                        t,
                    )),
                ],
                primary: Some(("Quit Aven", Button::Quit)),
                secondary: None,
                note: Some("Nothing has been changed.".into()),
            },
            Stage::Confirm(switch) => Screen {
                title: "Switch to Aven Desktop",
                body: self.confirm_body(switch, t),
                primary: Some(("Switch to Desktop", Button::Agree)),
                secondary: Some(("Not now", Button::Decline)),
                note: None,
            },
            Stage::Declined(_) => Screen {
                title: "Aven Desktop isn\u{2019}t set up",
                body: vec![lead(
                    "Nothing was changed. Desktop opens your tasks once it runs the aven \
                     command and background sync, so two versions never use them at once. \
                     Until then, keep using Aven in the terminal.",
                    t,
                )],
                primary: Some(("Review the switch", Button::Review)),
                secondary: Some(("Quit Aven", Button::Quit)),
                note: None,
            },
            Stage::RemoveFormula {
                cask,
                sync_paused,
                still_installed,
            } => {
                let mut steps = card(t)
                    .gap(px(10.))
                    .child(step_label("Run this in Terminal:", t))
                    .child(self.command(FORMULA_UNINSTALL, cx, t));
                if *cask {
                    steps = steps
                        .child(step_label(
                            "Then link the command installed with the app:",
                            t,
                        ))
                        .child(self.command(CASK_RELINK, cx, t));
                }
                Screen {
                    title: "Remove the Homebrew formula",
                    body: vec![
                        lead(
                            if *sync_paused {
                                "Background sync is paused and starts again from this app once \
                                 the formula is gone. Aven leaves Homebrew\u{2019}s files to \
                                 Homebrew."
                            } else {
                                "This app takes over once the formula is gone. Aven leaves \
                                 Homebrew\u{2019}s files to Homebrew."
                            },
                            t,
                        ),
                        steps,
                    ],
                    primary: Some(("Check again", Button::Check)),
                    secondary: Some(("Quit Aven", Button::Quit)),
                    note: Some(if *still_installed {
                        "The formula is still installed.".into()
                    } else {
                        "You can quit and finish later. Aven continues from here.".into()
                    }),
                }
            }
            Stage::Blocked(blocked) => {
                let (title, body) = self.blocked(blocked, &exe, cx, t);
                Screen {
                    title,
                    body,
                    primary: Some(("Check again", Button::Check)),
                    secondary: Some(("Quit Aven", Button::Quit)),
                    note: Some("Nothing has been changed.".into()),
                }
            }
            Stage::Failed(error) => Screen {
                title: "The switch didn\u{2019}t finish",
                body: vec![
                    lead(
                        "The old installation stays until everything checks out. Try again to \
                         continue from where it stopped.",
                        t,
                    ),
                    card(t).child(
                        small(Text::Secondary)
                            .font_family(MONO)
                            .text_color(t.bad)
                            .child(error.clone()),
                    ),
                ],
                primary: Some(("Try again", Button::Check)),
                secondary: Some(("Quit Aven", Button::Quit)),
                note: None,
            },
            Stage::Done(report) => Screen {
                title: if report.switched {
                    "Aven Desktop is ready"
                } else {
                    "The aven command is ready"
                },
                body: self.done_body(report, cx, t),
                primary: Some(("Open Aven", Button::Open)),
                secondary: None,
                note: None,
            },
        }
    }

    fn confirm_body(&self, switch: &Switch, t: &Theme) -> Vec<Div> {
        let home = &self.layout.home;
        let source = match (switch.script, switch.formula) {
            (true, true) => "with the install script and with Homebrew",
            (false, true) => "with Homebrew",
            _ => "with the install script",
        };
        let mut changes = card(t).gap(px(10.)).child(section("What changes", t));
        let commands = if switch.repoint.is_empty() {
            "The aven command runs this app".to_string()
        } else {
            let paths: Vec<String> = switch
                .repoint
                .iter()
                .map(|path| shown(path, home))
                .collect();
            format!("{} runs this app", paths.join(", "))
        };
        changes = changes.child(item("icons/ic-return.svg", commands, t));
        if let Some(agent) = &switch.agent {
            let work = if self.reminders {
                "Background sync and reminders"
            } else {
                "Background sync"
            };
            let state = if agent.disabled {
                format!("{work} move to this app and stay off")
            } else if agent.loaded {
                format!("{work} restart from this app and continue while it is quit")
            } else {
                format!("{work} move to this app and continue while it is quit")
            };
            changes = changes.child(item("icons/cloud.svg", state, t));
            if self.reminders
                && !agent.disabled
                && let Some(line) = notification_line(self.permission)
            {
                changes = changes.child(item("icons/ic-done.svg", line.text.to_string(), t));
                if line.settings {
                    changes = changes.child(
                        div().pl(px(26.)).flex().child(
                            line_btn(
                                "setup-notification-settings",
                                "Open Notification Settings",
                                t,
                            )
                            .h(px(28.))
                            .px(px(12.))
                            .on_click(|_, _, cx| cx.open_url(notifications::SETTINGS_URL)),
                        ),
                    );
                }
            }
        }
        if let Some(bundle) = &switch.retire {
            changes = changes.child(item(
                "icons/ic-trash.svg",
                format!(
                    "The old copy at {} is removed once everything checks out",
                    shown(bundle, home)
                ),
                t,
            ));
        }
        if switch.formula {
            changes = changes.child(item(
                "icons/ic-trash.svg",
                "You remove the Homebrew formula with one Terminal command".to_string(),
                t,
            ));
        }
        let kept = card(t)
            .gap(px(10.))
            .child(section("What stays", t))
            .child(item(
                "icons/ic-done.svg",
                "Your tasks, attachments, sync setup and daemon settings".to_string(),
                t,
            ));
        vec![
            lead(
                &format!(
                    "Aven is already installed {source}. Desktop takes over the aven command \
                     and background sync, so the app, the command and the daemon always run \
                     the same version."
                ),
                t,
            ),
            changes,
            kept,
        ]
    }

    fn blocked(
        &self,
        blocked: &Blocked,
        exe: &Path,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> (&'static str, Vec<Div>) {
        let home = &self.layout.home;
        let paths = |paths: &[PathBuf]| {
            paths.iter().fold(card(t).gap(px(6.)), |card, path| {
                card.child(mono(shown(path, home), t))
            })
        };
        match blocked {
            Blocked::UnknownAven { paths: found } => (
                "Another copy of Aven is installed",
                vec![
                    lead(
                        "These are Aven builds that neither the install script nor Homebrew put \
                         there, so Setup leaves them alone. Remove them or take them off your \
                         PATH, then check again.",
                        t,
                    ),
                    paths(found),
                ],
            ),
            Blocked::UnknownAgent { plist, program } => {
                let repair = format!("{} daemon repair", quoted(exe));
                let what = match program {
                    Some(program) => format!(
                        "Background sync runs {}, which Setup doesn\u{2019}t recognize.",
                        shown(program, home)
                    ),
                    None => format!(
                        "Setup can\u{2019}t read the background sync agent at {}.",
                        shown(plist, home)
                    ),
                };
                (
                    "Background sync needs moving by hand",
                    vec![
                        lead(&what, t),
                        card(t)
                            .gap(px(10.))
                            .child(step_label(
                                "To run it from this app instead, run this in Terminal:",
                                t,
                            ))
                            .child(self.command(&repair, cx, t)),
                    ],
                )
            }
            Blocked::Unwritable { paths: found } => {
                let mut fixes = card(t).gap(px(10.)).child(step_label(
                    "Setup can\u{2019}t change these. Run this in Terminal:",
                    t,
                ));
                for path in found {
                    let fix = if path.file_name().is_some_and(|name| name == "aven") {
                        format!("sudo ln -sfn {} {}", quoted(exe), quoted(path))
                    } else {
                        format!("sudo chown \"$USER\" {}", quoted(path))
                    };
                    fixes = fixes.child(self.command(&fix, cx, t));
                }
                (
                    "Aven needs permission to switch",
                    vec![
                        lead(
                            "Part of the existing installation belongs to another user.",
                            t,
                        ),
                        fixes,
                    ],
                )
            }
            Blocked::UnknownState { file, error } => (
                "Setup found a record it can\u{2019}t read",
                vec![
                    lead(
                        &format!(
                            "{} should describe a switch in progress, but Setup can\u{2019}t \
                             read it, so it stops instead of guessing. If you aren\u{2019}t \
                             switching, delete the file and check again.",
                            shown(file, home)
                        ),
                        t,
                    ),
                    card(t).child(
                        small(Text::Secondary)
                            .font_family(MONO)
                            .text_color(t.muted)
                            .child(error.clone()),
                    ),
                ],
            ),
        }
    }

    fn done_body(&self, report: &Report, cx: &mut Context<Self>, t: &Theme) -> Vec<Div> {
        let home = &self.layout.home;
        let mut body = Vec::new();
        if report.switched {
            body.push(lead(
                "The app, the aven command and background sync now run the same version. Your \
                 tasks and sync setup are as you left them.",
                t,
            ));
        } else {
            body.push(lead(
                "Use aven in the terminal for the same tasks, from the same version as the app.",
                t,
            ));
        }
        let mut facts = card(t).gap(px(10.));
        facts = facts.child(row(
            "Command",
            report
                .command
                .as_ref()
                .map(|path| shown(path, home))
                .unwrap_or_else(|| "Not linked".to_string()),
            report.command.is_some(),
            t,
        ));
        if let Some(agent) = report.agent {
            let state = match (agent.running, agent.disabled) {
                (true, _) => "Running from this app",
                (false, true) => "Off, as before",
                (false, false) => "Starts with this app",
            };
            facts = facts.child(row("Background sync", state.to_string(), false, t));
        }
        body.push(facts);
        if let Some(dir) = &report.add_to_path {
            let entry = shown(dir, home).replacen('~', "$HOME", 1);
            body.push(
                card(t)
                    .gap(px(10.))
                    .child(warning(
                        &format!(
                            "{} isn\u{2019}t on your PATH. Add this line to your shell \
                             profile, such as ~/.zshrc:",
                            shown(dir, home)
                        ),
                        t,
                    ))
                    .child(self.command(&format!("export PATH=\"{entry}:$PATH\""), cx, t)),
            );
        }
        if !report.shadowed_by.is_empty() {
            let mut card = card(t).gap(px(8.)).child(warning(
                "These run instead when you type aven. Remove them or put the command\u{2019}s \
                 folder first on your PATH:",
                t,
            ));
            for path in &report.shadowed_by {
                card = card.child(mono(shown(path, home), t));
            }
            body.push(card);
        }
        if report.cask_unlinked {
            body.push(
                card(t)
                    .gap(px(10.))
                    .child(warning(
                        "Homebrew hasn\u{2019}t linked the command installed with the app. \
                         Run this in Terminal:",
                        t,
                    ))
                    .child(self.command(CASK_RELINK, cx, t)),
            );
        } else if report.command.is_none() {
            let others: Vec<String> = report.others.iter().map(|path| shown(path, home)).collect();
            body.push(card(t).child(warning(
                &format!(
                    "{} is another program, so Setup left it alone.",
                    others.join(", ")
                ),
                t,
            )));
        }
        if let Some(bundle) = &report.kept_bundle {
            body.push(card(t).child(warning(
                &format!(
                    "{} isn\u{2019}t Aven, so it was left in place.",
                    shown(bundle, home)
                ),
                t,
            )));
        }
        body
    }

    /// A command to run in Terminal, with a button that copies it.
    fn command(&self, command: &str, cx: &mut Context<Self>, t: &Theme) -> Div {
        let command: SharedString = command.to_string().into();
        let copied = self.copied.as_ref() == Some(&command);
        let raised = t.raised;
        let label = if copied { "Copied" } else { "Copy" };
        div()
            .w_full()
            .min_h(px(36.))
            .pl(px(12.))
            .pr(px(4.))
            .py(px(4.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(RADIUS_CONTROL))
            .bg(t.page)
            .shadow(vec![t.inset_ring(t.line)])
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(MONO)
                    .text(Text::Secondary)
                    .text_color(t.ink)
                    .child(command.clone()),
            )
            .child(
                bare_btn(SharedString::from(format!("copy-{command}")), label, t)
                    .text_color(if copied { t.good } else { t.muted })
                    .hover(move |style| style.bg(raised))
                    .child(icon(
                        "icons/ic-copy.svg",
                        if copied { t.good } else { t.muted },
                    ))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(command.clone(), cx))),
            )
    }

    fn button(
        &self,
        label: &'static str,
        action: Button,
        primary: bool,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> impl IntoElement + use<> {
        let id = SharedString::from(format!("setup-{label}"));
        let button = if primary {
            fill_btn(id, label, t).child(kbd("↵", t.on_fill))
        } else {
            line_btn(id, label, t)
        };
        button.h(px(32.)).px(px(14.)).on_click(cx.listener(
            move |this, _, window, cx| match action {
                Button::Agree => this.agree(window, cx),
                Button::Decline => this.decline(cx),
                Button::Review => this.review(cx),
                Button::Check => this.check(window, cx),
                Button::Open => this.open(window, cx),
                Button::Quit => cx.quit(),
            },
        ))
    }
}

impl Render for Setup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let screen = self.screen(cx, &t);
        let mut buttons = div().mt(px(24.)).flex().items_center().gap(px(10.));
        if let Some((label, action)) = screen.secondary {
            buttons = buttons.child(self.button(label, action, false, cx, &t));
        }
        if let Some((label, action)) = screen.primary {
            buttons = buttons.child(self.button(label, action, true, cx, &t));
        }
        let column = div()
            .w(px(COLUMN))
            .flex()
            .flex_col()
            .items_center()
            .child(
                img(if t.dark {
                    "logo-dark.svg"
                } else {
                    "logo-light.svg"
                })
                .size(px(LOGO))
                .mb(px(18.)),
            )
            .child(
                div()
                    .text(Text::Display)
                    .line_height(px(31.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_center()
                    .child(screen.title),
            )
            .child(
                div()
                    .w_full()
                    .mt(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .children(screen.body),
            )
            .child(buttons)
            .children(screen.note.map(|note| {
                small(Text::Secondary)
                    .mt(px(12.))
                    .text_color(t.faint)
                    .child(note)
            }));
        // The title bar is transparent, so the strip under it moves the window.
        let title_bar = div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(40.))
            .on_mouse_down(gpui::MouseButton::Left, drag_window);
        div()
            .id("setup")
            .relative()
            .size_full()
            .bg(t.page)
            .text_color(t.ink)
            .font_family(".SystemUIFont")
            .text(Text::Body)
            .line_height(px(UI_LINE_HEIGHT))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .overflow_y_scroll()
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px(px(24.))
                    .pt(px(48.))
                    .pb(px(40.))
                    .child(column),
            )
            .child(title_bar)
    }
}

/// The screen's explanation, under its title.
fn lead(text: &str, t: &Theme) -> Div {
    div()
        .w_full()
        .mb(px(6.))
        .text(Text::Body)
        .line_height(px(20.))
        .text_center()
        .text_color(t.muted)
        .child(text.to_string())
}

fn card(t: &Theme) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .px(px(16.))
        .py(px(14.))
        .rounded(px(RADIUS_SURFACE))
        .bg(t.panel)
        .shadow(vec![t.inset_ring(t.line)])
}

fn section(text: &'static str, t: &Theme) -> Div {
    small(Text::Caption)
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.faint)
        .child(text)
}

fn step_label(text: &'static str, t: &Theme) -> Div {
    small(Text::Secondary).text_color(t.body).child(text)
}

fn item(path: &'static str, text: String, t: &Theme) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(div().pt(px(1.)).child(icon(path, t.muted)))
        .child(div().flex_1().min_w_0().text_color(t.body).child(text))
}

/// A labelled fact. A path value is set in the monospaced face.
fn row(label: &'static str, value: String, path: bool, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(div().flex_none().text_color(t.muted).child(label))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(t.ink)
                .when(path, |value| value.font_family(MONO).text(Text::Secondary))
                .child(value),
        )
}

fn mono(text: String, t: &Theme) -> Div {
    div()
        .font_family(MONO)
        .text(Text::Secondary)
        .text_color(t.ink)
        .child(text)
}

fn warning(text: &str, t: &Theme) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(div().pt(px(1.)).child(icon("icons/toast-warn.svg", t.warn)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(t.body)
                .child(text.to_string()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_under_home_are_shown_with_a_tilde() {
        let home = Path::new("/Users/me");
        assert_eq!(
            shown(Path::new("/Users/me/.local/bin/aven"), home),
            "~/.local/bin/aven"
        );
        assert_eq!(
            shown(Path::new("/opt/homebrew/bin/aven"), home),
            "/opt/homebrew/bin/aven"
        );
    }

    #[test]
    fn paths_with_spaces_are_quoted_for_the_shell() {
        assert_eq!(
            quoted(Path::new("/Applications/Aven.app/Contents/MacOS/aven")),
            "/Applications/Aven.app/Contents/MacOS/aven"
        );
        assert_eq!(quoted(Path::new("/Users/a b/aven")), "\"/Users/a b/aven\"");
    }

    #[test]
    fn the_notification_line_follows_the_permission() {
        let allowed = notification_line(Permission::Allowed).unwrap();
        assert!(!allowed.text.contains("asks") && !allowed.settings);
        let not_asked = notification_line(Permission::NotAsked).unwrap();
        assert!(not_asked.text.starts_with("macOS asks") && !not_asked.settings);
        let denied = notification_line(Permission::Denied).unwrap();
        assert!(denied.text.contains("off for Aven") && denied.settings);
        assert_eq!(notification_line(Permission::Unavailable), None);
    }
}
