//! Rubber band at the ends of a list, as `NSScrollView` does it. The list
//! keeps its clamped position; only its painted content moves past an edge.
//!
//! The gesture handling and physics are adapted from `gpui-base`'s
//! `scroll_bounce` and `overscroll` modules (Apache-2.0).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ContentMask, DispatchPhase, Element, ElementId, GlobalElementId,
    Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, ListState, OngoingScroll,
    Pixels, ScrollDelta, ScrollEvent, TouchPhase, Window, point, px,
};

/// Fraction of scroll travel the stretched edge follows at first.
const TRACKING: f32 = 0.075;
/// How far the edge can go past its end, stretched or thrown.
const MAX_STRETCH: f32 = 36.;
/// Undamped angular frequency of the critically damped return.
const OMEGA: f32 = 12.5;

/// A pause without packets that ends a gesture. Momentum arrives once per
/// frame with no phase of its own, so a pause marks the end of its stream.
const GESTURE_GAP: Duration = Duration::from_millis(250);

/// The fraction of the content's speed the edge carries when the content runs
/// into it or a finger lets go of it.
const CARRY: f32 = 0.31;

/// A finger that sends nothing for this long before lifting has stopped: it
/// lets go of the edge with no speed.
const STILL: Duration = Duration::from_millis(50);

/// The interval assumed for the first packet of a stream.
const FRAME: f32 = 1. / 60.;

/// How far back the stream's speed is read. Packets carry no timestamp, so a
/// slow frame hands over several at once; read over a span, their distance
/// still spreads over the time they took.
const SPEED_SPAN: Duration = Duration::from_millis(50);

/// The most a packet's unused part can be and still be rounding.
const ROUNDING: f32 = 0.01;

