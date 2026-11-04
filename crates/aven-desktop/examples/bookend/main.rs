//! Bookend, the first-launch intro, played over a static welcome screen with
//! controls for stepping through it. The intro itself is `aven_desktop::intro`.
//!
//! just bookend (a bare `cargo run` window cannot take keyboard focus)
//!
//! R replays, space pauses, ←/→ step a frame (shift: 100 ms), enter jumps to
//! the end, S cycles the speed, T switches the theme, Q quits.
//! BOOKEND_T=1100 opens paused on that frame; BOOKEND_LIGHT=1 opens in the
//! light theme.
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use aven_desktop::intro::{self, Colors, END, LANDED};
use aven_desktop::theme::{Theme, mix};
use gpui::{
    App, Bounds, Context, FocusHandle, Keystroke, Pixels, Render, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowOptions, canvas, div, point, prelude::*, px, size,
};
use gpui_platform::application;

const SPEEDS: [f32; 4] = [1., 0.5, 0.25, 0.1];

struct Clock {
    anchor: Instant,
    base: f32,
    speed: f32,
    paused: bool,
}

impl Clock {
    fn new() -> Self {
        Self {
            anchor: Instant::now(),
            base: 0.,
            speed: 1.,
            paused: false,
        }
    }
    fn now(&self) -> f32 {
        if self.paused {
            self.base
        } else {
            (self.base + self.anchor.elapsed().as_secs_f32() * 1000. * self.speed).min(END)
        }
    }
    fn seek(&mut self, t: f32) {
        self.base = t.clamp(0., END);
        self.anchor = Instant::now();
    }
}

struct Bookend {
    clock: Clock,
    dark: bool,
    focus: FocusHandle,
    slot: Rc<Cell<Option<Bounds<Pixels>>>>,
    _keys: Subscription,
}

impl Bookend {
    fn key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let step = if keystroke.modifiers.shift {
            100.
        } else {
            1000. / 60.
        };
        let now = self.clock.now();
        match keystroke.key.as_str() {
            "r" => self.clock.seek(0.),
            "enter" => self.clock.seek(END),
            "space" => {
                self.clock.paused = !self.clock.paused;
                self.clock.seek(now);
            }
            "left" | "right" => {
                self.clock.paused = true;
                let sign = if keystroke.key == "left" { -1. } else { 1. };
                self.clock.seek(now + sign * step);
            }
            "s" => {
                let index = SPEEDS
                    .iter()
                    .position(|&s| s == self.clock.speed)
                    .unwrap_or(0);
                self.clock.speed = SPEEDS[(index + 1) % SPEEDS.len()];
                self.clock.seek(now);
            }
            "t" => self.dark = !self.dark,
            "q" | "escape" => cx.quit(),
            _ => return,
        }
        cx.notify();
    }
}

fn rise(index: usize, t: f32, child: impl IntoElement) -> impl IntoElement {
    let rise = intro::rise(index, t);
    div()
        .w_full()
        .flex()
        .justify_center()
        .relative()
        .top(px(rise.y))
        .opacity(rise.opacity)
        .child(child)
}

