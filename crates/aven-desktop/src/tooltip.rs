//! Tooltips anchored to the control they explain.
//!
//! GPUI places its own tooltips at the cursor and times each element on its
//! own, so a tooltip here is drawn by the window's root instead: a control
//! reports its bounds and hover state, and [`layer`] draws the tooltip under
//! it, or beside it in the sidebar. Once a tooltip has shown, moving straight
//! to another control shows the next one without waiting.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Div, ElementId, MouseMoveEvent, Pixels,
    Point, SharedString, Stateful, Task, TextRun, Window, canvas, div, font, prelude::*,
};

use aven_core::choices::{TaskPriority, TaskStatus};

use crate::kit::{
    RADIUS_CHIP, RADIUS_CONTROL, Text, TextScale, ease_out, priority_name, px, small, status_name,
};
use crate::theme::Theme;

/// How long the pointer rests on a control before its tooltip shows.
const SHOW_DELAY: Duration = Duration::from_millis(400);
/// How soon after one tooltip hides the next shows without the delay.
const WARM: Duration = Duration::from_millis(800);
const RISE: Duration = Duration::from_millis(120);
/// The widest a tooltip grows before its lines wrap.
const MAX_WIDTH: f32 = 340.;
/// The space a tooltip may take beside its control, wider than any tooltip.
const ROOM: f32 = 380.;

/// What a control says while the pointer rests on it.
#[derive(Clone)]
pub struct Hint {
    label: SharedString,
    keys: Option<SharedString>,
    sub: Option<SharedString>,
    sub_keys: Option<SharedString>,
    beside: bool,
    clipped_at: Option<Text>,
}

