//! The settings surface, the Sync dialog and data recovery.
use std::path::PathBuf;
use std::rc::Rc;

use accesskit::Role;
use aven_core::sync::client::engine::Stage;
use aven_core::sync::client::invitation::InvitationCheck;
use aven_host::store::TaskQuery;
use chrono::Local;
use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, Div, Entity, FocusHandle, FontWeight, Hsla,
    SharedString, Stateful, Window, canvas, div, fill, point, prelude::*, size,
};
use qrcode::{Color, EcLevel, QrCode};

use crate::actions::*;
use crate::adapter::{Command, Reminders, SettingsEdit, SyncPhase, ThemePref};
use crate::dialogs::{ConfirmAction, Dialog, Layer};
use crate::input::{Editor, Mode};
use crate::kit::*;
use crate::measure::measured;
use crate::session::{SettingsTab, SyncPage, SyncTone, count_text};
use crate::shell::Root;
use crate::sync_ops::{Secret, SetupFacts, SyncKind, SyncOp, SyncRequest, steps};
use crate::theme::{Theme, theme};
use crate::workflows::tab_stop;

const TABS: [(SettingsTab, &str); 9] = [
    (SettingsTab::General, "General"),
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
const QR_SIZE: f32 = 184.;

fn row_title(title: impl Into<SharedString>, note: Option<String>, t: &Theme) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .child(title.into())
        .when_some(note, |row, note| {
            row.child(small(12.).mt(px(1.)).text_color(t.muted).child(note))
        })
}

fn path_title(title: &'static str, path: String, t: &Theme) -> Div {
    div().flex_1().min_w_0().child(title).child(
        small(11.5)
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
        .bg(t.panel)
        .text_color(t.ink)
}

/// Rows joined into one panel, with lines between them.
fn group(rows: Vec<Div>, t: &Theme) -> Div {
    let line = t.line;
    div()
        .relative()
        .rounded(px(10.))
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
                .rounded(px(10.))
                .border_1()
                .border_color(t.line),
        )
}