impl Render for Bookend {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.clock.now();
        if t < END && !self.clock.paused {
            window.request_animation_frame();
        }
        let theme = if self.dark {
            Theme::dark()
        } else {
            Theme::light()
        };
        let colors = Colors::new(&theme);
        let landed = t >= LANDED;
        let slot = self.slot.clone();
        let read_slot = self.slot.clone();
        let card = |title: &'static str, detail: &'static str| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .px(px(14.))
                .py(px(12.))
                .rounded(px(10.))
                .border_1()
                .border_color(theme.line)
                .child(div().text_size(px(13.)).text_color(theme.ink).child(title))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted)
                        .child(detail),
                )
        };
        let hud = format!(
            "{:>5.0} ms  ×{}{}   R replay · space pause · ←/→ step · enter end · S speed · T theme",
            t,
            self.clock.speed,
            if self.clock.paused { "  paused" } else { "" },
        );

        div()
            .size_full()
            .relative()
            .bg(theme.page)
            .font_family(".SystemUIFont")
            .track_focus(&self.focus)
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .pb(px(88.))
                    .child(
                        div().size(px(56.)).mb(px(20.)).child(
                            canvas(
                                move |bounds, _, _| slot.set(Some(bounds)),
                                move |bounds, _, window, _| {
                                    if landed {
                                        intro::paint_mark(window, bounds, colors);
                                    }
                                },
                            )
                            .size_full(),
                        ),
                    )
                    .child(rise(
                        0,
                        t,
                        div()
                            .text_size(px(26.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.ink)
                            .child("Welcome to Aven"),
                    ))
                    .child(rise(
                        1,
                        t,
                        div()
                            .w(px(460.))
                            .mt(px(6.))
                            .text_size(px(14.))
                            .text_center()
                            .text_color(theme.muted)
                            .child("Local-first tasks for you and your coding agents. Everything stays on this Mac until you choose to sync."),
                    ))
                    .child(rise(
                        2,
                        t,
                        div()
                            .w(px(460.))
                            .h(px(48.))
                            .mt(px(24.))
                            .pl(px(15.))
                            .pr(px(8.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .rounded(px(10.))
                            .bg(theme.panel)
                            .border_1()
                            .border_color(theme.line)
                            .text_size(px(15.))
                            .text_color(theme.faint)
                            .child("What's the first thing on your mind?")
                            .child(
                                div()
                                    .px(px(10.))
                                    .py(px(6.))
                                    .rounded(px(7.))
                                    .bg(mix(theme.panel, Theme::dark().accent, 0.25))
                                    .text_size(px(13.))
                                    .text_color(theme.muted)
                                    .child("Add ↵"),
                            ),
                    ))
                    .child(rise(
                        3,
                        t,
                        div()
                            .w(px(460.))
                            .mt(px(8.))
                            .flex()
                            .justify_between()
                            .text_size(px(12.))
                            .text_color(theme.faint)
                            .child("Adds to ● Personal  PRS")
                            .child("More details ⌘↵"),
                    ))
                    .child(rise(
                        4,
                        t,
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .mt(px(34.))
                                    .mb(px(12.))
                                    .text_size(px(12.))
                                    .text_color(theme.faint)
                                    .child("Already use Aven?"),
                            )
                            .child(
                                div()
                                    .w(px(460.))
                                    .flex()
                                    .gap(px(8.))
                                    .child(card("Join existing sync", "Use an invitation from a computer that syncs"))
                                    .child(card("Restore a backup", "Bring back tasks from an archive")),
                            ),
                    ))
                    .child(rise(
                        5,
                        t,
                        div()
                            .mt(px(22.))
                            .flex()
                            .gap(px(18.))
                            .text_size(px(12.))
                            .text_color(theme.muted)
                            .child("Try the demo")
                            .child("Skip to an empty queue")
                            .child("Keyboard shortcuts ?"),
                    )),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        if let Some(slot) = read_slot.get() {
                            intro::paint(window, t, bounds, slot, colors);
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(14.))
                    .bottom(px(10.))
                    .text_size(px(11.))
                    .text_color(theme.faint)
                    .child(hud),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Bookend".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.), px(21.))),
                }),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    focus.focus(window, cx);
                    window.activate_window();
                    let mut clock = Clock::new();
                    if cx.reduce_motion() {
                        clock.seek(END);
                    }
                    if let Some(t) = std::env::var("BOOKEND_T").ok().and_then(|t| t.parse().ok()) {
                        clock.paused = true;
                        clock.seek(t);
                    }
                    Bookend {
                        clock,
                        dark: std::env::var_os("BOOKEND_LIGHT").is_none(),
                        focus,
                        slot: Rc::default(),
                        // every keystroke in the window, whatever holds focus
                        _keys: cx.observe_keystrokes(|this: &mut Bookend, event, _, cx| {
                            this.key(&event.keystroke, cx)
                        }),
                    }
                })
            },
        )
        .expect("the window opens");
        cx.activate(true);
    });
}
