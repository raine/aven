//! The settings surface, the Sync dialog and data recovery.
use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use accesskit::Role;
use aven_core::sync::client::engine::Stage;
use aven_core::sync::client::invitation::InvitationCheck;
use aven_host::config::config_file_path;
use aven_host::store::TaskQuery;
use chrono::Local;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, ClipboardItem, Context, Div, Entity,
    FocusHandle, FontWeight, HighlightStyle, Hsla, SharedString, Stateful, StyledText,
    Transformation, Window, canvas, div, fill, point, prelude::*, radians, size, svg,
};
use qrcode::{Color, EcLevel, QrCode};

use crate::actions::*;
use crate::adapter::{
    AGENT_TEST_NOTE, AgentSetup, AgentShown, Command, NewTask, ProjectEntry, Reminders,
    SettingsEdit, SyncPhase, ThemePref, capitalised,
};
use crate::dialogs::{ConfirmAction, Dialog, Layer};
use crate::input::{Editor, Mode};
use crate::kit::*;
use crate::notifications::{self, Permission};
use crate::session::{AgentTestState, SettingsTab, SyncPage, SyncTone, ago, count_text};
use crate::shell::Root;
use crate::sync_ops::{Secret, SetupFacts, SyncKind, SyncOp, SyncRequest, steps};
use crate::theme::{Theme, theme};
use crate::toast::Toast;
use crate::workflows::tab_stop;

const TABS: [(SettingsTab, &str); 10] = [
    (SettingsTab::General, "General"),
    (SettingsTab::QuickCapture, "Quick capture"),
    (SettingsTab::Sync, "Sync and devices"),
    (SettingsTab::Reminders, "Reminders"),
    (SettingsTab::Workspaces, "Workspaces"),
    (SettingsTab::Projects, "Projects"),
    (SettingsTab::Labels, "Labels"),
    (SettingsTab::Fields, "Metadata fields"),
    (SettingsTab::Commands, "Custom commands"),
    (SettingsTab::Data, "Data and recovery"),
];

const CLOUD_URL: &str = "https://cloud.aventasks.dev";
const LOGIN_ITEMS_URL: &str = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";
const AGENT_DOCS_URL: &str = "https://aventasks.dev/configuration/#agent-task-intake";
const QR_SIZE: f32 = 184.;

fn row_title(title: impl Into<SharedString>, note: Option<String>, t: &Theme) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .child(title.into())
        .when_some(note, |row, note| {
            row.child(
                small(Text::Secondary)
                    .mt(px(1.))
                    .text_color(t.muted)
                    .child(note),
            )
        })
}

fn path_title(title: &'static str, path: String, t: &Theme) -> Div {
    div().flex_1().min_w_0().child(title).child(
        small(Text::Caption)
            .mt(px(1.))
            .font_family(MONO)
            .text_color(t.muted)
            .child(path),
    )
}

fn row(t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(48.))
        .pl(px(14.))
        .pr(px(12.))
        .py(px(8.))
        .text_color(t.ink)
}

/// Rows joined into one panel, with lines between them.
fn group(rows: Vec<Div>, t: &Theme) -> Div {
    let line = t.line;
    div()
        .relative()
        .rounded(px(RADIUS_SURFACE))
        .bg(t.panel)
        .overflow_hidden()
        .children(rows.into_iter().enumerate().map(|(index, row)| {
            div().relative().child(row).when(index > 0, |wrap| {
                wrap.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(px(1.))
                        .bg(line),
                )
            })
        }))
        .child(
            div()
                .absolute()
                .size_full()
                .top_0()
                .left_0()
                .rounded(px(RADIUS_SURFACE))
                .border_1()
                .border_color(t.line),
        )
}

