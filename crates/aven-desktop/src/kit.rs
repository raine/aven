//! Small elements shared across the app's surfaces.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use accesskit::Role;
use aven_core::choices::{TaskPriority, TaskStatus};
use gpui::{
    Animation, AnimationExt, AnyView, App, Bounds, BoxShadow, Context, Div, ElementId, Hsla,
    IntoElement, MouseButton, MouseDownEvent, Pixels, SharedString, Stateful, Svg, Transformation,
    Window, canvas, div, linear_color_stop, linear_gradient, point, prelude::*, size, svg,
};

use crate::session::POP_DURATION;
use crate::theme::{Theme, theme};

/// The text size setting, as thousandths. Drawing goes through [`px`], so the
/// whole window scales with it.
static ZOOM: AtomicU32 = AtomicU32::new(1000);
static COMPACT: AtomicBool = AtomicBool::new(false);

pub fn set_scale(zoom: f32, compact: bool) {
    ZOOM.store((zoom * 1000.).round() as u32, Ordering::Relaxed);
    COMPACT.store(compact, Ordering::Relaxed);
}

pub fn compact() -> bool {
    COMPACT.load(Ordering::Relaxed)
}

/// Logical pixels at the current text size.
pub fn px(value: f32) -> Pixels {
    gpui::px(value * ZOOM.load(Ordering::Relaxed) as f32 / 1000.)
}

pub const MONO: &str = "JetBrains Mono";
pub const UI_LINE_HEIGHT: f32 = 18.2;

pub const STATUSES: [(TaskStatus, &str); 6] = [
    (TaskStatus::Active, "Active"),
    (TaskStatus::Todo, "Todo"),
    (TaskStatus::Inbox, "Inbox"),
    (TaskStatus::Backlog, "Backlog"),
    (TaskStatus::Done, "Done"),
    (TaskStatus::Canceled, "Canceled"),
];

pub const PRIORITIES: [(TaskPriority, &str); 5] = [
    (TaskPriority::Urgent, "Urgent"),
    (TaskPriority::High, "High"),
    (TaskPriority::Medium, "Medium"),
    (TaskPriority::Low, "Low"),
    (TaskPriority::None, "None"),
];

/// The letter that picks a status once the status menu is open.
pub fn status_key(status: TaskStatus) -> char {
    match status {
        TaskStatus::Inbox => 'i',
        TaskStatus::Backlog => 'b',
        TaskStatus::Todo => 't',
        TaskStatus::Active => 'a',
        TaskStatus::Done => 'd',
        TaskStatus::Canceled => 'x',
    }
}

/// The letter that picks a priority once the priority menu is open.
pub fn priority_key(priority: TaskPriority) -> char {
    match priority {
        TaskPriority::None => 'n',
        TaskPriority::Low => 'l',
        TaskPriority::Medium => 'm',
        TaskPriority::High => 'h',
        TaskPriority::Urgent => 'u',
    }
}

pub fn status_name(status: TaskStatus) -> &'static str {
    STATUSES
        .iter()
        .find(|(value, _)| *value == status)
        .map_or("", |(_, name)| name)
}

pub fn priority_name(priority: TaskPriority) -> &'static str {
    PRIORITIES
        .iter()
        .find(|(value, _)| *value == priority)
        .map_or("", |(_, name)| name)
}

pub type Anchors = Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>;

/// A cubic ease-out over `delta` from 0 to 1.
pub fn ease_out(delta: f32) -> f32 {
    1. - (1. - delta).powi(3)
}

pub fn drag_window(event: &MouseDownEvent, window: &mut Window, _: &mut App) {
    if event.click_count == 2 {
        window.titlebar_double_click();
    } else {
        window.start_window_move();
    }
}

pub fn spread(color: Hsla, radius: f32) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(radius),
        inset: false,
    }
}

pub fn small(size: f32) -> Div {
    div().text_size(px(size)).line_height(px(size * 1.4))
}

pub fn icon(path: &'static str, color: Hsla) -> Svg {
    svg().path(path).size(px(16.)).flex_none().text_color(color)
}

