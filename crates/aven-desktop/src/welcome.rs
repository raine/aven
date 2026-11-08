//! The welcome: a database with no tasks that was never welcomed opens here,
//! on a field for the first task, instead of on an empty Queue.
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use aven_core::projects::normalize_key;
use gpui::{
    AnyElement, Bounds, Context, Div, Entity, FontWeight, KeyContext, Pixels, Subscription, Window,
    canvas, div, img, prelude::*, relative,
};

use crate::actions::*;
use crate::adapter::{SyncPhase, first_project_prefix};
use crate::draft::{Draft, parse_title};
use crate::input::{Editor, EditorEvent, Mode};
use crate::intro::{self, Colors, LANDED};
use crate::kit::*;
use crate::shell::Root;
use crate::sync_ops::SyncRequest;
use crate::theme::{Theme, theme};
use crate::toast::Toast;
use crate::tooltip::{Hint, Hinted};

/// The width of the welcome's column.
const COLUMN: f32 = 460.;
const LOGO: f32 = 56.;

pub(crate) struct Welcome {
    input: Entity<Editor>,
    /// The logo's bounds as of the last frame, where an intro can land.
    pub logo: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// While the intro plays, takes every keystroke to skip it.
    intro_keys: Option<Subscription>,
    _subscription: Subscription,
}

/// The project the first task goes to, as the hint row names it.
struct Destination {
    name: String,
    key: String,
    prefix: String,
    /// The project exists already, so naming it is an ordinary edit.
    exists: bool,
}

/// One block of the column. Each is its own element, so an intro can fade
/// and lift them one at a time.
fn block() -> Div {
    div().w(px(COLUMN)).flex().flex_col().items_center()
}

/// Fades and lowers `el` as intro item `index` at `t`, or leaves it at rest.
fn rise(el: Div, index: usize, t: Option<f32>) -> Div {
    el.when_some(t, |el, t| {
        let rise = intro::rise(index, t);
        el.relative().top(px(rise.y)).opacity(rise.opacity)
    })
}