fn section(text: &'static str, t: &Theme) -> Div {
    small(12.)
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
                .text_size(px(20.))
                .line_height(px(26.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.ink)
                .child(title),
        )
        .when_some(lead, |page, lead| {
            page.child(div().mb(px(20.)).text_color(t.muted).child(lead))
        })
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
    pub fn show_settings(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        self.session.open_settings(tab);
        cx.notify();
    }

    pub(crate) fn open_settings(
        &mut self,
        _: &OpenSettings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                    root.session.show_toast("Add a name".to_string(), None);
                } else {
                    root.session.write(Command::Settings(edit(text)));
                }
                cx.notify();
            }),
            window,
            cx,
        );
    }

    fn open_project(
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
                    body.child(small(12.).mt(px(8.)).text_color(t.bad).child(error))
                })
                .into_any_element(),
            self.cancel_and_accept("Save", cx, t).into_any_element(),
        ]
    }

    fn open_confirm(
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
        let dialog = Dialog::Confirm {
            title,
            body,
            note,
            label,
            action,
            buttons,
        };
        // Cancel takes focus, so Enter alone never confirms.
        self.open_dialog_at(dialog, Vec::new(), fields, 0, window, cx);
    }

    fn confirmed(&mut self, action: ConfirmAction, window: &mut Window, cx: &mut Context<Self>) {
        self.close_dialog(window, cx);
        match action {
            ConfirmAction::RemoveDevice { id, label } => {
                self.session.run_sync(SyncOp::RemoveDevice { id, label });
            }
            ConfirmAction::Restore(path) => self.session.write(Command::Restore(path)),
            ConfirmAction::Reset { unfinished_setup } => {
                self.session.run_sync(SyncOp::Reset { unfinished_setup });
            }
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
                    action,
                    buttons: [cancel, confirm],
                },
            ..
        }) = self.dialog.as_ref()
        else {
            return Vec::new();
        };
        let action = action.clone();
        vec![
            Self::heading(title.clone(), t).into_any_element(),
            div()
                .px(px(18.))
                .py(px(12.))
                .text_color(t.body)
                .child(body.clone())
                .when_some(*note, |body, note| {
                    body.child(small(12.).mt(px(8.)).text_color(t.faint).child(note))
                })
                .into_any_element(),
            Self::footer(t)
                .child(tab_stop(
                    line_btn("confirm-cancel", "Cancel", t),
                    cancel,
                    t,
                    cx,
                    |this, window, cx| this.close_dialog(window, cx),
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

    fn pick_archive(&mut self, cx: &mut Context<Self>) {
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

    fn open_other_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_prompt(
            "Open another database".to_string(),
            "Database path",
            "",
            "",
            Rc::new(|root, text, _, cx| {
                let text = text.trim();
                if text.is_empty() {
                    root.session.show_toast("Add a path".to_string(), None);
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
        measured(div(), "settings")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .child(
                measured(div(), "settings-nav")
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
                    .child(
                        measured(div(), "settings-page")
                            .w_full()
                            .max_w(px(620.))
                            .mx_auto()
                            .child(body),
                    ),
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
            .rounded(px(7.))
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
                    .rounded(px(5.))
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
    ) -> Stateful<Div> {
        div()
            .id(name)
            .role(Role::Switch)
            .aria_label(name)
            .flex_none()
            .relative()
            .w(px(34.))
            .h(px(20.))
            .rounded(px(99.))
            .bg(if on { t.fill } else { t.line2 })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_reminders(edit);
                cx.notify();
            }))
            .child(
                div()
                    .absolute()
                    .top(px(2.))
                    .left(px(if on { 16. } else { 2. }))
                    .size(px(16.))
                    .rounded_full()
                    .bg(gpui::white()),
            )
    }

    fn general_tab(&self, _: &Window, cx: &mut Context<Self>, t: &Theme) -> Div {
        let themes = [
            (ThemePref::Dark, "Dark"),
            (ThemePref::Light, "Light"),
            (ThemePref::System, "System"),
        ];
        let zooms = [(92, "Small"), (100, "Default"), (110, "Large")];
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
                        .child(row_title(
                            "Aven",
                            Some(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                            t,
                        ))
                        .child(
                            line_btn("open-about", "About Aven", t).on_click(cx.listener(
                                |this, _, window, cx| this.open_about(&OpenAbout, window, cx),
                            )),
                        ),
                ],
                t,
            ))
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
        .when(set_up, |tab| {
            tab.child(section("Devices", t))
                .child(group(self.device_rows(cx, t), t))
                .when(unfinished, |tab| {
                    tab.child(small(12.).pt(px(7.)).px(px(2.)).text_color(t.warn).child(
                        "Securing future changes is unfinished. Finish removal continues \
                                 it, and any remaining device finishes it when it syncs.",
                    ))
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
        let digest = reminders.digest_time.clone();
        page(
            "Reminders",
            Some(
                "Sent while the app is closed. Each device keeps its own record of sent reminders.",
            ),
            t,
        )
        .child(group(
            vec![
                row(t)
                    .child(row_title("Reminders", None, t))
                    .child(self.switch(
                        "Reminders",
                        reminders.enabled,
                        |value| value.enabled = !value.enabled,
                        cx,
                        t,
                    )),
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
                        .rounded(px(6.))
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
            Some("Trusted local programs. They appear in the palette and run on the selected or marked tasks."),
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
                        small(12.)
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
                small(12.)
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
                            .text_size(px(26.))
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
                .show_toast("Paste the invitation first.".to_string(), None);
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
                            .rounded(px(10.))
                            .overflow_hidden()
                            .shadow(vec![t.inset_ring(t.line)])
                            .child(self.option_row(
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
                            ))
                            .child(div().h(px(1.)).bg(t.line))
                            .child(self.option_row(
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
                            )),
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
                            line.child(small(11.5).text_color(t.faint).child("This device"))
                        })
                        .into_any_element()
                })
                .collect(),
            Some(Err(message)) => {
                vec![
                    small(12.)
                        .text_color(t.faint)
                        .child(message.clone())
                        .into_any_element(),
                ]
            }
            None => vec![
                small(12.)
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
                                .rounded(px(10.))
                                .bg(t.raised)
                                .child(icon("icons/cloud.svg", sync_icon(t, tone)).size(px(22.))),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.ink)
                                        .child(head),
                                )
                                .child(small(12.5).mt(px(2.)).text_color(t.muted).child(sub)),
                        ),
                )
                .when(conflicts > 0, |body| {
                    body.child(
                        div()
                            .mb(px(6.))
                            .rounded(px(8.))
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
                        .text_size(px(12.))
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
                        .rounded(px(8.))
                        .bg(t.page)
                        .shadow(vec![t.inset_ring(t.line2)])
                        .font_family(MONO)
                        .text_color(t.ink)
                        .child(input.clone()),
                )
                .child(
                    small(12.)
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
                        .rounded(px(8.))
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
                .child(small(12.).mt(px(8.)).text_color(t.faint).child(format!(
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
                    small(12.)
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
                .text_size(px(15.))
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
                        .text_size(px(12.))
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
                    small(12.)
                        .text_color(t.muted)
                        .child(format!("Expires in {minutes} minutes")),
                )
                .child(
                    small(12.)
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
                                .text_size(px(12.))
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
                                    "Invitation copied: it grants access to your synced data"
                                        .to_string(),
                                    None,
                                );
                                cx.notify();
                            },
                        )),
                )
                .into_any_element(),
            Self::footer(t)
                .child(
                    small(12.)
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
        .rounded(px(8.))
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