fn section(text: &'static str, t: &Theme) -> Div {
    small(Text::Secondary)
        .pt(px(20.))
        .pb(px(7.))
        .px(px(2.))
        .text_color(t.muted)
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

fn page(title: &'static str, lead: Option<&'static str>, t: &Theme) -> Div {
    div()
        .child(
            div()
                .mb(px(4.))
                .text(Text::Heading)
                .line_height(px(26.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.ink)
                .child(title),
        )
        .when_some(lead, |page, lead| {
            page.child(div().mb(px(20.)).text_color(t.muted).child(lead))
        })
}

/// A page's lead when it depends on state.
fn lead(text: impl Into<SharedString>, t: &Theme) -> Div {
    div().mb(px(20.)).text_color(t.muted).child(text.into())
}

/// What the Reminders page says about when reminders arrive.
fn reminders_lead(enabled: bool, agent: AgentShown) -> &'static str {
    match (enabled, agent) {
        (false, _) => "Notifications on this Mac when tasks come due or become available again.",
        (true, AgentShown::Running) => {
            "Sent while Aven is open or quit. Each device keeps its own record of sent reminders."
        }
        (true, AgentShown::Unknown) => "Each device keeps its own record of sent reminders.",
        (true, _) => {
            "Sent only while Aven is open. Each device keeps its own record of sent reminders."
        }
    }
}

/// What the Notifications row says about notifications that weren't shown
/// while notifications were off, if any were.
fn missed_note(missed: u64) -> Option<String> {
    match missed {
        0 => None,
        1 => Some("1 notification this week wasn't shown".to_string()),
        count => Some(format!("{count} notifications this week weren't shown")),
    }
}

/// A config file block, set in the code face.
fn command_block(t: &Theme) -> Div {
    small(Text::Secondary)
        .px(px(10.))
        .py(px(8.))
        .rounded(px(RADIUS_CONTROL))
        .bg(t.page)
        .shadow(vec![t.inset_ring(t.line)])
        .font_family(MONO)
        .text_color(t.body)
        .whitespace_normal()
}

fn status_pill(label: &'static str, tone: Option<Hsla>, t: &Theme) -> Div {
    small(Text::Caption)
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .px(px(8.))
        .rounded_full()
        .bg(t.raised)
        .text_color(tone.unwrap_or(t.muted))
        .when_some(tone, |pill, tone| {
            pill.child(div().size(px(6.)).rounded_full().bg(tone))
        })
        .child(label)
}

fn open_config_btn(t: &Theme) -> Stateful<Div> {
    line_btn("agent-open-config", "Open config", t).on_click(|_, _, cx| {
        if let Ok(path) = config_file_path() {
            if path.exists() {
                cx.open_with_system(&path);
            } else if let Some(dir) = path.parent() {
                cx.open_with_system(dir);
            }
        }
    })
}

/// The config file's path, with the home directory as `~`.
fn config_path_text() -> String {
    let Ok(path) = config_file_path() else {
        return "config.yaml".to_string();
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|home| path.strip_prefix(home).ok().map(PathBuf::from)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

const AGENT_SNIPPET: &str = "agent:\n  task_intake:\n    command: claude\n    image_args: \
                             [\"--add-dir\", \"{image_dir}\"]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Command,
    Placeholder,
}

/// The agent's command line as one string, with the command and each
/// `{placeholder}` marked.
fn command_line(setup: &AgentSetup) -> (String, Vec<(Range<usize>, Part)>) {
    let mut line = setup.command.clone();
    let mut spans = vec![(0..line.len(), Part::Command)];
    for arg in &setup.args {
        line.push(' ');
        let start = line.len();
        line.push_str(arg);
        let mut from = 0;
        while let Some(open) = arg[from..].find('{') {
            let open = from + open;
            let Some(close) = arg[open..].find('}') else {
                break;
            };
            let close = open + close + 1;
            spans.push((start + open..start + close, Part::Placeholder));
            from = close;
        }
    }
    (line, spans)
}

/// The fields of a test task worth showing beside its title.
fn test_chips(task: &NewTask, projects: &[ProjectEntry]) -> Vec<String> {
    let mut chips = Vec::new();
    if let Some(key) = &task.project {
        let name = projects
            .iter()
            .find(|project| &project.key == key)
            .map_or(key.as_str(), |project| project.name.as_str());
        chips.push(name.to_string());
    }
    if !task.schedule.trim().is_empty() {
        chips.push(task.schedule.trim().to_string());
    }
    if !task.priority.is_empty() && task.priority != "none" {
        chips.push(format!("! {}", capitalised(&task.priority)));
    }
    chips.extend(task.labels.iter().map(|label| format!("#{label}")));
    chips
}

fn tool_row(child: impl IntoElement) -> Div {
    div().mt(px(10.)).flex().gap(px(6.)).child(child)
}

pub(crate) fn sync_icon(t: &Theme, tone: SyncTone) -> Hsla {
    match tone {
        SyncTone::Plain => t.faint,
        SyncTone::Good => t.good,
        SyncTone::Busy => t.accent,
        SyncTone::Warn => t.warn,
        SyncTone::Bad => t.bad,
    }
}

impl Root {
    /// Quits, saying the first time that reminders and sync keep running in
    /// the background, as Quit from the menu or keyboard does. The notice
    /// reads the notification permission first, so it never promises
    /// reminders macOS won't show.
    pub(crate) fn ask_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let background = &self.session.settings_data.background;
        let running = background.managed && background.wanted;
        if !running || self.prefs.quit_notice_shown {
            self.quit(window, cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let permission = notifications::permission().await;
            this.update_in(cx, |this, window, cx| {
                this.quit_with_notice(permission, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn quit_with_notice(
        &mut self,
        permission: Permission,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let background = &self.session.settings_data.background;
        let notice = quit_notice(background.reminders, background.sync_when_quit, permission);
        let Some(notice) = notice.filter(|_| !self.prefs.quit_notice_shown) else {
            self.quit(window, cx);
            return;
        };
        self.prefs.quit_notice_shown = true;
        self.save_prefs();
        self.open_confirm(
            notice.title.to_string(),
            notice.body.to_string(),
            Some("This notice appears only once."),
            "Quit",
            ConfirmAction::Quit {
                notification_settings: notice.notification_settings,
            },
            window,
            cx,
        );
    }

    pub fn show_settings(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        self.session.open_settings(tab);
        self.read_notifications(cx);
        cx.notify();
    }

    /// Reads what settings show from outside Aven again, as when the user
    /// returns from System Settings.
    pub(crate) fn refresh_settings(&mut self, cx: &mut Context<Self>) {
        if self.session.settings.is_some() {
            self.session.reload_settings();
            self.read_notifications(cx);
        }
    }

    fn read_notifications(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let permission = notifications::permission().await;
            this.update(cx, |this, cx| {
                this.session.notifications = permission;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks macOS to let Aven show notifications. Only the first request
    /// prompts; later ones answer from the user's choice.
    pub(crate) fn ask_notifications(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let permission = notifications::request().await;
            this.update(cx, |this, cx| {
                this.session.notifications = permission;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_settings(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(window, cx);
        let tab = self.session.settings.unwrap_or_default();
        self.show_settings(tab, cx);
    }

    pub(crate) fn close_settings(
        &mut self,
        _: &CloseSettings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session.close_settings();
        cx.notify();
    }

    pub(crate) fn sync_now(&mut self, _: &SyncNow, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.sync_now() {
            self.open_sync(&OpenSync, window, cx);
        }
        cx.notify();
    }

    pub fn open_sync(&mut self, _: &OpenSync, window: &mut Window, cx: &mut Context<Self>) {
        self.session.open_sync();
        let input = cx.new(|cx| Editor::new(Mode::Single, "Paste the invitation here", window, cx));
        let buttons = [cx.focus_handle(), cx.focus_handle()];
        let fields = buttons.to_vec();
        let dialog = Dialog::Sync {
            input: input.clone(),
            buttons,
            page: None,
        };
        self.open_dialog_at(dialog, vec![input], fields, 1, window, cx);
    }

    /// Moves focus with the Sync dialog's page, which changes from buttons and
    /// from replies alike, and keeps Tab on the controls the page shows.
    pub(crate) fn focus_sync_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = std::mem::discriminant(&self.session.sync_page);
        let invitation = matches!(self.session.sync_page, SyncPage::Invitation { .. });
        let single = match &self.session.sync_page {
            SyncPage::Running | SyncPage::Failed(_) => true,
            SyncPage::Done { kind, .. } => *kind == SyncKind::Join,
            _ => false,
        };
        let choosing = matches!(self.session.sync_page, SyncPage::Home)
            && self.session.snapshot.sync.phase == SyncPhase::Local;
        let primary_last = !invitation && !single && !choosing;
        let Some(layer) = self.dialog.as_mut() else {
            return;
        };
        let Dialog::Sync {
            input,
            buttons,
            page,
        } = &mut layer.dialog
        else {
            return;
        };
        if *page == Some(current) {
            return;
        }
        *page = Some(current);
        layer.fields = if invitation {
            vec![
                input.read(cx).focus.clone(),
                buttons[0].clone(),
                buttons[1].clone(),
            ]
        } else if single {
            vec![buttons[0].clone()]
        } else {
            buttons.to_vec()
        };
        let focus = layer.fields[usize::from(primary_last)].clone();
        cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
    }

    fn set_theme_pref(&mut self, pref: ThemePref, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.theme = pref;
        self.save_prefs();
        self.apply_theme(window, cx);
    }

    fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        self.prefs.compact = compact;
        self.save_prefs();
        self.apply_scale(cx);
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.prefs.zoom = zoom;
        self.save_prefs();
        self.apply_scale(cx);
    }

    fn edit_reminders(&mut self, edit: impl FnOnce(&mut Reminders)) {
        let mut reminders = self.session.settings_data.reminders.clone();
        edit(&mut reminders);
        self.session.settings_data.reminders = reminders.clone();
        self.session
            .write(Command::Settings(SettingsEdit::Reminders(reminders)));
    }

    fn add_label(&mut self, cx: &mut Context<Self>) {
        let name = self.new_label.read(cx).text().trim().to_string();
        if !name.is_empty() {
            self.session
                .write(Command::Settings(SettingsEdit::NewLabel(name)));
            self.new_label
                .update(cx, |editor, cx| editor.set_text("", cx));
        }
    }

    fn rename_prompt(
        &mut self,
        title: &str,
        value: &str,
        edit: impl Fn(String) -> SettingsEdit + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_prompt(
            title.to_string(),
            "Name",
            "",
            value,
            Rc::new(move |root, text, _, cx| {
                let text = text.trim().to_string();
                if text.is_empty() {
                    root.session.show_toast(Toast::info("Add a name"));
                } else {
                    root.session.write(Command::Settings(edit(text)));
                }
                cx.notify();
            }),
            window,
            cx,
        );
    }

    pub(crate) fn open_project(
        &mut self,
        key: Option<String>,
        name: &str,
        prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = |text: &str, window: &mut Window, cx: &mut Context<Self>| {
            let text = text.to_string();
            cx.new(|cx| {
                let mut editor = Editor::new(Mode::Single, "", window, cx);
                editor.set_text(&text, cx);
                editor
            })
        };
        let name = field(name, window, cx);
        let prefix = field(prefix, window, cx);
        let dialog = Dialog::Project {
            key,
            name: name.clone(),
            prefix: prefix.clone(),
            error: None,
        };
        self.open_dialog(dialog, vec![name, prefix], window, cx);
    }

    pub(crate) fn save_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Layer {
            dialog:
                Dialog::Project {
                    key,
                    name,
                    prefix,
                    error,
                },
            ..
        }) = self.dialog.as_mut()
        else {
            return;
        };
        let (key, title) = (key.clone(), name.read(cx).text().trim().to_string());
        let code = prefix.read(cx).text().trim().to_uppercase();
        *error = title.is_empty().then_some("Add a name");
        if error.is_some() {
            cx.notify();
            return;
        }
        self.close_dialog(window, cx);
        let edit = match key {
            Some(key) => SettingsEdit::EditProject {
                key,
                name: title,
                prefix: code,
            },
            None => SettingsEdit::NewProject {
                name: title,
                prefix: code,
            },
        };
        self.session.write(Command::Settings(edit));
    }

    pub(crate) fn render_project(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Layer {
            dialog:
                Dialog::Project {
                    key,
                    name,
                    prefix,
                    error,
                },
            ..
        }) = self.dialog.as_ref()
        else {
            return Vec::new();
        };
        let title = if key.is_some() {
            "Edit project"
        } else {
            "New project"
        };
        vec![
            Self::heading(title.to_string(), t).into_any_element(),
            div()
                .px(px(18.))
                .pb(px(12.))
                .child(Self::field_label("Name", t))
                .child(Self::field_box(name, t))
                .child(Self::field_label("Prefix", t))
                .child(Self::field_box(prefix, t))
                .when_some(*error, |body, error| {
                    body.child(
                        small(Text::Secondary)
                            .mt(px(8.))
                            .text_color(t.bad)
                            .child(error),
                    )
                })
                .into_any_element(),
            self.cancel_and_accept("Save", cx, t).into_any_element(),
        ]
    }

    pub(crate) fn confirm_delete_project(
        &mut self,
        key: String,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirm(
            format!("Delete project {name}?"),
            "The project will be removed from the sidebar. Existing tasks will be kept, but recurring schedules in this project will stop."
                .to_string(),
            Some("This change syncs to your other devices."),
            "Delete project",
            ConfirmAction::DeleteProject(key),
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_confirm(
        &mut self,
        title: String,
        body: String,
        note: Option<&'static str>,
        label: &'static str,
        action: ConfirmAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let buttons = [cx.focus_handle(), cx.focus_handle()];
        let fields = buttons.to_vec();
        let cancel = match action {
            ConfirmAction::ReplaceDraft { .. } | ConfirmAction::OpenKeptDraft(_) => "Keep draft",
            ConfirmAction::Quit {
                notification_settings: true,
            } => "Open Notification Settings",
            ConfirmAction::Quit { .. } => "Open Settings",
            _ => "Cancel",
        };
        let first = action.first_button();
        let dialog = Dialog::Confirm {
            title,
            body,
            note,
            label,
            cancel,
            action,
            buttons,
        };
        self.open_dialog_at(dialog, Vec::new(), fields, first, window, cx);
    }

    fn confirmed(&mut self, action: ConfirmAction, window: &mut Window, cx: &mut Context<Self>) {
        self.close_dialog(window, cx);
        match action {
            ConfirmAction::DeleteProject(key) => {
                self.session
                    .write(Command::Settings(SettingsEdit::DeleteProject(key)));
            }
            ConfirmAction::RemoveDevice { id, label } => {
                self.session.run_sync(SyncOp::RemoveDevice { id, label });
            }
            ConfirmAction::Restore(path) => self.session.write(Command::Restore(path)),
            ConfirmAction::Reset { unfinished_setup } => {
                self.session.run_sync(SyncOp::Reset { unfinished_setup });
            }
            ConfirmAction::ReplaceDraft { workspace, task } => {
                self.take_launcher_draft(workspace, *task, window, cx);
            }
            ConfirmAction::OpenKeptDraft(job) => self.take_kept_draft(job, window, cx),
            ConfirmAction::Quit { .. } => self.quit(window, cx),
        }
        cx.notify();
    }

    pub(crate) fn render_confirm(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Layer {
            dialog:
                Dialog::Confirm {
                    title,
                    body,
                    note,
                    label,
                    cancel: cancel_label,
                    action,
                    buttons: [cancel, confirm],
                },
            ..
        }) = self.dialog.as_ref()
        else {
            return Vec::new();
        };
        let action = action.clone();
        let keeps_draft = matches!(action, ConfirmAction::ReplaceDraft { .. });
        let opens_settings = match action {
            ConfirmAction::Quit {
                notification_settings,
            } => Some(notification_settings),
            _ => None,
        };
        vec![
            Self::heading(title.clone(), t).into_any_element(),
            div()
                .px(px(18.))
                .py(px(12.))
                .text_color(t.body)
                .child(body.clone())
                .when_some(*note, |body, note| {
                    body.child(
                        small(Text::Secondary)
                            .mt(px(8.))
                            .text_color(t.faint)
                            .child(note),
                    )
                })
                .into_any_element(),
            Self::footer(t)
                .child(tab_stop(
                    line_btn("confirm-cancel", *cancel_label, t),
                    cancel,
                    t,
                    cx,
                    move |this, window, cx| {
                        this.close_dialog(window, cx);
                        if keeps_draft {
                            cx.defer(crate::launcher::show);
                        }
                        if opens_settings == Some(true) {
                            cx.open_url(notifications::SETTINGS_URL);
                        } else if opens_settings == Some(false) {
                            let tab = if this.session.settings_data.background.reminders {
                                SettingsTab::Reminders
                            } else {
                                SettingsTab::Sync
                            };
                            this.show_settings(tab, cx);
                        }
                    },
                ))
                .child(tab_stop(
                    fill_btn("confirm-ok", *label, t),
                    confirm,
                    t,
                    cx,
                    move |this, window, cx| this.confirmed(action.clone(), window, cx),
                ))
                .into_any_element(),
        ]
    }

    pub(crate) fn pick_archive(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Restore".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = receiver.await
                && let Some(path) = paths.pop()
            {
                this.update_in(cx, |this, window, cx| {
                    this.confirm_restore(path, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn confirm_restore(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirm(
            "Restore backup?".to_string(),
            path.display().to_string(),
            Some("The restored copy is local only. Set up sync again if you want it on other devices."),
            "Restore",
            ConfirmAction::Restore(path),
            window,
            cx,
        );
    }

    pub(crate) fn open_other_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_prompt(
            "Open another database".to_string(),
            "Database path",
            "",
            "",
            Rc::new(|root, text, _, cx| {
                let text = text.trim();
                if text.is_empty() {
                    root.session.show_toast(Toast::info("Add a path"));
                } else {
                    root.session
                        .write(Command::OpenDatabase(PathBuf::from(text)));
                }
                cx.notify();
            }),
            window,
            cx,
        );
    }

    pub(crate) fn render_settings(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let current = self.session.settings.unwrap_or_default();
        let body = match current {
            SettingsTab::General => self.general_tab(window, cx, &t),
            SettingsTab::QuickCapture => self.quick_capture_tab(cx, &t),
            SettingsTab::Sync => self.sync_tab(cx, &t),
            SettingsTab::Reminders => self.reminders_tab(cx, &t),
            SettingsTab::Workspaces => self.workspaces_tab(cx, &t),
            SettingsTab::Projects => self.projects_tab(cx, &t),
            SettingsTab::Labels => self.labels_tab(cx, &t),
            SettingsTab::Fields => self.fields_tab(cx, &t),
            SettingsTab::Commands => self.commands_tab(&t),
            SettingsTab::Data => self.data_tab(cx, &t),
        };
        let (tint, ink) = (t.tint, t.ink);
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .child(
                div()
                    .flex_none()
                    .w(px(190.))
                    .h_full()
                    .px(px(8.))
                    .py(px(12.))
                    .child(
                        div()
                            .id("settings-back")
                            .role(Role::Button)
                            .aria_label("Back to tasks")
                            .flex()
                            .items_center()
                            .h(px(27.))
                            .mb(px(8.))
                            .px(px(8.))
                            .text_color(t.muted)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_settings(&CloseSettings, window, cx);
                            }))
                            .child("← Back to tasks"),
                    )
                    .children(TABS.into_iter().map(|(tab, name)| {
                        nav_item(SharedString::from(format!("settings-tab-{name}")), &t)
                            .role(Role::Tab)
                            .aria_label(name)
                            .when(tab == current, |item| {
                                item.bg(tint)
                                    .text_color(ink)
                                    .font_weight(FontWeight::MEDIUM)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_settings(tab, cx);
                            }))
                            .child(name)
                    })),
            )
            .child(div().flex_none().w(px(1.)).h_full().bg(t.line))
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .px(px(32.))
                    .pt(px(32.))
                    .pb(px(60.))
                    .child(div().w_full().max_w(px(620.)).mx_auto().child(body)),
            )
            .into_any_element()
    }

    fn segment<T: Copy + PartialEq + 'static>(
        &self,
        name: &'static str,
        options: &[(T, &'static str)],
        current: T,
        pick: fn(&mut Self, T, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Div {
        let (page, ink, muted) = (t.page, t.ink, t.muted);
        div()
            .flex()
            .flex_none()
            .p(px(2.))
            .rounded(px(RADIUS_CONTROL))
            .bg(t.raised)
            .children(options.iter().map(|(value, label)| {
                let value = *value;
                let on = value == current;
                let ring = t.inset_ring(t.line);
                div()
                    .id(SharedString::from(format!("{name}-{label}")))
                    .role(Role::RadioButton)
                    .aria_label(*label)
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(RADIUS_CHIP))
                    .text_color(if on { ink } else { muted })
                    .when(on, |item| item.bg(page).shadow(vec![ring]))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        pick(this, value, window, cx);
                    }))
                    .child(*label)
            }))
    }

    fn switch(
        &self,
        name: &'static str,
        on: bool,
        edit: fn(&mut Reminders),
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> AnyElement {
        let track = div()
            .id(name)
            .role(Role::Switch)
            .aria_label(name)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_reminders(edit);
                this.mark_switched(name);
                cx.notify();
            }));
        self.switch_track(track, name, on, 1., cx, t)
    }

    pub(crate) fn mark_switched(&mut self, name: &'static str) {
        self.switched.mark_switched(name);
    }

    pub(crate) fn switch_track(
        &self,
        track: Stateful<Div>,
        name: &'static str,
        on: bool,
        scale: f32,
        cx: &App,
        t: &Theme,
    ) -> AnyElement {
        self.switched.switch_track(track, name, on, scale, cx, t)
    }
    fn general_tab(&self, _: &Window, cx: &mut Context<Self>, t: &Theme) -> Div {
        let themes = [
            (ThemePref::Dark, "Dark"),
            (ThemePref::Light, "Light"),
            (ThemePref::System, "System"),
        ];
        let zooms = [(92, "Small"), (100, "Default"), (110, "Large")];
        let (update_note, update_button) = self.update_status(cx, t);
        page("General", Some("Appearance applies to this device."), t)
            .child(group(
                vec![
                    row(t)
                        .child(row_title("Theme", None, t))
                        .child(self.segment(
                            "theme",
                            &themes,
                            self.prefs.theme,
                            Self::set_theme_pref,
                            cx,
                            t,
                        )),
                    row(t)
                        .child(row_title("Density", None, t))
                        .child(self.segment(
                            "density",
                            &[(false, "Comfortable"), (true, "Compact")],
                            self.prefs.compact,
                            |this, compact, _, cx| this.set_compact(compact, cx),
                            cx,
                            t,
                        )),
                    row(t)
                        .child(row_title("Text size", None, t))
                        .child(self.segment(
                            "zoom",
                            &zooms,
                            (self.prefs.zoom * 100.).round() as i32,
                            |this, zoom, _, cx| this.set_zoom(zoom as f32 / 100., cx),
                            cx,
                            t,
                        )),
                ],
                t,
            ))
            .child(section("About", t))
            .child(group(
                vec![
                    row(t)
                        .child(row_title("Aven", Some(update_note), t))
                        .when_some(update_button, |row, button| row.child(button))
                        .child(
                            line_btn("open-about", "About Aven", t).on_click(cx.listener(
                                |this, _, window, cx| this.open_about(&OpenAbout, window, cx),
                            )),
                        ),
                ],
                t,
            ))
    }

    fn quick_capture_tab(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        page(
            "Quick capture",
            Some("The launcher adds tasks from any app while Aven runs."),
            t,
        )
        .child(group(
            vec![
                self.shortcut_row(cx, t),
                self.launcher_ai_row(cx, t),
                self.agent_row(cx, t),
            ],
            t,
        ))
    }

    /// The agent task intake runs, opening into its command line and a test.
    fn agent_row(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let setup = self.session.settings_data.agent.as_ref();
        let open = self.agent_open;
        let (status, tone) = match setup {
            Some(setup) if setup.runnable => ("Ready", Some(t.good)),
            Some(_) => ("Not found", Some(t.bad)),
            None => ("Not set up", None),
        };
        let note = match setup {
            Some(setup) => small(Text::Secondary)
                .mt(px(1.))
                .font_family(MONO)
                .text_color(t.muted)
                .child(setup.command.clone()),
            None => small(Text::Secondary)
                .mt(px(1.))
                .text_color(t.muted)
                .child("Set agent.task_intake.command to use your agent."),
        };
        let chevron = svg()
            .path("icons/chevron.svg")
            .size(px(14.))
            .flex_none()
            .text_color(t.faint)
            .when(open, |glyph| {
                glyph.with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
            });
        let raised = t.raised;
        let head = row(t)
            .id("agent-row")
            .role(Role::Button)
            .aria_label("Agent")
            .aria_expanded(open)
            .cursor_pointer()
            .hover(move |style| style.bg(raised))
            .on_click(cx.listener(|this, _, _, cx| {
                this.agent_open = !this.agent_open;
                if this.agent_open {
                    this.session.reload_settings();
                }
                cx.notify();
            }))
            .child(div().flex_1().min_w_0().child("Agent").child(note))
            .child(status_pill(status, tone, t))
            .child(chevron);
        let details = match setup {
            Some(setup) => self.agent_details(setup, cx, t),
            None => self.agent_snippet(cx, t),
        };
        div()
            .child(head)
            .when(open, |row| row.child(details.px(px(14.)).pb(px(12.))))
    }

    fn agent_details(&self, setup: &AgentSetup, cx: &mut Context<Self>, t: &Theme) -> Div {
        let (line, spans) = command_line(setup);
        let highlights = spans
            .into_iter()
            .map(|(range, part)| {
                let style = match part {
                    Part::Command => HighlightStyle {
                        font_weight: Some(FontWeight::SEMIBOLD),
                        color: Some(t.ink),
                        ..HighlightStyle::default()
                    },
                    Part::Placeholder => HighlightStyle {
                        color: Some(t.accent),
                        ..HighlightStyle::default()
                    },
                };
                (range, style)
            })
            .collect::<Vec<_>>();
        let running = matches!(self.session.agent_test, Some(AgentTestState::Running));
        let label = if running { "Testing\u{2026}" } else { "Test" };
        let test = btn("agent-test", label, t)
            .shadow(vec![t.inset_ring(t.line2)])
            .when(running, |button| {
                button.text_color(t.muted).child(
                    svg()
                        .path("icons/toast-progress.svg")
                        .size(px(13.))
                        .text_color(t.accent)
                        .with_animation(
                            "agent-test-spin",
                            Animation::new(Duration::from_millis(700)).repeat(),
                            |glyph, delta| {
                                glyph.with_transformation(Transformation::rotate(radians(
                                    delta * TAU,
                                )))
                            },
                        ),
                )
            })
            .when(!running, |button| {
                button.on_click(cx.listener(|this, _, _, cx| {
                    this.session.test_agent();
                    cx.notify();
                }))
            })
            .child(label);
        div()
            .child(command_block(t).child(StyledText::new(line).with_highlights(highlights)))
            .when(!setup.runnable, |details| {
                details.child(
                    small(Text::Secondary)
                        .mt(px(6.))
                        .text_color(t.bad)
                        .child(format!("`{}` isn\u{2019}t on PATH.", setup.command)),
                )
            })
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(test)
                    .child(open_config_btn(t)),
            )
            .when_some(self.agent_result(setup, t), |details, result| {
                details.child(result)
            })
    }

    /// What the last test sent and what came back, or why it failed.
    fn agent_result(&self, setup: &AgentSetup, t: &Theme) -> Option<Div> {
        let card = div()
            .mt(px(10.))
            .p(px(10.))
            .rounded(px(RADIUS_CONTROL))
            .bg(t.page)
            .shadow(vec![t.inset_ring(t.line)]);
        match self.session.agent_test.as_ref()? {
            AgentTestState::Running => None,
            AgentTestState::Failed(message) => {
                Some(
                    card.child(div().text_color(t.bad).child(message.clone()))
                        .when(!setup.runnable, |card| {
                            card.child(small(Text::Secondary).mt(px(2.)).text_color(t.muted).child(
                                "Install it, or set command to its full path in config.yaml.",
                            ))
                        }),
                )
            }
            AgentTestState::Done(test) => {
                let projects = &self.session.settings_data.projects;
                let chips = test_chips(&test.task, projects);
                Some(
                    card.child(small(Text::Secondary).mb(px(4.)).text_color(t.muted).child(
                        format!("Sent \u{201c}{AGENT_TEST_NOTE}\u{201d}. Your agent wrote:"),
                    ))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.ink)
                            .child(test.task.title.clone()),
                    )
                    .when(!chips.is_empty(), |card| {
                        card.child(div().mt(px(6.)).flex().flex_wrap().gap(px(6.)).children(
                            chips.into_iter().map(|chip| {
                                small(Text::Secondary)
                                    .px(px(7.))
                                    .rounded(px(RADIUS_CHIP))
                                    .bg(t.raised)
                                    .text_color(t.body)
                                    .child(chip)
                            }),
                        ))
                    })
                    .child(
                        small(Text::Secondary)
                            .mt(px(6.))
                            .text_color(t.muted)
                            .child(format!(
                                "Answered in {:.1} s. Nothing was saved.",
                                test.elapsed.as_secs_f32()
                            )),
                    ),
                )
            }
        }
    }

    /// The config to add when no agent is set up.
    fn agent_snippet(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let path = config_path_text();
        div()
            .child(
                small(Text::Secondary)
                    .mb(px(8.))
                    .flex()
                    .flex_wrap()
                    .text_color(t.muted)
                    .child("Add to\u{a0}")
                    .child(div().font_family(MONO).child(path))
                    .child(":"),
            )
            .child(command_block(t).child(AGENT_SNIPPET))
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(line_btn("agent-copy", "Copy", t).on_click(cx.listener(
                        |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                AGENT_SNIPPET.to_string(),
                            ));
                            this.session.copied_toast("agent config");
                            cx.notify();
                        },
                    )))
                    .child(open_config_btn(t))
                    .child(div().flex_1())
                    .child(
                        small(Text::Secondary)
                            .id("agent-snippet-docs")
                            .role(Role::Link)
                            .aria_label("Agent setup")
                            .text_color(t.accent)
                            .on_click(|_, _, cx| cx.open_url(AGENT_DOCS_URL))
                            .child("Agent setup"),
                    ),
            )
    }

    fn shortcut_row(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let recording = self.recorder.is_some();
        let shortcut = self.prefs.launcher.shortcut.as_ref();
        let note = self
            .shortcut_error
            .clone()
            .unwrap_or_else(|| "Opens the launcher.".to_string());
        let label = match (recording, shortcut) {
            (true, _) => "Press keys\u{2026}".to_string(),
            (false, Some(shortcut)) => shortcut.label(),
            (false, None) => "Record shortcut".to_string(),
        };
        let accent = t.accent;
        row(t)
            .child(
                div().flex_1().min_w_0().child("Launcher shortcut").child(
                    small(Text::Secondary)
                        .mt(px(1.))
                        .text_color(if self.shortcut_error.is_some() {
                            t.bad
                        } else {
                            t.muted
                        })
                        .child(note),
                ),
            )
            .when(shortcut.is_some() && !recording, |row| {
                row.child(
                    btn("shortcut-clear", "Clear shortcut", t)
                        .on_click(cx.listener(|this, _, _, cx| this.clear_shortcut(cx)))
                        .child("Clear"),
                )
            })
            .child(
                line_btn("shortcut-record", label, t)
                    .when(recording, |button| {
                        button.text_color(accent).shadow(vec![t.inset_ring(accent)])
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.recorder.is_some() {
                            this.stop_recording(cx);
                        } else {
                            this.record_shortcut(cx);
                        }
                    })),
            )
    }

    /// Whether the launcher's Return has the agent write the task. Off and
    /// fixed while no agent is configured.
    fn launcher_ai_row(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        const NAME: &str = "Create tasks with AI";
        let agent = self.session.settings_data.agent.is_some();
        let on = agent && self.prefs.launcher.ai;
        let note = if agent {
            "Return has your agent write the task, and \u{2325}\u{21b5} adds it as typed."
        } else {
            "Set up an agent below to have it write your tasks."
        };
        let track = div()
            .id(NAME)
            .role(Role::Switch)
            .aria_label(NAME)
            .when(agent, |track| {
                track.on_click(cx.listener(|this, _, _, cx| this.toggle_launcher_ai(cx)))
            })
            .when(!agent, |track| track.opacity(0.5));
        row(t)
            .child(
                div().flex_1().min_w_0().child(NAME).child(
                    small(Text::Secondary)
                        .mt(px(1.))
                        .flex()
                        .flex_wrap()
                        .text_color(t.muted)
                        .child(format!("{note} "))
                        .child(
                            div()
                                .id("agent-docs")
                                .role(Role::Link)
                                .aria_label("Agent setup")
                                .text_color(t.accent)
                                .on_click(|_, _, cx| cx.open_url(AGENT_DOCS_URL))
                                .child("Agent setup"),
                        ),
                ),
            )
            .child(self.switch_track(track, NAME, on, 1., cx, t))
    }

    fn toggle_launcher_ai(&mut self, cx: &mut Context<Self>) {
        let ai = !self.prefs.launcher.ai;
        self.prefs.launcher.ai = ai;
        self.save_prefs();
        crate::launcher::set_ai(ai, cx);
        self.mark_switched("Create tasks with AI");
        cx.notify();
    }

    /// Takes the next key pressed with Command, Control or Option as the
    /// launcher shortcut. The current one is free while recording, so it can
    /// be recorded again.
    fn record_shortcut(&mut self, cx: &mut Context<Self>) {
        let _ = crate::launcher::set_shortcut(None, cx);
        let this = cx.weak_entity();
        self.recorder = Some(cx.intercept_keystrokes(move |event, _, cx| {
            if event.keystroke.is_modifier_key() {
                return;
            }
            cx.stop_propagation();
            let keystroke = event.keystroke.clone();
            this.update(cx, |root, cx| root.recorded(&keystroke, cx))
                .ok();
        }));
        self.shortcut_error = None;
        cx.notify();
    }

    fn recorded(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<Self>) {
        let modifiers = keystroke.modifiers;
        if keystroke.key == "escape" && !modifiers.modified() {
            self.stop_recording(cx);
            return;
        }
        let shortcut = crate::hotkey::Shortcut::from_keystroke(keystroke);
        let taken = if shortcut.usable() {
            crate::launcher::set_shortcut(Some(&shortcut), cx)
        } else {
            Err("Use \u{2318}, \u{2303} or \u{2325} with a key".to_string())
        };
        match taken {
            Ok(()) => {
                self.end_recording(cx);
                self.shortcut_error = None;
                self.prefs.launcher.shortcut = Some(shortcut);
                self.save_prefs();
            }
            Err(error) => self.shortcut_error = Some(error),
        }
        cx.notify();
    }

    /// Stops recording and takes back the shortcut there was.
    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        self.end_recording(cx);
        self.shortcut_error = self
            .prefs
            .launcher
            .shortcut
            .as_ref()
            .and_then(|shortcut| crate::launcher::set_shortcut(Some(shortcut), cx).err());
        cx.notify();
    }

    fn end_recording(&mut self, cx: &mut Context<Self>) {
        // The recorder may be the interceptor running now.
        let recorder = self.recorder.take();
        cx.defer(move |_| drop(recorder));
    }

    fn clear_shortcut(&mut self, cx: &mut Context<Self>) {
        let _ = crate::launcher::set_shortcut(None, cx);
        self.prefs.launcher.shortcut = None;
        self.shortcut_error = None;
        self.save_prefs();
        cx.notify();
    }

    fn device_rows(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<Div> {
        match &self.session.devices {
            Some(Ok(list)) => list
                .devices
                .iter()
                .enumerate()
                .map(|(index, device)| {
                    let (id, label) = (device.id, device.label.clone());
                    let note = device.current.then(|| "This device".to_string());
                    row(t)
                        .child(icon("icons/device.svg", t.faint))
                        .child(row_title(device.label.clone(), note, t))
                        .when(!device.current, |row| {
                            row.child(
                                line_btn(
                                    SharedString::from(format!("device-remove-{index}")),
                                    "Remove",
                                    t,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.open_confirm(
                                            format!("Remove {label}?"),
                                            "This device will lose access to future synced \
                                             changes. Tasks and images it already downloaded \
                                             will remain on it."
                                                .to_string(),
                                            None,
                                            "Remove",
                                            ConfirmAction::RemoveDevice {
                                                id,
                                                label: label.clone(),
                                            },
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            )
                        })
                })
                .collect(),
            Some(Err(message)) => vec![row(t).child(row_title(message.clone(), None, t))],
            None => vec![row(t).child(row_title("Checking devices…", None, t))],
        }
    }

    fn sync_tab(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let (tone, line) = self.session.sync_line();
        let set_up = self.session.snapshot.sync.phase == SyncPhase::SetUp;
        let unfinished = (self.session.devices.as_ref())
            .is_some_and(|list| list.as_ref().is_ok_and(|list| list.removal_unfinished));
        page(
            "Sync and devices",
            Some("Sync is end-to-end encrypted. Workspaces share one sync connection."),
            t,
        )
        .child(group(
            vec![
                row(t)
                    .child(icon("icons/cloud.svg", sync_icon(t, tone)))
                    .child(row_title(line, None, t))
                    .child(line_btn("settings-open-sync", "Open sync", t).on_click(
                        cx.listener(|this, _, window, cx| this.open_sync(&OpenSync, window, cx)),
                    )),
            ],
            t,
        ))
        // The switch stays reachable while it's on, so a config that asks for
        // background sync without a connection can still be turned off.
        .when(
            set_up || self.session.settings_data.background.sync_when_quit,
            |tab| {
                tab.child(section("While Aven is quit", t))
                    .child(group(self.sync_when_quit_rows(cx, t), t))
            },
        )
        .when(set_up, |tab| {
            tab.child(section("Devices", t))
                .child(group(self.device_rows(cx, t), t))
                .when(unfinished, |tab| {
                    tab.child(
                        small(Text::Secondary)
                            .pt(px(7.))
                            .px(px(2.))
                            .text_color(t.warn)
                            .child(
                                "Securing future changes is unfinished. Finish removal continues \
                                 it, and any remaining device finishes it when it syncs.",
                            ),
                    )
                })
                .child(
                    tool_row(line_btn("settings-add-device", "Add device", t).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_sync(&OpenSync, window, cx);
                            this.session.add_device();
                        }),
                    ))
                    .when(unfinished, |tools| {
                        tools.child(
                            line_btn("settings-finish-removal", "Finish removal", t).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.session.run_sync(SyncOp::FinishRemoval);
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
                )
        })
    }

    fn reminders_tab(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let reminders = self.session.settings_data.reminders.clone();
        let agent = self.session.settings_data.background.agent;
        let digest = reminders.digest_time.clone();
        let on = reminders.enabled;
        let track = div()
            .id("Reminders on this Mac")
            .role(Role::Switch)
            .aria_label("Reminders on this Mac")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_reminders(|value| value.enabled = !value.enabled);
                this.mark_switched("Reminders on this Mac");
                if !on {
                    this.ask_notifications(cx);
                }
                cx.notify();
            }));
        let status: Vec<Div> = [
            self.background_agent_row(cx, t),
            self.notifications_row(cx, t),
        ]
        .into_iter()
        .flatten()
        .collect();
        page("Reminders", None, t)
            .child(lead(reminders_lead(on, agent), t))
            .child(group(
                vec![
                    row(t)
                        .child(row_title("Reminders on this Mac", None, t))
                        .child(self.switch_track(track, "Reminders on this Mac", on, 1., cx, t)),
                    row(t)
                        .child(row_title(
                            "Daily digest",
                            Some("Due and overdue tasks, once a day".to_string()),
                            t,
                        ))
                        .child(self.switch(
                            "Daily digest",
                            reminders.due_digest,
                            |value| value.due_digest = !value.due_digest,
                            cx,
                            t,
                        )),
                    row(t).child(row_title("Digest time", None, t)).child(
                        line_btn("digest", digest.clone(), t).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_prompt(
                                    "Digest time".to_string(),
                                    "Time",
                                    "Local time, as HH:MM.",
                                    &digest,
                                    Rc::new(|root, text, _, cx| {
                                        root.edit_reminders(|value| {
                                            value.digest_time = text.trim().to_string();
                                        });
                                        cx.notify();
                                    }),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    ),
                    row(t)
                        .child(row_title(
                            "Available again",
                            Some("When a deferred task reaches its time".to_string()),
                            t,
                        ))
                        .child(self.switch(
                            "Available again",
                            reminders.available,
                            |value| value.available = !value.available,
                            cx,
                            t,
                        )),
                ],
                t,
            ))
            .when(on && !status.is_empty(), |tab| {
                tab.child(section("Status", t)).child(group(status, t))
            })
    }

    /// The background agent's state, for an app that manages it.
    fn background_agent_row(&self, cx: &mut Context<Self>, t: &Theme) -> Option<Div> {
        let background = &self.session.settings_data.background;
        if !background.managed {
            return None;
        }
        let work = background.sync_when_quit || self.session.settings_data.reminders.enabled;
        let (pill, tone, note) = match background.agent {
            AgentShown::Unknown => return None,
            AgentShown::Running => (
                "Running",
                Some(t.good),
                "Keeps reminders and sync going while Aven is quit",
            ),
            AgentShown::Off => ("Off", None, "Reminders and sync stop when Aven quits"),
            AgentShown::NotRunning => (
                "Not running",
                Some(t.warn),
                "Reminders and sync stop when Aven quits",
            ),
            AgentShown::TurnedOff => (
                "Turned off",
                Some(t.warn),
                "Turned off in System Settings > General > Login Items & Extensions",
            ),
        };
        let button = match background.agent {
            AgentShown::TurnedOff => Some(
                line_btn("agent-login-items", "Open Login Items", t)
                    .on_click(|_, _, cx| cx.open_url(LOGIN_ITEMS_URL)),
            ),
            AgentShown::Off | AgentShown::NotRunning if work => Some(
                line_btn("agent-turn-on", "Turn On", t).on_click(cx.listener(|this, _, _, cx| {
                    this.session
                        .write(Command::Settings(SettingsEdit::TurnOnBackground));
                    cx.notify();
                })),
            ),
            _ => None,
        };
        Some(
            row(t)
                .child(row_title("Background agent", Some(note.to_string()), t))
                .child(status_pill(pill, tone, t))
                .children(button),
        )
    }

    fn notifications_row(&self, cx: &mut Context<Self>, t: &Theme) -> Option<Div> {
        let missed = missed_note(self.session.settings_data.background.notifications_off);
        let (pill, tone, note) =
            match self.session.notifications {
                Permission::Unavailable => return None,
                Permission::Allowed => ("Allowed", Some(t.good), None),
                Permission::Denied => (
                    "Denied",
                    Some(t.bad),
                    Some(missed.unwrap_or_else(|| {
                        "Reminders can't appear until you allow them".to_string()
                    })),
                ),
                Permission::NotAsked => ("Not asked", None, missed),
            };
        let button = if self.session.notifications == Permission::NotAsked {
            line_btn("notifications-allow", "Allow", t)
                .on_click(cx.listener(|this, _, _, cx| this.ask_notifications(cx)))
        } else {
            line_btn("notifications-settings", "Open System Settings", t)
                .on_click(|_, _, cx| cx.open_url(notifications::SETTINGS_URL))
        };
        Some(
            row(t)
                .child(row_title("Notifications", note, t))
                .child(status_pill(pill, tone, t))
                .child(button),
        )
    }

    /// The Sync tab's switch for syncing while Aven is quit, with the agent's
    /// last round.
    fn sync_when_quit_rows(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<Div> {
        let background = &self.session.settings_data.background;
        let on = background.sync_when_quit;
        let note = match background.last_sync {
            Some(at) => format!("Last background sync {}", ago(at, Local::now().timestamp())),
            None => "Keeps this Mac up to date in the background".to_string(),
        };
        let track = div()
            .id("Sync when Aven is quit")
            .role(Role::Switch)
            .aria_label("Sync when Aven is quit")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.session.set_sync_when_quit(!on);
                this.mark_switched("Sync when Aven is quit");
                cx.notify();
            }));
        let mut rows = vec![
            row(t)
                .child(row_title("Sync when Aven is quit", Some(note), t))
                .child(self.switch_track(track, "Sync when Aven is quit", on, 1., cx, t)),
        ];
        rows.extend(self.background_agent_row(cx, t).filter(|_| on));
        rows
    }

    fn workspaces_tab(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let rows = (self.session.settings_data.workspaces.iter())
            .enumerate()
            .map(|(index, workspace)| {
                let (key, name) = (workspace.key.clone(), workspace.name.clone());
                row(t)
                    .child(row_title(
                        workspace.name.clone(),
                        Some(workspace.key.clone()),
                        t,
                    ))
                    .child(
                        line_btn(
                            SharedString::from(format!("ws-rename-{index}")),
                            "Rename",
                            t,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let key = key.clone();
                                this.rename_prompt(
                                    "Rename workspace",
                                    &name,
                                    move |name| SettingsEdit::RenameWorkspace(key.clone(), name),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
            })
            .collect();
        page(
            "Workspaces",
            Some("Tasks in different workspaces stay separate. Directory routes pick one automatically."),
            t,
        )
        .child(group(rows, t))
        .child(tool_row(line_btn("new-workspace", "New workspace", t).on_click(
            cx.listener(|this, _, window, cx| {
                this.rename_prompt("New workspace", "", SettingsEdit::NewWorkspace, window, cx);
            }),
        )))
    }

    fn projects_tab(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let rows = (self.session.settings_data.projects.iter())
            .enumerate()
            .map(|(index, project)| {
                let (key, name, prefix) = (
                    project.key.clone(),
                    project.name.clone(),
                    project.prefix.clone(),
                );
                row(t)
                    .child(row_title(
                        project.name.clone(),
                        Some(format!("Prefix {}", project.prefix)),
                        t,
                    ))
                    .child(
                        line_btn(
                            SharedString::from(format!("project-edit-{index}")),
                            "Edit",
                            t,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_project(Some(key.clone()), &name, &prefix, window, cx);
                            },
                        )),
                    )
            })
            .collect();
        page(
            "Projects",
            Some("The prefix is display context for refs. Changing it keeps each task's identity."),
            t,
        )
        .child(group(rows, t))
        .child(tool_row(
            line_btn("new-project", "New project", t).on_click(
                cx.listener(|this, _, window, cx| this.open_project(None, "", "", window, cx)),
            ),
        ))
    }

    fn labels_tab(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let mut rows = vec![
            row(t)
                .child(
                    div()
                        .flex_1()
                        .h(px(30.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_CONTROL))
                        .bg(t.page)
                        .shadow(vec![t.inset_ring(t.line)])
                        .child(self.new_label.clone()),
                )
                .child(
                    line_btn("new-label", "Create", t)
                        .on_click(cx.listener(|this, _, _, cx| this.add_label(cx))),
                ),
        ];
        rows.extend(self.session.settings_data.labels.iter().enumerate().map(
            |(index, (label, used))| {
                let name = label.clone();
                row(t)
                    .child(row_title(label.clone(), Some(count_text(*used, "task")), t))
                    .child(
                        line_btn(
                            SharedString::from(format!("label-rename-{index}")),
                            "Rename",
                            t,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let old = name.clone();
                                this.rename_prompt(
                                    "Rename label",
                                    &name,
                                    move |new| SettingsEdit::RenameLabel(old.clone(), new),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
            },
        ));
        page(
            "Labels",
            Some("Names are normalized: lowercase, with punctuation and spaces as dashes."),
            t,
        )
        .child(group(rows, t))
    }

    fn fields_tab(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let rows: Vec<Div> = (self.session.settings_data.fields.iter())
            .enumerate()
            .map(|(index, (field, used))| {
                let name = field.clone();
                row(t)
                    .child(row_title(field.clone(), Some(count_text(*used, "task")), t))
                    .child(
                        line_btn(
                            SharedString::from(format!("field-rename-{index}")),
                            "Rename",
                            t,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let old = name.clone();
                                this.rename_prompt(
                                    "Rename field",
                                    &name,
                                    move |new| SettingsEdit::RenameField(old.clone(), new),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
            })
            .collect();
        page(
            "Metadata fields",
            Some("Fields hold string values. Filter on exact value, present or missing."),
            t,
        )
        .when(!rows.is_empty(), |tab| tab.child(group(rows, t)))
    }

    fn commands_tab(&self, t: &Theme) -> Div {
        let rows: Vec<Div> = (self.session.settings_data.commands.iter())
            .map(|(name, description)| {
                row(t).child(row_title(name.clone(), Some(description.clone()), t))
            })
            .collect();
        page(
            "Custom commands",
            Some("Trusted local programs. They run from the terminal app, on the selected or marked tasks."),
            t,
        )
        .when(!rows.is_empty(), |tab| tab.child(group(rows, t)))
    }

    fn data_tab(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let mut databases = vec![
            row(t)
                .child(path_title(
                    "Current database",
                    self.session.database.display().to_string(),
                    t,
                ))
                .child(line_btn("open-other-db", "Open another", t).on_click(
                    cx.listener(|this, _, window, cx| this.open_other_database(window, cx)),
                )),
        ];
        let others = (self.prefs.databases.iter())
            .filter(|known| known.path != self.session.database)
            .enumerate();
        databases.extend(others.map(|(index, known)| {
            let path = known.path.clone();
            row(t)
                .child(
                    path_title("Other database", known.path.display().to_string(), t).child(
                        small(Text::Secondary)
                            .text_color(t.muted)
                            .child(count_text(known.tasks, "task")),
                    ),
                )
                .child(
                    line_btn(SharedString::from(format!("switch-db-{index}")), "Open", t).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.session.write(Command::OpenDatabase(path.clone()));
                            cx.notify();
                        }),
                    ),
                )
        }));
        let phase = self.session.snapshot.sync.phase;
        let unfinished_setup = phase == SyncPhase::SetupIncomplete;
        page("Data and recovery", None, t)
            .child(section("Database", t))
            .child(group(databases, t))
            .child(
                small(Text::Secondary)
                    .pt(px(7.))
                    .px(px(2.))
                    .text_color(t.faint)
                    .child("Found the same way as the CLI: --db, AVEN_DB, local.db_path, then the default."),
            )
            .child(section("Backups", t))
            .child(group(
                vec![
                    row(t)
                        .child(row_title(
                            "Back up now",
                            Some("Writes an archive, including images stored on this device".to_string()),
                            t,
                        ))
                        .child(line_btn("backup-now", "Back up", t).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.session.write(Command::Backup);
                                cx.notify();
                            },
                        ))),
                    row(t)
                        .child(row_title(
                            "Restore",
                            Some("Restores to a fresh database. The restored copy is local only.".to_string()),
                            t,
                        ))
                        .child(line_btn("restore-backup", "Choose archive", t).on_click(
                            cx.listener(|this, _, _, cx| this.pick_archive(cx)),
                        )),
                    row(t)
                        .child(row_title(
                            "Export JSON",
                            Some("Keeps attachment records, without image files".to_string()),
                            t,
                        ))
                        .child(line_btn("export-json", "Export", t).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.session.write(Command::Export);
                                cx.notify();
                            },
                        ))),
                ],
                t,
            ))
            .when(phase != SyncPhase::Local && phase != SyncPhase::JoinIncomplete, |tab| {
                tab.child(section("Sync", t)).child(group(
                    vec![
                        row(t)
                            .child(row_title(
                                "Reset sync data",
                                Some("Tasks, images and history stay. Other devices still list this one.".to_string()),
                                t,
                            ))
                            .child(
                                line_btn("reset-sync", "Reset", t)
                                    .text_color(t.bad)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_reset(unfinished_setup, window, cx);
                                    })),
                            ),
                    ],
                    t,
                ))
            })
    }

    fn confirm_reset(
        &mut self,
        unfinished_setup: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (title, body) = if unfinished_setup {
            (
                "Abandon sync setup?",
                "Tasks stay on this device. This may orphan a server that accepted the setup.",
            )
        } else {
            (
                "Reset sync data?",
                "This deletes this database's sync state, unsent sync queue and protected sync \
                 keys. Tasks, images and history stay and remain editable. Changes not yet \
                 uploaded don't reach the old sync.",
            )
        };
        self.open_confirm(
            title.to_string(),
            body.to_string(),
            None,
            "Reset",
            ConfirmAction::Reset { unfinished_setup },
            window,
            cx,
        );
    }

    /// A window whose database did not open, with the ways out.
    pub(crate) fn render_database_gate(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(error) = self.session.database_error.clone() else {
            return div().into_any_element();
        };
        let path = error.path.clone();
        div()
            .flex_1()
            .min_w_0()
            .pt(px(120.))
            .px(px(24.))
            .child(
                div()
                    .w_full()
                    .max_w(px(560.))
                    .mx_auto()
                    .child(
                        div()
                            .mb(px(6.))
                            .text(Text::Display)
                            .line_height(px(31.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.ink)
                            .child("Aven can't open this database"),
                    )
                    .child(div().mb(px(18.)).text_color(t.muted).child(
                        "Aven hasn't changed this file. Restoring creates a new database and \
                         leaves this file in place.",
                    ))
                    .child(div().mb(px(18.)).text_color(t.faint).child(format!(
                        "{} · {}",
                        error.path.display(),
                        error.message
                    )))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                fill_btn("gate-restore", "Restore backup", &t)
                                    .on_click(cx.listener(|this, _, _, cx| this.pick_archive(cx))),
                            )
                            .child(
                                line_btn("gate-retry", "Try again", &t).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.session.write(Command::OpenDatabase(path.clone()));
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(line_btn("gate-open", "Open another database", &t).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.open_other_database(window, cx);
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }

    /// The Sync dialog's main action, which Enter also runs.
    pub(crate) fn sync_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.sync_page.clone() {
            SyncPage::Invitation { kind, new_database } => {
                self.invitation_next(kind, new_database, cx);
            }
            SyncPage::Confirm {
                kind,
                invitation,
                new_database,
                ..
            } => {
                self.session.run_sync(match kind {
                    SyncKind::Setup => SyncOp::Setup(invitation),
                    SyncKind::Join => SyncOp::Join {
                        invitation: Some(invitation),
                        new_database,
                    },
                });
                self.clear_invitation_input(cx);
            }
            SyncPage::Done { .. } | SyncPage::Running => self.close_dialog(window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn clear_invitation_input(&mut self, cx: &mut Context<Self>) {
        if let Some(Layer {
            dialog: Dialog::Sync { input, .. },
            ..
        }) = self.dialog.as_ref()
        {
            input.update(cx, |editor, cx| editor.set_text("", cx));
        }
    }

    fn invitation_input(&self, cx: &Context<Self>) -> Option<String> {
        match self.dialog.as_ref().map(|layer| &layer.dialog) {
            Some(Dialog::Sync { input, .. }) => Some(input.read(cx).text().trim().to_string()),
            _ => None,
        }
    }

    fn invitation_next(&mut self, kind: SyncKind, new_database: bool, cx: &mut Context<Self>) {
        let Some(text) = self.invitation_input(cx) else {
            return;
        };
        let check = InvitationCheck::of(&text);
        if check == InvitationCheck::Empty {
            self.session
                .show_toast(Toast::info("Paste the invitation first"));
        }
        let fits = matches!(
            (kind, &check),
            (SyncKind::Setup, InvitationCheck::Setup(_))
                | (SyncKind::Join, InvitationCheck::Device(_))
        );
        if fits {
            self.session.sync_page = SyncPage::Confirm {
                kind,
                invitation: Secret::new(text),
                new_database,
                facts: None,
            };
            if kind == SyncKind::Setup {
                self.session.request_sync(SyncRequest::Preview);
            }
        }
    }

    pub(crate) fn render_sync(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Layer {
            dialog: Dialog::Sync { input, buttons, .. },
            ..
        }) = self.dialog.as_ref()
        else {
            return Vec::new();
        };
        match &self.session.sync_page {
            SyncPage::Home => self.sync_home(buttons, cx, t),
            SyncPage::Invitation { kind, new_database } => {
                self.sync_invitation(*kind, *new_database, input, buttons, cx, t)
            }
            SyncPage::Confirm {
                kind,
                invitation,
                new_database,
                facts,
            } => self.sync_confirm(
                *kind,
                invitation,
                *new_database,
                facts.as_ref(),
                buttons,
                cx,
                t,
            ),
            SyncPage::NonEmpty(tasks) => self.sync_non_empty(*tasks, buttons, cx, t),
            SyncPage::Running => self.sync_running(buttons, cx, t),
            SyncPage::Done { kind, note } => self.sync_done(*kind, note, buttons, cx, t),
            SyncPage::Failed(message) => self.sync_failed(message, buttons, cx, t),
            SyncPage::AddDevice => self.sync_add_device(buttons, cx, t),
        }
    }

    fn close_button(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement + use<> {
        btn("sync-close", "Close", t)
            .ml_auto()
            .text_color(t.faint)
            .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
            .child("Close")
    }

    fn sync_head(&self, title: String, cx: &mut Context<Self>, t: &Theme) -> AnyElement {
        div()
            .flex()
            .items_center()
            .pr(px(10.))
            .child(div().flex_1().child(Self::heading(title, t)))
            .child(div().mt(px(8.)).child(self.close_button(cx, t)))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn option_row(
        &self,
        id: &'static str,
        icon_path: &'static str,
        title: &'static str,
        note: &'static str,
        focus: &FocusHandle,
        run: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Stateful<Div> {
        let raised = t.raised;
        tab_stop(
            row(t)
                .id(id)
                .role(Role::Button)
                .aria_label(title)
                .w_full()
                .min_h(px(52.))
                .hover(move |style| style.bg(raised))
                .child(icon(icon_path, t.muted))
                .child(row_title(title, Some(note.to_string()), t))
                .child(div().text_color(t.faint).child("›")),
            focus,
            t,
            cx,
            run,
        )
    }

    fn sync_home(
        &self,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let view = &self.session.snapshot.sync;
        match view.phase {
            SyncPhase::Local => vec![
                self.sync_head("Sync".to_string(), cx, t),
                div()
                    .px(px(18.))
                    .pt(px(12.))
                    .pb(px(16.))
                    .child(div().mb(px(12.)).text_color(t.muted).child(
                        "Keep your tasks in sync across devices. Set up sync from this \
                         computer's tasks, or join sync that another device already uses.",
                    ))
                    .child(
                        div()
                            .rounded(px(RADIUS_SURFACE))
                            .bg(t.panel)
                            .overflow_hidden()
                            .shadow(vec![t.inset_ring(t.line)])
                            .child(
                                self.option_row(
                                    "sync-start",
                                    "icons/cloud.svg",
                                    "Set up sync",
                                    "Start from this computer's tasks",
                                    &buttons[0],
                                    |this, _, cx| {
                                        this.session.sync_page = SyncPage::Invitation {
                                            kind: SyncKind::Setup,
                                            new_database: false,
                                        };
                                        cx.notify();
                                    },
                                    cx,
                                    t,
                                )
                                .rounded_t(px(RADIUS_SURFACE)),
                            )
                            .child(div().h(px(1.)).bg(t.line))
                            .child(
                                self.option_row(
                                    "sync-join",
                                    "icons/device.svg",
                                    "Join existing sync",
                                    "Use an invitation from a device that already syncs",
                                    &buttons[1],
                                    |this, _, cx| {
                                        this.session.request_sync(SyncRequest::CheckJoin);
                                        cx.notify();
                                    },
                                    cx,
                                    t,
                                )
                                .rounded_b(px(RADIUS_SURFACE)),
                            ),
                    )
                    .into_any_element(),
            ],
            SyncPhase::SetupIncomplete | SyncPhase::JoinIncomplete => {
                let setup = view.phase == SyncPhase::SetupIncomplete;
                vec![
                    self.sync_head(
                        if setup {
                            "Sync setup didn't finish"
                        } else {
                            "Joining didn't finish"
                        }
                        .to_string(),
                        cx,
                        t,
                    ),
                    div()
                        .px(px(18.))
                        .py(px(12.))
                        .text_color(t.body)
                        .child(if setup {
                            "Your tasks are safe on this device. Resume with an invitation from \
                             the same server."
                        } else {
                            "Resume to finish downloading your tasks."
                        })
                        .into_any_element(),
                    Self::footer(t)
                        .child(if setup {
                            tab_stop(
                                line_btn("sync-abandon", "Abandon, keep local data", t)
                                    .text_color(t.bad),
                                &buttons[0],
                                t,
                                cx,
                                |this, window, cx| this.confirm_reset(true, window, cx),
                            )
                        } else {
                            tab_stop(
                                line_btn("sync-new-invitation", "Use a new invitation", t),
                                &buttons[0],
                                t,
                                cx,
                                |this, _, cx| {
                                    this.session.sync_page = SyncPage::Invitation {
                                        kind: SyncKind::Join,
                                        new_database: false,
                                    };
                                    cx.notify();
                                },
                            )
                        })
                        .child(div().flex_1())
                        .child(tab_stop(
                            fill_btn(
                                "sync-resume",
                                if setup {
                                    "Resume setup"
                                } else {
                                    "Resume joining"
                                },
                                t,
                            ),
                            &buttons[1],
                            t,
                            cx,
                            move |this, _, cx| {
                                this.session.run_sync(if setup {
                                    SyncOp::ResumeSetup
                                } else {
                                    SyncOp::Join {
                                        invitation: None,
                                        new_database: false,
                                    }
                                });
                                cx.notify();
                            },
                        ))
                        .into_any_element(),
                ]
            }
            SyncPhase::SetUp => self.sync_status(buttons, cx, t),
        }
    }

    fn sync_server(&self, t: &Theme) -> Div {
        let server = self.session.snapshot.sync.server.as_deref();
        let identity = if server.is_some_and(aven_core::sync::client::is_aven_cloud) {
            let (fill, line, ink) = if t.dark {
                (0x241c2c, 0x624a7b, 0xdac4f3)
            } else {
                (0xf4edfd, 0xcfb6f1, 0x6b3fc4)
            };
            btn("sync-server-cloud", "Aven Cloud", t)
                .role(Role::Link)
                .h(px(22.))
                .px(px(5.))
                .ml(px(-5.))
                .on_click(|_, _, cx| cx.open_url(CLOUD_URL))
                .child(
                    gpui::img(if t.dark {
                        "wordmark-dark.svg"
                    } else {
                        "wordmark-light.svg"
                    })
                    .h(px(12.))
                    .w(px(12. * 195.6 / 52.)),
                )
                .child(
                    small(Text::Caption)
                        .font_family(MONO)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(gpui::rgb(ink))
                        .px(px(5.))
                        .rounded(px(RADIUS_CHIP))
                        .bg(gpui::rgb(fill))
                        .border_1()
                        .border_color(gpui::rgb(line))
                        .child("CLOUD"),
                )
                .into_any_element()
        } else {
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(7.))
                .child(icon("icons/server.svg", t.faint))
                .child(
                    small(Text::Secondary)
                        .min_w_0()
                        .font_family(MONO)
                        .text_color(t.body)
                        .child(server.unwrap_or("Unavailable").to_string()),
                )
                .into_any_element()
        };
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .min_h(px(26.))
            .pb(px(6.))
            .child(
                small(Text::Secondary)
                    .flex_none()
                    .text_color(t.faint)
                    .child("Server"),
            )
            .child(identity)
    }

    fn sync_status(
        &self,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let view = &self.session.snapshot.sync;
        let (tone, _) = self.session.sync_line();
        let (head, sub) = self.session.sync_hero();
        let stopped = view.access_refused || view.vault_deleted;
        let busy = self.session.sync_run.is_some();
        let devices = match &self.session.devices {
            Some(Ok(list)) => list
                .devices
                .iter()
                .map(|device| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(34.))
                        .text_color(t.body)
                        .child(icon("icons/device.svg", t.faint))
                        .child(device.label.clone())
                        .when(device.current, |line| {
                            line.child(
                                small(Text::Caption)
                                    .text_color(t.faint)
                                    .child("This device"),
                            )
                        })
                        .into_any_element()
                })
                .collect(),
            Some(Err(message)) => {
                vec![
                    small(Text::Secondary)
                        .text_color(t.faint)
                        .child(message.clone())
                        .into_any_element(),
                ]
            }
            None => vec![
                small(Text::Secondary)
                    .text_color(t.faint)
                    .child("Checking devices…")
                    .into_any_element(),
            ],
        };
        let conflicts = view.conflicts;
        vec![
            self.sync_head("Sync".to_string(), cx, t),
            div()
                .px(px(18.))
                .pt(px(12.))
                .pb(px(16.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .pt(px(6.))
                        .pb(px(14.))
                        .child(
                            div()
                                .flex_none()
                                .size(px(40.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(RADIUS_SURFACE))
                                .bg(t.raised)
                                .child(icon("icons/cloud.svg", sync_icon(t, tone)).size(px(22.))),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .text(Text::Title)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.ink)
                                        .child(head),
                                )
                                .child(
                                    small(Text::Secondary)
                                        .mt(px(2.))
                                        .text_color(t.muted)
                                        .child(sub),
                                ),
                        ),
                )
                .child(self.sync_server(t))
                .when(conflicts > 0, |body| {
                    body.child(
                        div()
                            .mb(px(6.))
                            .rounded(px(RADIUS_SURFACE))
                            .bg(t.panel)
                            .shadow(vec![t.inset_ring(t.line)])
                            .flex()
                            .items_center()
                            .justify_between()
                            .min_h(px(44.))
                            .pl(px(12.))
                            .pr(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(icon("icons/ic-conflicts.svg", t.warn))
                                    .child(format!(
                                        "{} between devices",
                                        count_text(conflicts, "conflict")
                                    )),
                            )
                            .child(
                                line_btn("sync-conflicts", "Review", t).on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.close_dialog(window, cx);
                                        this.session.close_settings();
                                        this.show_view(&ShowView(TaskQuery::Conflicts), window, cx);
                                    },
                                )),
                            ),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .pt(px(12.))
                        .pb(px(4.))
                        .text_color(t.faint)
                        .text(Text::Secondary)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Devices")
                        .child(
                            btn("sync-add", "Add device", t)
                                .h(px(20.))
                                .px(px(4.))
                                .text_color(t.muted)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.session.add_device();
                                    cx.notify();
                                }))
                                .child("Add device"),
                        ),
                )
                .children(devices)
                .into_any_element(),
            Self::footer(t)
                .justify_start()
                .child(tab_stop(
                    line_btn("sync-devices", "Manage devices", t),
                    &buttons[0],
                    t,
                    cx,
                    |this, window, cx| {
                        this.close_dialog(window, cx);
                        this.show_settings(SettingsTab::Sync, cx);
                    },
                ))
                .child(div().flex_1())
                .child(if stopped {
                    tab_stop(
                        fill_btn("sync-repair", "Pair again", t),
                        &buttons[1],
                        t,
                        cx,
                        |this, window, cx| this.confirm_reset(false, window, cx),
                    )
                } else {
                    tab_stop(
                        (if view.pending > 0 { fill_btn } else { line_btn })(
                            "sync-now",
                            if busy { "Syncing…" } else { "Sync now" },
                            t,
                        ),
                        &buttons[1],
                        t,
                        cx,
                        |this, _, cx| {
                            this.session.sync_now();
                            cx.notify();
                        },
                    )
                })
                .into_any_element(),
        ]
    }

    fn sync_title(kind: SyncKind) -> &'static str {
        match kind {
            SyncKind::Setup => "Set up sync",
            SyncKind::Join => "Join existing sync",
        }
    }

    fn back_button(
        id: &'static str,
        label: &'static str,
        to: SyncPage,
        focus: &FocusHandle,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Stateful<Div> {
        tab_stop(line_btn(id, label, t), focus, t, cx, move |this, _, cx| {
            this.session.sync_page = to.clone();
            cx.notify();
        })
    }

    fn sync_invitation(
        &self,
        kind: SyncKind,
        new_database: bool,
        input: &Entity<Editor>,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let text = input.read(cx).text().to_string();
        let (good, check) = match (kind, InvitationCheck::of(&text)) {
            (_, InvitationCheck::Empty) => (false, String::new()),
            (SyncKind::Setup, InvitationCheck::Setup(server))
            | (SyncKind::Join, InvitationCheck::Device(server)) => (true, format!("✓ {server}")),
            (SyncKind::Setup, InvitationCheck::Device(_)) => (
                false,
                "Device invitation; use Join existing sync instead".to_string(),
            ),
            (SyncKind::Join, InvitationCheck::Setup(_)) => (
                false,
                "Setup invitation; use Set up sync instead".to_string(),
            ),
            (_, InvitationCheck::Incomplete) => (
                false,
                "Incomplete invitation; part may be missing".to_string(),
            ),
            (_, InvitationCheck::Unknown) => (false, "Not an Aven invitation".to_string()),
        };
        let guide = match kind {
            SyncKind::Setup => div()
                .flex()
                .flex_wrap()
                .text_color(t.muted)
                .child("Paste the setup invitation from ")
                .child(
                    div()
                        .id("sync-cloud")
                        .role(Role::Link)
                        .aria_label("Aven Cloud")
                        .text_color(t.accent)
                        .on_click(|_, _, cx| cx.open_url(CLOUD_URL))
                        .child("Aven Cloud"),
                )
                .child(", or from ")
                .child(div().font_family(MONO).child("aven server setup"))
                .child(" if you self-host."),
            SyncKind::Join => div().text_color(t.muted).child(
                "On a device that already syncs, use Add device or run `aven sync invite`, then \
                 paste the invitation here.",
            ),
        };
        vec![
            self.sync_head(format!("Sync › {}", Self::sync_title(kind)), cx, t),
            div()
                .px(px(18.))
                .pt(px(12.))
                .pb(px(16.))
                .child(guide)
                .child(
                    div()
                        .mt(px(14.))
                        .h(px(36.))
                        .px(px(11.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_CONTROL))
                        .bg(t.page)
                        .shadow(vec![t.inset_ring(t.line2)])
                        .font_family(MONO)
                        .text_color(t.ink)
                        .child(input.clone()),
                )
                .child(
                    small(Text::Secondary)
                        .mt(px(7.))
                        .min_h(px(18.))
                        .text_color(if good { t.good } else { t.bad })
                        .child(check),
                )
                .into_any_element(),
            Self::footer(t)
                .child(Self::back_button(
                    "sync-back",
                    "Back",
                    SyncPage::Home,
                    &buttons[0],
                    cx,
                    t,
                ))
                .child(tab_stop(
                    fill_btn("sync-continue", "Continue", t)
                        .when(!good, |button| button.opacity(0.5)),
                    &buttons[1],
                    t,
                    cx,
                    move |this, _, cx| {
                        this.invitation_next(kind, new_database, cx);
                        cx.notify();
                    },
                ))
                .into_any_element(),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    fn sync_confirm(
        &self,
        kind: SyncKind,
        invitation: &Secret,
        new_database: bool,
        facts: Option<&SetupFacts>,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let server = match InvitationCheck::of(invitation.text()) {
            InvitationCheck::Setup(server) | InvitationCheck::Device(server) => server,
            _ => String::new(),
        };
        let mut rows = vec![("Server", server)];
        if let Some(facts) = facts {
            rows.push(("Workspaces", facts.workspaces.to_string()));
            rows.push(("Tasks", facts.tasks.to_string()));
        }
        let note = match kind {
            SyncKind::Setup => {
                "Includes scheduled and recurring tasks. Other devices will receive this data \
                 when they join."
            }
            SyncKind::Join => {
                "This computer will download the synced tasks, then their images. Keep Add \
                 device open on the other device until joining finishes."
            }
        };
        let line = t.line;
        vec![
            self.sync_head(format!("Sync › {}", Self::sync_title(kind)), cx, t),
            div()
                .px(px(18.))
                .pt(px(12.))
                .pb(px(16.))
                .child(
                    div()
                        .rounded(px(RADIUS_SURFACE))
                        .overflow_hidden()
                        .bg(t.panel)
                        .shadow(vec![t.inset_ring(t.line)])
                        .children(rows.into_iter().enumerate().map(|(index, (name, value))| {
                            div()
                                .flex()
                                .justify_between()
                                .gap(px(16.))
                                .px(px(12.))
                                .py(px(9.))
                                .text_color(t.ink)
                                .when(index > 0, |row| row.border_t_1().border_color(line))
                                .child(div().text_color(t.muted).child(name))
                                .child(value)
                        })),
                )
                .child(div().mt(px(12.)).text_color(t.muted).child(note))
                .when(
                    facts.is_some_and(|facts| facts.leaves_unencrypted_server),
                    |body| {
                        body.child(div().mt(px(8.)).text_color(t.muted).child(
                            "This database stops using its previous unencrypted sync server.",
                        ))
                    },
                )
                .when_some(
                    facts.filter(|facts| facts.missing_images > 0),
                    |body, facts| {
                        body.child(div().mt(px(8.)).text_color(t.muted).child(format!(
                            "{} images aren't on this device and sync as unavailable.",
                            facts.missing_images
                        )))
                    },
                )
                .into_any_element(),
            Self::footer(t)
                .child(Self::back_button(
                    "sync-back",
                    "Back",
                    SyncPage::Invitation { kind, new_database },
                    &buttons[0],
                    cx,
                    t,
                ))
                .child(tab_stop(
                    fill_btn(
                        "sync-go",
                        match kind {
                            SyncKind::Setup => "Set up sync",
                            SyncKind::Join => "Join",
                        },
                        t,
                    ),
                    &buttons[1],
                    t,
                    cx,
                    |this, window, cx| this.sync_accept(window, cx),
                ))
                .into_any_element(),
        ]
    }

    fn sync_non_empty(
        &self,
        tasks: i64,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        vec![
            self.sync_head(format!("This database already has {tasks} tasks"), cx, t),
            div()
                .px(px(18.))
                .py(px(12.))
                .text_color(t.body)
                .child(
                    "Aven can't merge existing tasks into a sync, so joining needs a new database.",
                )
                .child(small(Text::Secondary).mt(px(8.)).text_color(t.faint).child(format!(
                    "These {tasks} tasks stay in the current database. They won't sync, and they \
                     won't appear in the joined tasks. You can reopen them from Data and recovery."
                )))
                .into_any_element(),
            Self::footer(t)
                .child(Self::back_button(
                    "sync-back",
                    "Cancel",
                    SyncPage::Home,
                    &buttons[0],
                    cx,
                    t,
                ))
                .child(tab_stop(
                    fill_btn("sync-backup-join", "Back up and join", t),
                    &buttons[1],
                    t,
                    cx,
                    |this, _, cx| {
                        this.session.write(Command::Backup);
                        this.session.sync_page = SyncPage::Invitation {
                            kind: SyncKind::Join,
                            new_database: true,
                        };
                        cx.notify();
                    },
                ))
                .into_any_element(),
        ]
    }

    fn step_rows(
        kind: SyncKind,
        at: Option<Stage>,
        amount: Option<&str>,
        finished: bool,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let all = steps(kind);
        let current = match at {
            _ if finished => all.len(),
            Some(stage) => (all.iter())
                .position(|(step, _, _)| *step == stage)
                .unwrap_or(0),
            None => 0,
        };
        all.iter()
            .enumerate()
            .flat_map(|(index, (_, now, past))| {
                let done = index < current;
                let live = index == current;
                let line = div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_h(px(30.))
                    .text_color(if live {
                        t.ink
                    } else if done {
                        t.muted
                    } else {
                        t.faint
                    })
                    .child(
                        div()
                            .flex_none()
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if done {
                                div().text_color(t.good).child("✓")
                            } else {
                                div().size(px(6.)).rounded_full().bg(if live {
                                    t.accent
                                } else {
                                    t.line2
                                })
                            }),
                    )
                    .child(if done { *past } else { *now })
                    .into_any_element();
                let detail = amount.filter(|_| live).map(|amount| {
                    small(Text::Secondary)
                        .pl(px(26.))
                        .text_color(t.muted)
                        .child(amount.to_string())
                        .into_any_element()
                });
                std::iter::once(line).chain(detail)
            })
            .collect()
    }

    fn sync_running(
        &self,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let Some(run) = self.session.sync_run.as_ref() else {
            return self.sync_home(buttons, cx, t);
        };
        let kind = run.op.kind().unwrap_or(SyncKind::Setup);
        let downloaded = matches!(
            run.stage,
            Some(Stage::CatchingUp | Stage::DownloadingImages)
        );
        let note = match kind {
            SyncKind::Setup => "You can close this dialog and keep working.",
            SyncKind::Join if downloaded => {
                "Tasks are available. You can close this dialog and keep working while the rest \
                 downloads."
            }
            SyncKind::Join => {
                "You can close this dialog. Editing waits until tasks are downloaded. Keep Add \
                 device open on the other device."
            }
        };
        vec![
            Self::heading(
                match kind {
                    SyncKind::Setup => "Setting up sync",
                    SyncKind::Join => "Joining sync",
                }
                .to_string(),
                t,
            )
            .into_any_element(),
            div()
                .px(px(18.))
                .pt(px(12.))
                .pb(px(16.))
                .children(Self::step_rows(
                    kind,
                    run.stage,
                    run.amount.as_deref(),
                    false,
                    t,
                ))
                .child(div().mt(px(12.)).text_color(t.muted).child(note))
                .into_any_element(),
            Self::footer(t)
                .child(tab_stop(
                    line_btn("sync-close-op", "Close", t),
                    &buttons[0],
                    t,
                    cx,
                    |this, window, cx| this.close_dialog(window, cx),
                ))
                .into_any_element(),
        ]
    }

    /// The switch sync setup and join end on, on by default.
    fn sync_when_quit_choice(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        let on = self.session.settings_data.background.sync_when_quit;
        let track = div()
            .id("sync-done-when-quit")
            .role(Role::Switch)
            .aria_label("Sync when Aven is quit")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.session.set_sync_when_quit(!on);
                this.mark_switched("sync-done-when-quit");
                cx.notify();
            }));
        div()
            .mt(px(14.))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(row_title(
                "Sync when Aven is quit",
                Some(
                    "A background agent keeps this Mac up to date while Aven is closed."
                        .to_string(),
                ),
                t,
            ))
            .child(self.switch_track(track, "sync-done-when-quit", on, 1., cx, t))
    }

    fn sync_done(
        &self,
        kind: SyncKind,
        note: &str,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let setup = kind == SyncKind::Setup;
        let sub = if setup {
            "Add your other devices with an invitation from this one.".to_string()
        } else if note.starts_with("Joined sync") {
            "Your tasks are on this device.".to_string()
        } else {
            note.to_string()
        };
        vec![
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(18.))
                .pt(px(14.))
                .text(Text::Title)
                .font_weight(FontWeight::SEMIBOLD)
                .child(
                    div()
                        .flex_none()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(t.good)
                        .text_color(t.page)
                        .text(Text::Secondary)
                        .child("✓"),
                )
                .child(if setup {
                    "Sync is set up"
                } else {
                    "Joined sync"
                })
                .into_any_element(),
            div()
                .px(px(18.))
                .pt(px(12.))
                .pb(px(16.))
                .children(Self::step_rows(kind, None, None, true, t))
                .child(div().mt(px(12.)).text_color(t.muted).child(sub))
                .child(self.sync_when_quit_choice(cx, t))
                .into_any_element(),
            Self::footer(t)
                .child(tab_stop(
                    (if setup { line_btn } else { fill_btn })("sync-done", "Done", t),
                    &buttons[0],
                    t,
                    cx,
                    |this, window, cx| this.close_dialog(window, cx),
                ))
                .when(setup, |foot| {
                    foot.child(tab_stop(
                        fill_btn("sync-done-add", "Add device", t),
                        &buttons[1],
                        t,
                        cx,
                        |this, _, cx| {
                            this.session.add_device();
                            cx.notify();
                        },
                    ))
                })
                .into_any_element(),
        ]
    }

    fn sync_failed(
        &self,
        message: &str,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        vec![
            self.sync_head("Sync didn't finish".to_string(), cx, t),
            div()
                .px(px(18.))
                .py(px(12.))
                .text_color(t.body)
                .child(message.to_string())
                .into_any_element(),
            Self::footer(t)
                .child(Self::back_button(
                    "sync-back",
                    "Back",
                    SyncPage::Home,
                    &buttons[0],
                    cx,
                    t,
                ))
                .into_any_element(),
        ]
    }

    fn sync_add_device(
        &self,
        buttons: &[FocusHandle; 2],
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let Some(invitation) = self.session.invitation.as_ref() else {
            return vec![
                Self::heading("Add device".to_string(), t).into_any_element(),
                div()
                    .px(px(18.))
                    .py(px(16.))
                    .text_color(t.muted)
                    .child("Creating an invitation…")
                    .into_any_element(),
            ];
        };
        let minutes = (invitation.expires_at as i64 - Local::now().timestamp() + 59).max(0) / 60;
        let text = invitation.text.text().to_string();
        let copied = invitation.text.clone();
        vec![
            Self::heading("Add device".to_string(), t).into_any_element(),
            div()
                .px(px(18.))
                .pt(px(12.))
                .pb(px(16.))
                .flex()
                .flex_col()
                .items_center()
                .child(qr_code(&text))
                .child(
                    div()
                        .mt(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Scan with Aven on the new device"),
                )
                .child(
                    small(Text::Secondary)
                        .text_color(t.muted)
                        .child(format!("Expires in {minutes} minutes")),
                )
                .child(
                    small(Text::Secondary)
                        .mt(px(14.))
                        .text_color(t.faint)
                        .child("or on a computer, paste this invitation"),
                )
                .child(
                    div()
                        .mt(px(6.))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .id("pair-code")
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .font_family(MONO)
                                .text(Text::Secondary)
                                .child(text.clone()),
                        )
                        .child(tab_stop(
                            line_btn("pair-copy", "Copy", t),
                            &buttons[0],
                            t,
                            cx,
                            move |this, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    copied.text().to_string(),
                                ));
                                this.session.show_toast(
                                    Toast::warn("Invitation copied")
                                        .detail("Anyone who has it can read your synced tasks"),
                                );
                                cx.notify();
                            },
                        )),
                )
                .into_any_element(),
            Self::footer(t)
                .child(
                    small(Text::Secondary)
                        .flex_1()
                        .text_color(t.muted)
                        .child("Waiting for the new device"),
                )
                .child(tab_stop(
                    line_btn("pair-cancel", "Cancel", t),
                    &buttons[1],
                    t,
                    cx,
                    |this, _, cx| {
                        this.session.request_sync(SyncRequest::CancelInvitation);
                        cx.notify();
                    },
                ))
                .into_any_element(),
        ]
    }
}