impl Root {
    /// Builds the welcome while the database waits for one, and drops it after.
    pub(crate) fn sync_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let due = self.session.snapshot.welcome && self.session.database_error.is_none();
        if due == self.welcome.is_some() {
            return;
        }
        if !due {
            self.intro.skip();
            self.welcome = None;
            if !self.composer_open(cx) && !self.dialog_open() {
                let focus = self.focus.clone();
                cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
            }
            return;
        }
        let input = cx.new(|cx| {
            Editor::new(
                Mode::Single,
                "What's the first thing on your mind?",
                window,
                cx,
            )
        });
        let intro_keys = if self.intro.start(Instant::now()) {
            // The field takes focus when the intro ends, so nothing is typed into
            // it before it shows.
            cx.defer_in(window, |this, window, cx| window.focus(&this.focus, cx));
            let this = cx.weak_entity();
            Some(cx.intercept_keystrokes(move |event, _, cx| {
                let _ = this.update(cx, |root, cx| {
                    // Lives only until the render after the intro ends.
                    if root.intro.skip_key(&event.keystroke) {
                        cx.stop_propagation();
                    }
                    cx.notify();
                });
            }))
        } else {
            let focus = input.read(cx).focus.clone();
            cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
            None
        };
        self.welcome = Some(Welcome {
            _subscription: cx.subscribe_in(&input, window, Self::on_welcome_event),
            input,
            logo: Rc::default(),
            intro_keys,
        });
    }

    fn on_welcome_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Submit => self.add_first_task(window, cx),
            EditorEvent::Cancel => window.focus(&self.focus, cx),
            EditorEvent::Changed => cx.notify(),
            _ => {}
        }
    }

    fn welcome_text(&self, cx: &Context<Self>) -> String {
        self.welcome
            .as_ref()
            .map(|welcome| welcome.input.read(cx).text().trim().to_string())
            .unwrap_or_default()
    }

    fn add_first_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = Draft {
            title: self.welcome_text(cx),
            ..Draft::default()
        };
        // An empty title, the only thing a bare title can get wrong, adds nothing.
        let Ok(task) = draft.task(&self.session.snapshot.projects) else {
            return;
        };
        self.session.create_first_task(task);
        self.welcome = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn focus_welcome(
        &mut self,
        _: &FocusWelcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(welcome) = &self.welcome {
            let focus = welcome.input.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
    }

    /// Opens the composer on what the field holds.
    pub(crate) fn welcome_details(
        &mut self,
        _: &WelcomeDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.welcome_text(cx);
        if !title.is_empty() {
            self.composer
                .update(cx, |composer, _| composer.set_title(title));
        }
        self.open_composer(window, cx);
    }

    fn skip_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.skip_welcome();
        self.welcome = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn destination(&self) -> Destination {
        let projects = &self.session.snapshot.projects;
        let scoped = self.session.snapshot.view.project.as_ref();
        let existing = scoped
            .and_then(|key| projects.iter().find(|project| project.key == *key))
            .or_else(|| projects.first());
        match existing {
            Some(project) => Destination {
                name: project.name.clone(),
                key: project.key.clone(),
                prefix: project.prefix.clone(),
                exists: true,
            },
            None => {
                let name = self.session.first_project_name();
                Destination {
                    key: normalize_key(&name),
                    prefix: first_project_prefix(&name),
                    name,
                    exists: false,
                }
            }
        }
    }

    fn name_first_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let destination = self.destination();
        if destination.exists {
            let Destination {
                key, name, prefix, ..
            } = destination;
            self.open_project(Some(key), &name, &prefix, window, cx);
            return;
        }
        self.open_prompt(
            "Name your first project".to_string(),
            "Project name",
            "You can rename it any time in Settings.",
            &destination.name,
            Rc::new(|root, text, _, cx| {
                let text = text.trim();
                if normalize_key(text).is_empty() {
                    root.session.show_toast(Toast::info("Add a name"));
                } else {
                    root.session.name_first_project(text.to_string());
                }
                cx.notify();
            }),
            window,
            cx,
        );
    }

    fn join_from_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_sync(&OpenSync, window, cx);
        // The Sync dialog's own Join option, taken for the user.
        if self.session.snapshot.sync.phase == SyncPhase::Local {
            self.session.request_sync(SyncRequest::CheckJoin);
        }
        cx.notify();
    }

    pub(crate) fn render_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = theme(cx);
        let mut key_context = KeyContext::default();
        key_context.add("Welcome");
        if self.menu.is_some() || self.composer_open(cx) || self.dialog_open() {
            key_context.add("Overlay");
        }
        let intro = self.intro.frame(|started| intro::elapsed(started, cx));
        if intro.is_some() {
            window.request_animation_frame();
        } else if let Some(welcome) = self.welcome.as_mut()
            && welcome.intro_keys.take().is_some()
        {
            let focus = welcome.input.read(cx).focus.clone();
            cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
        }
        let content = self.render_welcome_column(intro, window, cx);
        let overlay = intro.and_then(|intro| self.render_intro(intro, cx));
        let composer = self.render_composer(window, cx);
        let dialog = self.render_dialog(window, cx);
        let menu = self.render_menu(window, cx);
        let toast = self.render_toast(cx);
        let toast_action = self
            .session
            .toast()
            .is_some_and(|toast| toast.action.is_some());
        let unblocked = !self.composer_open(cx) && !self.dialog_open();
        div()
            .relative()
            .size_full()
            .bg(t.page)
            .text_color(t.ink)
            .font_family(".SystemUIFont")
            .text(Text::Body)
            .line_height(px(UI_LINE_HEIGHT))
            .key_context(key_context)
            .track_focus(&self.focus)
            .when(unblocked, |el| {
                el.on_action(cx.listener(Self::open_help))
                    .on_action(cx.listener(Self::new_task))
                    .on_action(cx.listener(Self::focus_welcome))
                    .on_action(cx.listener(Self::welcome_details))
                    .on_action(cx.listener(Self::open_about))
            })
            .when(toast_action, |el| {
                el.on_action(cx.listener(Self::run_toast_action))
            })
            .on_action(cx.listener(Self::open_sync))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::next_field))
            .on_action(cx.listener(Self::previous_field))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::zoom))
            .on_action(cx.listener(Self::toggle_full_screen))
            .child(content)
            .children(composer)
            .children(dialog)
            .children(menu)
            .children(toast)
            .children(overlay)
            .map(|root| crate::tooltip::host(root, window, cx, &t))
    }

    /// The intro over the whole window. A click skips it and reaches nothing
    /// below.
    fn render_intro(&self, t: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let slot = self.welcome.as_ref()?.logo.clone();
        let colors = Colors::new(&theme(cx));
        Some(
            div()
                .id("welcome-intro")
                .absolute()
                .inset_0()
                .occlude()
                .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    if this.intro.skip() {
                        cx.notify();
                    }
                }))
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            if let Some(slot) = slot.get() {
                                intro::paint(window, t, bounds, slot, colors);
                            }
                        },
                    )
                    .size_full(),
                )
                .into_any_element(),
        )
    }

    /// The welcome's content, as of intro time `intro` while the intro plays.
    fn render_welcome_column(
        &self,
        intro: Option<f32>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let Some(welcome) = self.welcome.as_ref() else {
            return div().into_any_element();
        };
        let slot = welcome.logo.clone();
        // While the intro plays, the mark it lands is painted in place of the
        // logo image.
        let landed = intro.map(|t| t >= LANDED);
        let colors = Colors::new(&t);
        let logo = div()
            .relative()
            .size(px(LOGO))
            .mb(px(20.))
            .when(landed.is_none(), |logo| {
                logo.child(
                    img(if t.dark {
                        "logo-dark.svg"
                    } else {
                        "logo-light.svg"
                    })
                    .size_full(),
                )
            })
            .child(
                canvas(
                    move |bounds, _, _| slot.set(Some(bounds)),
                    move |bounds, _, window, _| {
                        if landed == Some(true) {
                            intro::paint_mark(window, bounds, colors);
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        let input = &welcome.input;
        let focused = input.read(cx).focus.is_focused(window);
        let typed = !parse_title(input.read(cx).text(), &self.session.snapshot.projects)
            .title
            .is_empty();
        let destination = self.destination();
        let resuming = self.session.snapshot.sync.phase == SyncPhase::JoinIncomplete;
        let (join_title, join_detail) = if resuming {
            ("Resume joining", "Continue downloading your tasks")
        } else {
            (
                "Join existing sync",
                "Use an invitation from a computer that syncs",
            )
        };
        let database = self.session.database.display().to_string();

        let column =
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px(px(24.))
                .pt(px(32.))
                .pb(px(88.))
                .child(logo)
                .child(rise(
                    block().child(
                        div()
                            .text(Text::Display)
                            .line_height(px(31.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Welcome to Aven"),
                    ),
                    0,
                    intro,
                ))
                .child(rise(
                    block().child(
                        div()
                            .max_w(px(380.))
                            .mt(px(8.))
                            .text(Text::Body)
                            .line_height(px(20.))
                            .text_center()
                            .text_color(t.muted)
                            .child(
                                "Local-first tasks for you and your coding agents. Everything \
                             stays on this Mac until you choose to sync.",
                            ),
                    ),
                    1,
                    intro,
                ))
                .child(rise(
                    block()
                        .mt(px(24.))
                        .child(self.welcome_field(focused, typed, cx, &t)),
                    2,
                    intro,
                ))
                .child(rise(
                    block().mt(px(8.)).child(
                        small(Text::Secondary)
                            .w_full()
                            .px(px(2.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .text_color(t.faint)
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .child("Adds to")
                                    .child(self.project_chip(destination, cx, &t)),
                            )
                            .child(
                                link("welcome-details", &t)
                                    .flex_none()
                                    .child("More details")
                                    .child(kbd("⌘↩", t.faint))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.welcome_details(&WelcomeDetails, window, cx);
                                    })),
                            ),
                    ),
                    3,
                    intro,
                ))
                // The divider and the cards under it come in as one.
                .child(rise(
                    block().mt(px(34.)).mb(px(12.)).child(
                        small(Text::Secondary)
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .text_color(t.faint)
                            .child(div().flex_1().h(px(1.)).bg(t.line))
                            .child("Already use Aven?")
                            .child(div().flex_1().h(px(1.)).bg(t.line)),
                    ),
                    4,
                    intro,
                ))
                .child(rise(
                    block().child(
                        div()
                            .w_full()
                            .flex()
                            .gap(px(8.))
                            .child(
                                option_card(
                                    "welcome-join",
                                    "icons/cloud.svg",
                                    join_title,
                                    join_detail,
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.join_from_welcome(window, cx);
                                    },
                                )),
                            )
                            .child(
                                option_card(
                                    "welcome-restore",
                                    "icons/ic-recent.svg",
                                    "Restore a backup",
                                    "Bring back tasks from an archive",
                                    &t,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.pick_archive(cx))),
                            ),
                    ),
                    4,
                    intro,
                ))
                .child(rise(
                    block().mt(px(22.)).child(
                        small(Text::Secondary)
                            .flex()
                            .gap(px(18.))
                            .child(
                                // The app has no demo database to open yet.
                                div()
                                    .id("welcome-demo")
                                    .text_color(t.faint)
                                    .opacity(0.6)
                                    .hint(Hint::new("Not available in the app yet"))
                                    .child("Try the demo"),
                            )
                            .child(
                                link("welcome-skip", &t)
                                    .child("Skip to an empty queue")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.skip_welcome(window, cx)
                                    })),
                            )
                            .child(
                                link("welcome-help", &t)
                                    .child("Keyboard shortcuts")
                                    .child(kbd("?", t.faint))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_help(&OpenHelp, window, cx);
                                    })),
                            ),
                    ),
                    5,
                    intro,
                ));

        let footer = div()
            .absolute()
            .left(px(18.))
            .right(px(18.))
            .bottom(px(14.))
            // The database bar comes in with the links above it.
            .child(rise(
                small(Text::Secondary)
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .text_color(t.faint)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .text(Text::Caption)
                            .child(database),
                    )
                    .child(
                        link("welcome-open-database", &t)
                            .child("Open another database")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_other_database(window, cx);
                            })),
                    ),
                5,
                intro,
            ));

        // The title bar is transparent, so the strip under it moves the window.
        let title_bar = div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(40.))
            .on_mouse_down(gpui::MouseButton::Left, drag_window);
        div()
            .relative()
            .size_full()
            .child(column)
            .child(title_bar)
            .child(footer)
            .into_any_element()
    }

    fn welcome_field(
        &self,
        focused: bool,
        typed: bool,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> impl IntoElement + use<> {
        let Some(welcome) = self.welcome.as_ref() else {
            return div().into_any_element();
        };
        let ring = if focused {
            vec![t.ring(t.tint_line), spread(t.tint, 4.)]
        } else {
            vec![t.ring(t.line2)]
        };
        let add = fill_btn("welcome-add", "Add", t)
            .h(px(32.))
            .child(kbd("↩", t.on_fill))
            .when(typed, |add| {
                add.on_click(cx.listener(|this, _, window, cx| this.add_first_task(window, cx)))
            })
            .when(!typed, |add| add.opacity(0.35).cursor_default());
        div()
            .w_full()
            .h(px(48.))
            .pl(px(15.))
            .pr(px(7.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(RADIUS_SURFACE))
            .bg(t.panel)
            .shadow(ring)
            .text(Text::Title)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .key_context("WelcomeInput")
                    .child(welcome.input.clone()),
            )
            .child(add)
            .into_any_element()
    }

    fn project_chip(
        &self,
        destination: Destination,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> impl IntoElement + use<> {
        let (panel, ink) = (t.panel, t.ink);
        div()
            .id("welcome-project")
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(22.))
            .ml(px(6.))
            .pr(px(7.))
            .rounded(px(RADIUS_CHIP))
            .shadow(vec![t.inset_ring(t.line)])
            .text_color(t.body)
            .hover(move |style| style.bg(panel).text_color(ink))
            .hint(Hint::new(if destination.exists {
                "Your first project. Edit it here or any time in Settings."
            } else {
                "Your first project. Rename it now, or any time in Settings."
            }))
            .child(project_dot(&destination.key, t))
            .child(destination.name)
            .child(
                div()
                    .ml(px(4.))
                    .font_family(MONO)
                    .text(Text::Caption)
                    .text_color(t.faint)
                    .child(destination.prefix),
            )
            .on_click(cx.listener(|this, _, window, cx| this.name_first_project(window, cx)))
    }
}

/// A quiet text button.
fn link(id: &'static str, t: &Theme) -> gpui::Stateful<Div> {
    let ink = t.ink;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .text_color(t.muted)
        .hover(move |style| style.text_color(ink))
}

fn option_card(
    id: &'static str,
    icon_path: &'static str,
    title: &'static str,
    detail: &'static str,
    t: &Theme,
) -> gpui::Stateful<Div> {
    let panel = t.panel;
    // An equal share of the row whatever the text, which wraps inside.
    div()
        .id(id)
        .flex_1()
        .flex_basis(relative(0.))
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(3.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(RADIUS_SURFACE))
        .shadow(vec![t.inset_ring(t.line)])
        .hover(move |style| style.bg(panel))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .font_weight(FontWeight::MEDIUM)
                .child(icon(icon_path, t.muted))
                .child(div().min_w_0().child(title)),
        )
        .child(
            small(Text::Secondary)
                .w_full()
                .text_color(t.muted)
                .child(detail),
        )
}