/// The largest outward speed, at most `speed`, that lets an edge at `position`
/// go no farther out than `MAX_STRETCH`.
fn limit_throw(position: f32, speed: f32) -> f32 {
    let peak = |speed: f32| {
        if speed <= OMEGA * position {
            position
        } else {
            speed / OMEGA * (OMEGA * position / speed - 1.).exp()
        }
    };
    if peak(speed) <= MAX_STRETCH {
        return speed;
    }
    let (mut low, mut high) = (0., speed);
    for _ in 0..24 {
        let mid = (low + high) / 2.;
        if peak(mid) <= MAX_STRETCH {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

/// The stretch past an edge and its return.
#[derive(Default)]
struct Physics {
    position: f32,
    velocity: f32,
    dragging: bool,
    extent: f32,
}

impl Physics {
    fn is_moving(&self) -> bool {
        !self.dragging && (self.position != 0. || self.velocity != 0.)
    }

    fn offset(&self) -> f32 {
        if self.dragging {
            let extent = self.extent.max(1.);
            let stretch = self.position * TRACKING / (1. + TRACKING * self.position.abs() / extent);
            // Follows the stretch closely at first, then eases into the limit.
            MAX_STRETCH * (stretch / MAX_STRETCH).tanh()
        } else {
            self.position
        }
    }

    fn begin(&mut self, extent: f32) {
        let offset = self.offset();
        // A displaced edge keeps the extent it was stretched under, so the
        // finger lands where the edge is even if the viewport has resized.
        if offset == 0. || self.extent <= 0. {
            self.extent = extent.max(1.);
        }
        // Invert the curves so grabbing a returning edge is continuous.
        let stretch = MAX_STRETCH * (offset / MAX_STRETCH).clamp(-0.999, 0.999).atanh();
        self.position = stretch / (TRACKING * (1. - stretch.abs() / self.extent).max(0.01));
        self.velocity = 0.;
        self.dragging = true;
    }

    /// Applies finger travel and returns the part that crosses back over the
    /// edge into the content.
    fn pull(&mut self, delta: f32) -> f32 {
        let previous = self.position;
        let next = previous + delta;
        if previous != 0. && previous.signum() != next.signum() {
            self.position = 0.;
            next
        } else {
            self.position = next;
            0.
        }
    }

    /// Lets go of the edge: from wherever it is, it returns at full speed and
    /// carries `velocity` outward on the way. Its path is
    /// `(x + velocity * t) * e^(-ωt)`, so it never crosses the boundary.
    fn let_go(&mut self, velocity: f32) {
        self.position = self.offset();
        self.dragging = false;
        let outward = if self.position * velocity >= 0. {
            velocity
        } else {
            0.
        };
        let outward = outward.signum() * limit_throw(self.position.abs(), outward.abs());
        self.velocity = outward - OMEGA * self.position;
    }

    /// Exact critically damped spring integration, independent of refresh rate.
    fn step(&mut self, seconds: f32) -> bool {
        if self.dragging || (self.position == 0. && self.velocity == 0.) {
            return false;
        }
        let decay = (-OMEGA * seconds).exp();
        let c = self.velocity + OMEGA * self.position;
        self.position = (self.position + c * seconds) * decay;
        self.velocity = (self.velocity - OMEGA * c * seconds) * decay;
        if self.position.abs() < 0.1 && self.velocity.abs() < 1. {
            self.position = 0.;
            self.velocity = 0.;
            false
        } else {
            true
        }
    }
}

/// Displacement of a list past its ends, driven by wheel packets.
#[derive(Default)]
pub struct Bounce {
    physics: Physics,
    sampled_at: Option<Instant>,
    last_packet_at: Option<Instant>,
    /// When the stream last moved, and how fast, in pixels per second.
    moved_at: Option<Instant>,
    velocity: f32,
    /// The packets that moved within the last `SPEED_SPAN`, and the one before
    /// them, which marks where the span starts.
    moves: Vec<(Instant, f32)>,
    /// A finger rests on the trackpad but has not moved since it touched.
    resting: bool,
    /// The edge already answered the rest of this gesture. The momentum that
    /// follows still scrolls the content back, but no longer moves the edge.
    spent: bool,
    ongoing_scroll: OngoingScroll,
}

impl Bounce {
    /// How far the content is drawn from where it scrolled to. Positive moves
    /// it down, past the top.
    pub fn offset(&self) -> f32 {
        self.physics.offset()
    }

    fn start_gesture(&mut self) {
        self.spent = false;
        self.velocity = 0.;
        self.moves.clear();
    }

    /// Tracks the stream's speed from packets that moved.
    fn sample(&mut self, dy: f32, now: Instant) {
        if dy == 0. {
            return;
        }
        if self.moves.last().is_some_and(|&(_, last)| last * dy < 0.) {
            self.moves.clear();
        }
        self.moves.push((now, dy));
        let start = self
            .moves
            .iter()
            .rposition(|&(at, _)| now.saturating_duration_since(at) > SPEED_SPAN)
            .unwrap_or(0);
        self.moves.drain(..start);
        let (first, _) = self.moves[0];
        let seconds = now.saturating_duration_since(first).as_secs_f32();
        self.velocity = if seconds > 0. && self.moves.len() > 1 {
            self.moves[1..].iter().map(|&(_, dy)| dy).sum::<f32>() / seconds
        } else {
            // The first packet, or several handed over at once.
            self.moves.iter().map(|&(_, dy)| dy).sum::<f32>() / FRAME
        };
        self.moved_at = Some(now);
    }

    /// Handles one vertical packet of `dy` over a viewport `extent` tall.
    /// `scroll` moves the content and returns how much of `dy` it used.
    /// Returns whether the stretch changed.
    fn packet(
        &mut self,
        dy: f32,
        phase: TouchPhase,
        extent: f32,
        now: Instant,
        mut scroll: impl FnMut(f32) -> f32,
    ) -> bool {
        let before = self.offset();
        let mut thrown = false;
        let gap = self
            .last_packet_at
            .replace(now)
            .map(|at| now.saturating_duration_since(at))
            .filter(|gap| *gap < GESTURE_GAP);
        // A finger resting on the trackpad sends nothing, however long it
        // holds the edge; only its own `Ended` lets go. A pause ends the rest.
        if gap.is_none() && !self.physics.dragging {
            self.start_gesture();
            self.moved_at = None;
        }
        self.sample(dy, now);
        if phase == TouchPhase::Started {
            // Grab the edge, a returning one included, where it is.
            self.physics.begin(extent);
            self.spent = false;
            self.resting = dy == 0.;
        } else if dy != 0. {
            self.resting = false;
        } else if self.resting && self.physics.dragging {
            // A finger that touches and lifts without moving cancels its
            // gesture, which arrives as a move of nothing.
            self.resting = false;
            self.physics.let_go(0.);
            self.spent = self.offset() != 0.;
            if self.spent {
                self.sampled_at = Some(now);
            }
            return self.physics.is_moving();
        }
        if self.physics.dragging {
            // A stretched edge takes the packet first; only what crosses back
            // over the edge scrolls the content.
            let rest = if self.offset() != 0. {
                self.physics.pull(dy)
            } else {
                dy
            };
            if rest != 0. {
                let unused = rest - scroll(rest);
                if unused.abs() > ROUNDING {
                    self.physics.pull(unused);
                }
            }
            if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                let moving = self
                    .moved_at
                    .is_some_and(|at| now.saturating_duration_since(at) < STILL);
                // Only an edge held past its end carries the finger's speed.
                let velocity = if moving && self.offset() != 0. {
                    self.velocity
                } else {
                    0.
                };
                self.physics.let_go(velocity * CARRY);
                thrown = true;
                self.spent = self.offset() != 0.;
            }
        } else if self.spent {
            // Momentum after the edge answered: back into the content it
            // scrolls; against the edge it has nothing left to move.
            scroll(dy);
        } else {
            // Momentum or phaseless precise scrolling: when the content runs
            // into the edge it throws the edge as fast as it arrived, once.
            let unused = dy - scroll(dy);
            if unused.abs() > ROUNDING {
                self.physics.let_go(self.velocity * CARRY);
                thrown = true;
                self.spent = true;
            }
        }
        let changed = (thrown && self.physics.is_moving()) || self.offset() != before;
        // Start the clock when the edge starts moving, and again when it is
        // let go; a packet arriving while it moves keeps the time since the
        // last frame.
        if thrown || (changed && self.sampled_at.is_none()) {
            self.sampled_at = Some(now);
        }
        changed
    }

    /// Advances the return to `now`. Returns whether it is still moving.
    fn step(&mut self, now: Instant) -> bool {
        let elapsed = self
            .sampled_at
            .replace(now)
            .map_or(0., |at| now.saturating_duration_since(at).as_secs_f32());
        let moving = self.physics.step(elapsed);
        if !moving {
            self.sampled_at = None;
        }
        moving
    }

    /// Drops any stretch at once.
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Wraps a list, takes over its vertical wheel input and stretches it at both
/// ends. The list must be measured up front, or it stretches at a false end
/// where measuring has got to.
pub struct ListBounce {
    id: ElementId,
    state: Rc<RefCell<Bounce>>,
    list: ListState,
    child: AnyElement,
}

impl ListBounce {
    pub fn new(
        id: impl Into<ElementId>,
        state: Rc<RefCell<Bounce>>,
        list: ListState,
        child: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            state,
            list,
            child: child.into_any_element(),
        }
    }
}

impl IntoElement for ListBounce {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ListBounce {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        let offset = {
            let mut state = self.state.borrow_mut();
            if cx.reduce_motion() || self.list.max_offset_for_scrollbar().y <= px(0.) {
                state.reset();
            }
            if state.step(cx.background_executor().now()) {
                window.request_animation_frame();
            }
            state.offset()
        };
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.with_element_offset(point(px(0.), px(offset)), |window| {
                self.child.prepaint(window, cx);
            });
        });
        hitbox
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.child.paint(window, cx);
        });
        let hitbox = hitbox.id;
        let state = self.state.clone();
        let list = self.list.clone();
        let height = f32::from(bounds.size.height);
        let view = window.current_view();
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                return;
            }
            let mut delta = event.delta.pixel_delta(window.line_height());
            let mut state = state.borrow_mut();
            state.ongoing_scroll.filter(&mut delta, event.touch_phase);
            let max = list.max_offset_for_scrollbar().y;
            if max <= px(0.) {
                state.reset();
                return;
            }
            if delta.x.abs() > delta.y.abs() {
                return;
            }
            let before = list.scroll_px_offset_for_scrollbar();
            let scroll = |dy: f32| {
                let before = list.scroll_px_offset_for_scrollbar();
                let requested = before.y + px(dy);
                let y = requested.clamp(-max, px(0.));
                list.set_offset_from_scrollbar(point(before.x, y));
                // Only clamping leaves unused input; subtraction can leave a
                // rounding residue inside the list.
                if y == requested {
                    dy
                } else {
                    f32::from(y - before.y)
                }
            };
            let changed = if cx.reduce_motion() || matches!(event.delta, ScrollDelta::Lines(_)) {
                let displaced = state.offset() != 0.;
                state.reset();
                scroll(f32::from(delta.y));
                displaced
            } else {
                let now = cx.background_executor().now();
                state.packet(f32::from(delta.y), event.touch_phase, height, now, scroll)
            };
            let scrolled = list.scroll_px_offset_for_scrollbar() != before;
            drop(state);
            cx.stop_propagation();
            if changed || scrolled {
                cx.notify(view);
            }
        });
    }
}
