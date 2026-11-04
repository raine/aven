//! Animated switch tracks shared by settings and compact controls.
use crate::kit::px;
use crate::theme::{Theme, mix};
use gpui::{Animation, AnimationExt, AnyElement, App, Div, Stateful, div, ease_in_out, prelude::*};
use std::time::{Duration, Instant};

const SWITCH_DURATION: Duration = Duration::from_millis(160);

#[derive(Default)]
pub(crate) struct Switch {
    switched: Option<(&'static str, usize, Instant)>,
}

impl Switch {
    /// Starts the slide of the switch named `name`.
    pub(crate) fn mark_switched(&mut self, name: &'static str) {
        let run = self.switched.map_or(0, |(_, run, _)| run + 1);
        self.switched = Some((name, run, Instant::now()));
    }

    /// Draws a switch on `track`, sliding while `name` was just toggled. `scale`
    /// shrinks it for tight rows.
    pub(crate) fn switch_track(
        &self,
        track: Stateful<Div>,
        name: &'static str,
        on: bool,
        scale: f32,
        cx: &App,
        t: &Theme,
    ) -> AnyElement {
        let (off_color, on_color) = (t.line2, t.fill);
        let travel = move |delta: f32| if on { delta } else { 1. - delta };
        let thumb_left = move |delta: f32| (2. + 14. * travel(delta)) * scale;
        let track_color = move |delta: f32| mix(off_color, on_color, travel(delta));
        let moving = self
            .switched
            .filter(|(toggled, _, at)| {
                *toggled == name && at.elapsed() < SWITCH_DURATION && !cx.reduce_motion()
            })
            .map(|(_, run, _)| run);
        let thumb = div()
            .absolute()
            .top(px(2. * scale))
            .size(px(16. * scale))
            .rounded_full()
            .bg(gpui::white());
        let track = track
            .flex_none()
            .relative()
            .w(px(34. * scale))
            .h(px(20. * scale))
            .rounded_full();
        let Some(run) = moving else {
            return track
                .bg(track_color(1.))
                .child(thumb.left(px(thumb_left(1.))))
                .into_any_element();
        };
        let animation = Animation::new(SWITCH_DURATION).with_easing(ease_in_out);
        track
            .child(thumb.with_animation(
                ("switch-thumb", run),
                animation.clone(),
                move |thumb, delta| thumb.left(px(thumb_left(delta))),
            ))
            .with_animation(("switch-track", run), animation, move |track, delta| {
                track.bg(track_color(delta))
            })
            .into_any_element()
    }
}
