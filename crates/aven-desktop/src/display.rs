//! The Display popover, which picks the optional fields task rows show.
use std::time::Duration;

use accesskit::{Role, Toggled};
use gpui::{
    Animation, AnimationExt as _, Bounds, Context, FocusHandle, FontWeight, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Window, div, prelude::*,
};

use crate::adapter::RowFields;
use crate::kit::*;
use crate::shell::Root;
use crate::theme::theme;

const POP_WIDTH: f32 = 252.;
const POP_ENTER: Duration = Duration::from_millis(140);

/// The fields a row can show, each with what it tells.
const FIELDS: [(&str, &str); 3] = [
    ("Labels", "Up to two per task"),
    ("Created date", "When the task was added"),
    ("Updated date", "When anything last changed"),
];

fn shown(fields: &mut RowFields, ix: usize) -> &mut bool {
    match ix {
        0 => &mut fields.labels,
        1 => &mut fields.created,
        _ => &mut fields.updated,
    }
}

pub(crate) struct DisplayPop {
    index: usize,
    focus: FocusHandle,
    prior: Option<FocusHandle>,
    anchor: Bounds<Pixels>,
    run: usize,
}

impl Root {
    pub(crate) fn display_open(&self) -> bool {
        self.display.is_some()
    }

    pub(crate) fn toggle_display(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.display_open() {
            return self.close_display(window, cx);
        }
        let focus = cx.focus_handle();
        let prior = window.focused(cx);
        window.focus(&focus, cx);
        self.display = Some(DisplayPop {
            index: 0,
            prior,
            focus,
            anchor: self.anchor("display"),
            run: self.session.motion.next_run(),
        });
        cx.notify();
    }

    pub(crate) fn close_display(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pop) = self.display.take() {
            let focus = pop.prior.unwrap_or_else(|| self.focus.clone());
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn flip_field(&mut self, ix: usize, cx: &mut Context<Self>) {
        let on = shown(&mut self.prefs.row_fields, ix);
        *on = !*on;
        self.mark_switched(FIELDS[ix].0);
        self.save_prefs();
        cx.notify();
    }

    fn set_display_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(pop) = self.display.as_mut() {
            pop.index = ix % FIELDS.len();
            cx.notify();
        }
    }

    fn on_display_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.display.as_ref().map(|pop| pop.index) else {
            return;
        };
        let stroke = &event.keystroke;
        if stroke.modifiers.platform || stroke.modifiers.control || stroke.modifiers.alt {
            return;
        }
        let last = FIELDS.len() - 1;
        match stroke.key.as_str() {
            "escape" => self.close_display(window, cx),
            "space" | "enter" => self.flip_field(index, cx),
            "down" | "j" => self.set_display_index(index + 1, cx),
            "up" | "k" => self.set_display_index(index + last, cx),
            "tab" if stroke.modifiers.shift => self.set_display_index(index + last, cx),
            "tab" => self.set_display_index(index + 1, cx),
            _ => {}
        }
        cx.stop_propagation();
    }

    pub(crate) fn render_display(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let pop = self.display.as_ref()?;
        let t = theme(cx);
        let mut fields = self.prefs.row_fields;
        let viewport = window.viewport_size();
        let right = (viewport.width - pop.anchor.right()).max(px(12.));
        let top = pop.anchor.bottom() + px(6.);
        let rows = FIELDS.iter().enumerate().map(|(ix, (label, hint))| {
            let on = *shown(&mut fields, ix);
            let row = div()
                .id(("display-field", ix))
                .role(Role::Switch)
                .aria_label(*label)
                .aria_description(*hint)
                .aria_toggled(if on { Toggled::True } else { Toggled::False })
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(6.))
                .text_color(t.ink)
                .when(ix == pop.index, |row| row.bg(t.pick()))
                .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                    if this.display.as_ref().is_some_and(|pop| pop.index != ix) {
                        this.set_display_index(ix, cx);
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_display_index(ix, cx);
                    this.flip_field(ix, cx);
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(*label)
                        .child(small(11.).mt(px(1.)).text_color(t.faint).child(*hint)),
                );
            let knob = div().id(("display-switch", ix));
            row.child(self.switch_track(knob, label, on, 0.85, cx, &t))
        });
        let reduced = cx.reduce_motion();
        let panel = div()
            .id("display")
            .role(Role::Dialog)
            .aria_label("Display")
            .track_focus(&pop.focus)
            .on_key_down(cx.listener(Self::on_display_key))
            .absolute()
            .right(right)
            .top(top)
            .w(px(POP_WIDTH))
            .p(px(5.))
            .rounded(px(10.))
            .bg(t.pop)
            .shadow(t.menu_shadow())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.faint)
                    .child("Show in rows"),
            )
            .children(rows);
        let panel = if reduced {
            panel.into_any_element()
        } else {
            panel
                .with_animation(
                    ("display-enter", pop.run),
                    Animation::new(POP_ENTER),
                    move |panel, delta| {
                        let eased = 1. - (1. - delta).powi(3);
                        panel.top(top - px(2. * (1. - eased))).opacity(eased)
                    },
                )
                .into_any_element()
        };
        Some(
            div()
                .id("display-layer")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_display(window, cx);
                    }),
                )
                .child(panel),
        )
    }
}