pub fn project_dot(key: &str, t: &Theme) -> Div {
    div()
        .flex_none()
        .mx(px(4.))
        .size(px(8.))
        .rounded(px(3.))
        .bg(t.project_dot(key))
}

pub fn nav_item(id: impl Into<ElementId>, t: &Theme) -> Stateful<Div> {
    let raised = t.raised;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(9.))
        .w_full()
        .h(px(if compact() { 24. } else { 27. }))
        .mb(px(2.))
        .px(px(8.))
        .rounded(px(6.))
        .text_color(t.body)
        .hover(move |style| style.bg(raised))
}

pub fn nav_count(text: impl Into<SharedString>, hot: bool, t: &Theme) -> impl IntoElement {
    small(12.)
        .ml_auto()
        .font_weight(if hot {
            gpui::FontWeight::SEMIBOLD
        } else {
            gpui::FontWeight::NORMAL
        })
        .text_color(if hot { t.bad } else { t.faint })
        .child(text.into())
}

pub fn btn(id: impl Into<ElementId>, label: impl Into<SharedString>, t: &Theme) -> Stateful<Div> {
    let raised = t.raised;
    bare_btn(id, label, t).hover(move |style| style.bg(raised))
}

/// An outlined button that names itself.
pub fn line_btn(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    t: &Theme,
) -> Stateful<Div> {
    let label = label.into();
    btn(id, label.clone(), t)
        .shadow(vec![t.inset_ring(t.line2)])
        .child(label)
}

/// The filled primary button that names itself.
pub fn fill_btn(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    t: &Theme,
) -> Stateful<Div> {
    let label = label.into();
    let (fill_top, fill, fill_h) = (t.fill_top, t.fill, t.fill_h);
    bare_btn(id, label.clone(), t)
        .bg(linear_gradient(
            180.,
            linear_color_stop(fill_top, 0.),
            linear_color_stop(fill, 1.),
        ))
        .text_color(t.on_fill)
        .font_weight(gpui::FontWeight::MEDIUM)
        .hover(move |style| style.bg(fill_h))
        .child(label)
}

/// A key shown inside a button.
pub fn kbd(text: &'static str, color: Hsla) -> impl IntoElement + use<> {
    small(12.)
        .text_color(color)
        .font_weight(gpui::FontWeight::NORMAL)
        .child(text)
}

/// A button that sets its own hover colour.
pub fn bare_btn(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.into())
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(10.))
        .rounded(px(6.))
        .text_color(t.body)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// One line shown while the pointer rests on an element.
pub struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        small(12.)
            .font_family(".SystemUIFont")
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(t.pop)
            .shadow(t.menu_shadow())
            .text_color(t.body)
            .child(self.0.clone())
    }
}

/// Builds a [`Tip`] for an element's `tooltip`.
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text = text.into();
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}