impl Hint {
    pub fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self {
        if condition { then(self) } else { self }
    }

    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            keys: None,
            sub: None,
            sub_keys: None,
            beside: false,
            clipped_at: None,
        }
    }

    /// The shortcut, as typed: chord steps separated by spaces, like `g d`.
    pub fn keys(mut self, keys: impl Into<SharedString>) -> Self {
        self.keys = Some(keys.into());
        self
    }

    /// A fainter second line.
    pub fn sub(mut self, sub: impl Into<SharedString>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    /// The shortcut for the second line's action.
    pub fn sub_keys(mut self, keys: impl Into<SharedString>) -> Self {
        self.sub_keys = Some(keys.into());
        self
    }

    /// Opens to the right of the control instead of below it.
    pub fn beside(mut self) -> Self {
        self.beside = true;
        self
    }

    /// Shows only when the label, drawn at `step`, is cut off by the control.
    pub fn when_clipped(mut self, step: Text) -> Self {
        self.clipped_at = Some(step);
        self
    }

    fn clipped(&self, width: Pixels, window: &Window) -> bool {
        let Some(step) = self.clipped_at else {
            return true;
        };
        let run = TextRun {
            len: self.label.len(),
            font: font(".SystemUIFont"),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line =
            window
                .text_system()
                .shape_line(self.label.clone(), px(step.value()), &[run], None);
        line.width > width + px(0.5)
    }
}

/// A status glyph that marks its task done when clicked.
pub fn status_hint(status: TaskStatus, selected: bool) -> Hint {
    let hint = Hint::new(status_name(status));
    if status.is_terminal() {
        hint.sub("Click to reopen")
    } else {
        hint.sub("Click to mark done")
            .when(selected, |hint| hint.keys("d"))
    }
}

pub fn priority_hint(priority: TaskPriority, selected: bool) -> Hint {
    let label = match priority {
        TaskPriority::None => "No priority".to_string(),
        priority => format!("{} priority", priority_name(priority)),
    };
    Hint::new(label).when(selected, |hint| hint.keys("!"))
}

pub fn epic_hint(closed: bool) -> Hint {
    Hint::new(if closed { "Closed epic" } else { "Epic" })
}

pub trait Hinted: Sized {
    /// Gives the element a tooltip.
    fn hint(self, hint: Hint) -> Self;
}

impl Hinted for Stateful<Div> {
    fn hint(self, hint: Hint) -> Self {
        let bounds = Rc::new(Cell::new(Bounds::default()));
        let probe = bounds.clone();
        let mut this = self;
        let key = (this.interactivity().element_id.clone(), hint.label.clone());
        this.relative()
            .child(
                canvas(move |at, _, _| probe.set(at), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .on_hover(move |hovered, window, cx| {
                if *hovered {
                    enter(&key, &hint, bounds.get(), window, cx);
                } else {
                    leave(&key, window);
                }
            })
    }
}

/// Which control a tooltip belongs to: its element id and label.
type Key = (Option<ElementId>, SharedString);

#[derive(Default)]
struct Tips {
    target: Option<(Key, Hint, Bounds<Pixels>)>,
    shown: bool,
    rise: bool,
    run: usize,
    hidden_at: Option<Instant>,
    /// The control just clicked, which stays quiet until the pointer leaves.
    quiet: Option<Key>,
    timer: Option<Task<()>>,
}

impl Tips {
    fn hide(&mut self) {
        if self.shown {
            self.hidden_at = Some(Instant::now());
        }
        self.target = None;
        self.shown = false;
        self.timer = None;
    }
}

thread_local! {
    static TIPS: RefCell<Tips> = RefCell::default();
}

fn enter(key: &Key, hint: &Hint, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    if bounds.size.width <= px(0.) || !hint.clipped(bounds.size.width, window) {
        return;
    }
    let warm = TIPS.with_borrow_mut(|tips| {
        if tips.quiet.as_ref() == Some(key) {
            return None;
        }
        let warm = tips.shown || tips.hidden_at.is_some_and(|at| at.elapsed() < WARM);
        tips.quiet = None;
        tips.run += 1;
        tips.target = Some((key.clone(), hint.clone(), bounds));
        tips.shown = warm;
        tips.rise = !warm;
        tips.timer = None;
        Some((warm, tips.run))
    });
    let Some((warm, run)) = warm else {
        return;
    };
    if warm {
        window.refresh();
        return;
    }
    let timer = window.spawn(cx, async move |cx| {
        cx.background_executor().timer(SHOW_DELAY).await;
        cx.update(|window, _| {
            let due = TIPS.with_borrow_mut(|tips| {
                let due = tips.run == run && tips.target.is_some();
                tips.shown |= due;
                due
            });
            if due {
                window.refresh();
            }
        })
        .ok();
    });
    TIPS.with_borrow_mut(|tips| {
        if tips.run == run {
            tips.timer = Some(timer);
        }
    });
}

fn leave(key: &Key, window: &mut Window) {
    let changed = TIPS.with_borrow_mut(|tips| {
        if tips.quiet.as_ref() == Some(key) {
            tips.quiet = None;
        }
        let ours = tips.target.as_ref().is_some_and(|(at, _, _)| at == key);
        let shown = tips.shown;
        if ours {
            tips.hide();
        }
        ours && shown
    });
    if changed {
        window.refresh();
    }
}

/// Hides the tooltip once the pointer has left its control, for controls that
/// moved or went away without reporting it.
fn track(position: Point<Pixels>, window: &mut Window) {
    let changed = TIPS.with_borrow_mut(|tips| {
        let left = tips
            .target
            .as_ref()
            .is_some_and(|(_, _, at)| !at.contains(&position));
        let shown = tips.shown;
        if left {
            tips.hide();
        }
        left && shown
    });
    if changed {
        window.refresh();
    }
}

/// Hides the tooltip for a click, a key press or a scroll. A clicked control
/// stays quiet until the pointer leaves it, and the next tooltip waits again.
fn dismiss(window: &mut Window) {
    let changed = TIPS.with_borrow_mut(|tips| {
        let shown = tips.shown;
        tips.quiet = tips.target.as_ref().map(|(key, _, _)| key.clone());
        tips.hide();
        tips.hidden_at = None;
        shown
    });
    if changed {
        window.refresh();
    }
}

/// Lets a window's root draw tooltips over everything else in it.
pub fn host(root: Div, window: &Window, cx: &App, t: &Theme) -> Div {
    root.on_mouse_move(|event: &MouseMoveEvent, window, _| track(event.position, window))
        .capture_any_mouse_down(|_, window, _| dismiss(window))
        .capture_key_down(|_, window, _| dismiss(window))
        .on_scroll(|_, window, _| dismiss(window))
        .children(layer(window, cx, t))
}

/// The tooltip showing now, placed against its control.
fn layer(window: &Window, cx: &App, t: &Theme) -> Option<AnyElement> {
    let (hint, bounds, rise, run) = TIPS.with_borrow(|tips| {
        let (_, hint, bounds) = tips.target.clone().filter(|_| tips.shown)?;
        Some((hint, bounds, tips.rise && !cx.reduce_motion(), tips.run))
    })?;
    let viewport = window.viewport_size();
    let (gap, edge, room) = (px(6.), px(8.), px(ROOM));
    let frame = div().absolute().flex();
    let (frame, side) = if hint.beside {
        let frame = frame
            .left(bounds.right() + px(8.))
            .top(bounds.center().y - room / 2.)
            .w(room)
            .h(room)
            .items_center();
        (frame, Side::Right)
    } else {
        let above = bounds.bottom() + gap + px(48.) > viewport.height;
        let frame = if above {
            frame.top(bounds.top() - gap - room).items_end()
        } else {
            frame.top(bounds.bottom() + gap).items_start()
        }
        .h(room)
        .w(room);
        let center = bounds.center().x;
        let frame = if bounds.size.width > px(220.) || center - room / 2. < edge {
            frame.left(bounds.left().max(edge)).justify_start()
        } else if center + room / 2. > viewport.width - edge {
            frame
                .left(bounds.right().min(viewport.width - edge) - room)
                .justify_end()
        } else {
            frame.left(center - room / 2.).justify_center()
        };
        (frame, if above { Side::Above } else { Side::Below })
    };
    let body = card(&hint, t);
    let body = if rise {
        body.with_animation(
            ("tooltip", run),
            Animation::new(RISE).with_easing(ease_out),
            move |body, delta| {
                let lag = px(3. * (1. - delta));
                let body = body.opacity(delta);
                match side {
                    Side::Below => body.top(-lag),
                    Side::Above => body.top(lag),
                    Side::Right => body.left(-lag),
                }
            },
        )
        .into_any_element()
    } else {
        body.into_any_element()
    };
    Some(frame.child(body).into_any_element())
}

#[derive(Clone, Copy)]
enum Side {
    Below,
    Above,
    Right,
}

fn card(hint: &Hint, t: &Theme) -> Div {
    let line = |text: SharedString, keys: Option<SharedString>| {
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .child(div().min_w_0().child(text))
            .when_some(keys, |line, keys| line.child(caps(&keys, t)))
    };
    small(Text::Secondary)
        .relative()
        .flex_none()
        .max_w(px(MAX_WIDTH))
        .px(px(8.))
        .py(px(4.))
        .rounded(px(RADIUS_CONTROL))
        .bg(t.pop)
        .shadow(t.menu_shadow())
        .text_color(t.body)
        .child(line(hint.label.clone(), hint.keys.clone()))
        .when_some(hint.sub.clone(), |card, sub| {
            card.child(line(sub, hint.sub_keys.clone()).text_color(t.faint))
        })
}

/// A shortcut drawn as keycaps, one per chord step.
fn caps(keys: &str, t: &Theme) -> Div {
    div()
        .ml_auto()
        .flex_none()
        .flex()
        .gap(px(3.))
        .children(keys.split(' ').map(|key| {
            div()
                .min_w(px(16.))
                .h(px(16.))
                .px(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_CHIP))
                .bg(t.raised)
                .shadow(vec![t.inset_ring(t.line2)])
                .text(Text::Caption)
                .line_height(px(16.))
                .text_color(t.muted)
                .child(SharedString::from(key.to_string()))
        }))
}