/// The invitation as a QR code, on white so scanners read it in either theme.
fn qr_code(text: &str) -> AnyElement {
    let Ok(code) = QrCode::with_error_correction_level(text, EcLevel::L) else {
        return div().into_any_element();
    };
    let width = code.width();
    let colors = code.to_colors();
    div()
        .id("pair-qr")
        .size(px(QR_SIZE))
        .role(Role::Image)
        .aria_label("Pairing code")
        .rounded(px(RADIUS_SURFACE))
        .bg(gpui::white())
        .p(px(10.))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<gpui::Pixels>, _, window, _| {
                    let cell = bounds.size.width / width as f32;
                    for (index, color) in colors.iter().enumerate() {
                        if *color == Color::Dark {
                            let (x, y) = ((index % width) as f32, (index / width) as f32);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(bounds.origin.x + cell * x, bounds.origin.y + cell * y),
                                    size(cell, cell),
                                ),
                                gpui::black(),
                            ));
                        }
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// The notice the first Quit shows about what the background agent does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuitNotice {
    title: &'static str,
    body: &'static str,
    /// Whether the notice offers System Settings, where notifications for
    /// Aven are turned on.
    notification_settings: bool,
}

/// What the first Quit says, or `None` when nothing the agent does after
/// quitting would be seen: reminders alone, with notifications off.
/// Undecided notifications count as off, since only the app asks.
fn quit_notice(reminders: bool, sync: bool, permission: Permission) -> Option<QuitNotice> {
    let shown = !matches!(permission, Permission::Denied | Permission::NotAsked);
    let (title, body, notification_settings) = match (reminders, sync, shown) {
        (true, true, true) => (
            "Reminders and sync keep running",
            "After you quit, a background agent continues to send reminders and sync. You \
             can turn this off in Settings.",
            false,
        ),
        (true, false, true) => (
            "Reminders keep coming",
            "After you quit, a background agent continues to send reminders. You can turn \
             this off in Settings.",
            false,
        ),
        (true, true, false) => (
            "Sync keeps running",
            "After you quit, a background agent continues to sync. Reminders won\u{2019}t \
             show because notifications for Aven are off.",
            true,
        ),
        (true, false, false) => return None,
        (false, ..) => (
            "Sync keeps running",
            "After you quit, a background agent continues to sync. You can turn this off \
             in Settings.",
            false,
        ),
    };
    Some(QuitNotice {
        title,
        body,
        notification_settings,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_quit_notice_follows_the_notification_permission() {
        use super::{QuitNotice, quit_notice};
        use crate::notifications::Permission::*;
        let title = |notice: Option<QuitNotice>| notice.map(|notice| notice.title);
        for allowed in [Allowed, Unavailable] {
            assert_eq!(
                title(quit_notice(true, true, allowed)),
                Some("Reminders and sync keep running")
            );
            assert_eq!(
                title(quit_notice(true, false, allowed)),
                Some("Reminders keep coming")
            );
            assert!(
                !quit_notice(true, true, allowed)
                    .unwrap()
                    .notification_settings
            );
        }
        for off in [Denied, NotAsked] {
            let notice = quit_notice(true, true, off).unwrap();
            assert_eq!(notice.title, "Sync keeps running");
            assert!(notice.body.contains("notifications for Aven are off"));
            assert!(notice.notification_settings);
            assert_eq!(quit_notice(true, false, off), None);
        }
        for permission in [Allowed, Denied, NotAsked, Unavailable] {
            let notice = quit_notice(false, true, permission).unwrap();
            assert_eq!(notice.title, "Sync keeps running");
            assert!(!notice.body.contains("Reminders") && !notice.notification_settings);
        }
    }

    #[test]
    fn return_quits_from_the_quit_notice_but_never_confirms_a_loss() {
        use crate::dialogs::ConfirmAction;
        for notification_settings in [false, true] {
            let quit = ConfirmAction::Quit {
                notification_settings,
            };
            assert_eq!(quit.first_button(), 1);
        }
        let losses = [
            ConfirmAction::DeleteProject("app".into()),
            ConfirmAction::RemoveDevice {
                id: [0; 32],
                label: "Phone".into(),
            },
            ConfirmAction::Restore("backup.aven".into()),
            ConfirmAction::Reset {
                unfinished_setup: false,
            },
            ConfirmAction::OpenKeptDraft(1),
        ];
        for action in losses {
            assert_eq!(action.first_button(), 0);
        }
    }

    use super::*;

    #[test]
    fn the_notifications_row_counts_what_was_not_shown() {
        assert_eq!(missed_note(0), None);
        assert_eq!(
            missed_note(1).as_deref(),
            Some("1 notification this week wasn't shown")
        );
        assert_eq!(
            missed_note(3).as_deref(),
            Some("3 notifications this week weren't shown")
        );
    }

    #[test]
    fn command_lines_mark_the_command_and_placeholders() {
        let setup = AgentSetup {
            command: "claude".to_string(),
            args: vec![
                "-p".to_string(),
                "--dir={image_dir}".to_string(),
                "{prompt}".to_string(),
            ],
            runnable: true,
        };
        let (line, spans) = command_line(&setup);
        assert_eq!(line, "claude -p --dir={image_dir} {prompt}");
        let marked = spans
            .iter()
            .map(|(range, part)| (&line[range.clone()], *part))
            .collect::<Vec<_>>();
        assert_eq!(
            marked,
            [
                ("claude", Part::Command),
                ("{image_dir}", Part::Placeholder),
                ("{prompt}", Part::Placeholder),
            ]
        );
    }

    #[test]
    fn test_chips_name_the_project_and_skip_empty_fields() {
        let projects = [ProjectEntry {
            key: "home".to_string(),
            name: "Home".to_string(),
            prefix: "HOME".to_string(),
        }];
        let task = NewTask {
            project: Some("home".to_string()),
            schedule: "due fri".to_string(),
            priority: "high".to_string(),
            labels: vec!["bills".to_string()],
            ..NewTask::titled("Pay rent")
        };
        assert_eq!(
            test_chips(&task, &projects),
            ["Home", "due fri", "! High", "#bills"]
        );
        assert!(test_chips(&NewTask::titled("Pay rent"), &projects).is_empty());
    }
}