/// Reports where a control is, so a menu can open from it.
pub fn anchor_probe(key: &'static str, anchors: &Anchors) -> impl IntoElement + use<> {
    let anchors = anchors.clone();
    canvas(
        move |bounds, _, _| {
            anchors.borrow_mut().insert(key, bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

fn status_look(status: TaskStatus, t: &Theme) -> (&'static str, Hsla) {
    match status {
        TaskStatus::Active => ("icons/st-active.svg", t.st_active),
        TaskStatus::Todo => ("icons/st-ring.svg", t.st_todo),
        TaskStatus::Inbox => ("icons/st-inbox.svg", t.faint),
        TaskStatus::Backlog => ("icons/st-ring.svg", t.faint),
        TaskStatus::Done => ("icons/st-done.svg", t.good),
        TaskStatus::Canceled => ("icons/st-canceled.svg", t.faint),
    }
}

pub fn status_glyph(status: TaskStatus, t: &Theme) -> impl IntoElement + use<> {
    status_glyph_sized(status, 14., t)
}

pub fn status_glyph_sized(status: TaskStatus, size: f32, t: &Theme) -> impl IntoElement + use<> {
    let (path, color) = status_look(status, t);
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// The colour a status's mark and name read in.
pub fn status_color(status: TaskStatus, t: &Theme) -> Hsla {
    status_look(status, t).1
}

/// The star after an epic's title or ref, faint once the epic is closed.
pub fn epic_star(closed: bool, t: &Theme) -> Svg {
    svg()
        .path("icons/epic-star.svg")
        .size(px(12.))
        .flex_none()
        .text_color(if closed { t.faint } else { t.epic })
}

/// The old mark fades while the done mark scales up past its size and settles.
pub fn pop_glyph(previous: TaskStatus, run: usize, t: &Theme) -> impl IntoElement + use<> {
    let (old_path, old_color) = status_look(previous, t);
    let (path, color) = status_look(TaskStatus::Done, t);
    let layer = move |path: &'static str, color: Hsla| {
        svg()
            .absolute()
            .top_0()
            .left_0()
            .size(px(14.))
            .path(path)
            .text_color(color)
    };
    let animation = Animation::new(POP_DURATION);
    div()
        .relative()
        .flex_none()
        .size(px(14.))
        .child(layer(old_path, old_color).with_animation(
            ("pop-old", run),
            animation.clone(),
            move |old, delta| old.text_color(old_color.opacity(1. - (delta / 0.4).min(1.))),
        ))
        .child(layer(path, color).with_animation(
            ("pop-done", run),
            animation,
            move |done, delta| {
                let bent = delta - 1.;
                let eased = 1. + 3.7 * bent.powi(3) + 2.7 * bent.powi(2);
                let scale = 0.4 + 0.6 * eased;
                done.text_color(color.opacity((delta / 0.3).min(1.)))
                    .with_transformation(Transformation::scale(size(scale, scale)))
            },
        ))
}

pub fn priority_color(priority: TaskPriority, t: &Theme) -> Hsla {
    match priority {
        TaskPriority::None => t.faint,
        TaskPriority::High => t.pr_high,
        TaskPriority::Urgent => t.pr_urgent,
        TaskPriority::Low | TaskPriority::Medium => t.pr_low,
    }
}

pub fn priority_icon(priority: TaskPriority, t: &Theme) -> impl IntoElement + use<> {
    let color = priority_color(priority, t);
    let (on, off): (&'static str, Option<&'static str>) = match priority {
        TaskPriority::None => ("icons/pri-0.svg", None),
        TaskPriority::Low => ("icons/pri-1-on.svg", Some("icons/pri-1-off.svg")),
        TaskPriority::Medium => ("icons/pri-2-on.svg", Some("icons/pri-2-off.svg")),
        TaskPriority::High => ("icons/pri-3-on.svg", None),
        TaskPriority::Urgent => ("icons/pri-4.svg", None),
    };
    div()
        .relative()
        .flex_none()
        .size(px(16.))
        .child(svg().absolute().path(on).size(px(16.)).text_color(color))
        .when_some(off, |el, off| {
            el.child(
                svg()
                    .absolute()
                    .path(off)
                    .size(px(16.))
                    .text_color(color.opacity(0.3)),
            )
        })
}

pub fn ref_label(
    reference: &str,
    project_key: &str,
    width: Option<f32>,
    t: &Theme,
) -> impl IntoElement + use<> {
    let (prefix, rest) = reference.split_at(reference.find('-').unwrap_or(reference.len()));
    div()
        .flex()
        .flex_none()
        .when_some(width, |el, width| el.w(px(width)))
        .overflow_hidden()
        .font_family(MONO)
        .text_size(px(12.))
        .text_color(t.faint)
        .child(
            div()
                .text_color(t.project(project_key))
                .child(prefix.to_string()),
        )
        .child(rest.to_string())
}

#[cfg(test)]
mod key_tests {
    use std::collections::HashSet;

    use super::{PRIORITIES, STATUSES, priority_key, status_key};

    #[test]
    fn each_choice_has_its_own_letter() {
        let statuses: HashSet<_> = STATUSES.iter().map(|(one, _)| status_key(*one)).collect();
        assert_eq!(statuses.len(), STATUSES.len());
        let priorities: HashSet<_> = PRIORITIES
            .iter()
            .map(|(one, _)| priority_key(*one))
            .collect();
        assert_eq!(priorities.len(), PRIORITIES.len());
    }
}
