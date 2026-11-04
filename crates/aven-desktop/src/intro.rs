//! Bookend, the first-launch intro: the TUI splash at hero size, resolved into
//! the vector mark by its afterglow, which then settles into the welcome
//! screen's header while the content rises in.
//!
//! The screen behind it lays itself out as usual and reports where its logo
//! square sits. Each frame, [`paint`] draws the intro over the whole window
//! until the mark lands in that square; from then on the screen draws the mark
//! itself with [`paint_mark`]. Content items fade and rise by [`rise`].
mod logo;

use std::f32::consts::PI;
use std::time::Instant;

use gpui::{
    App, Background, Bounds, FillOptions, FillRule, Hsla, Keystroke, PathBuilder, PathStyle,
    Pixels, Window, fill, linear_color_stop, linear_gradient, point, px, size,
};

use crate::theme::{Theme, mix};
use logo::{Pixel, Point, clip_convex, ellipse};

// ---------- Timeline, in ms ----------
const PURPLE: (f32, f32) = (0., 400.);
const CHECK: (f32, f32) = (320., 700.);
const GLOW: (f32, f32) = (750., 1450.);
const BUMP: (f32, f32) = (750., 1550.);
const SETTLE: (f32, f32) = (1300., 1820.);
const RISE_AT: f32 = 1620.;
const RISE_MS: f32 = 360.;
const RISE_STAGGER: f32 = 45.;
/// When the mark sits in the logo square and the intro stops painting.
pub const LANDED: f32 = SETTLE.1;
/// The content items that rise in after the mark, in order.
pub const ITEMS: usize = 6;
/// When the last item has risen and the screen is at rest.
pub const END: f32 = RISE_AT + RISE_STAGGER * (ITEMS - 1) as f32 + RISE_MS;
/// The hero mark's height as a share of the window, and its cap in px.
const HERO: f32 = 0.5;
const HERO_MAX: f32 = 380.;

/// When content item `index` starts rising.
pub fn item_start(index: usize) -> f32 {
    RISE_AT + RISE_STAGGER * index as f32
}

/// How far a content item has risen in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rise {
    pub opacity: f32,
    /// Downward offset in px, zero at rest.
    pub y: f32,
}

/// Content item `index` at `t` ms.
pub fn rise(index: usize, t: f32) -> Rise {
    let start = item_start(index);
    let p = ease_out_quint(win(t, (start, start + RISE_MS)));
    Rise {
        opacity: p,
        y: 8. * (1. - p),
    }
}

/// Milliseconds into the intro since `started`, or [`END`] when the system asks
/// for reduced motion, so the screen appears at rest.
pub fn elapsed(started: Instant, cx: &App) -> f32 {
    if cx.reduce_motion() {
        END
    } else {
        (started.elapsed().as_secs_f32() * 1000.).min(END)
    }
}

/// Where a screen is with its intro, so it plays once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Playback {
    #[default]
    Unplayed,
    Playing(Instant),
    Played,
}

impl Playback {
    /// Starts the intro at `now` unless it has played. Returns whether it did.
    pub fn start(&mut self, now: Instant) -> bool {
        if *self != Self::Unplayed {
            return false;
        }
        *self = Self::Playing(now);
        true
    }

    /// Jumps a playing intro to its end. Returns whether one was playing.
    pub fn skip(&mut self) -> bool {
        if !matches!(self, Self::Playing(_)) {
            return false;
        }
        *self = Self::Played;
        true
    }

    /// Skips a playing intro on `keystroke`. Returns whether the key should go
    /// no further: platform shortcuts, such as quit, skip and still run.
    pub fn skip_key(&mut self, keystroke: &Keystroke) -> bool {
        self.skip() && !keystroke.modifiers.platform
    }

    /// The time to draw the intro at, from `clock` given its start, or `None`
    /// when it is not playing. Reaching [`END`] finishes it.
    pub fn frame(&mut self, clock: impl FnOnce(Instant) -> f32) -> Option<f32> {
        let Self::Playing(started) = *self else {
            return None;
        };
        let t = clock(started);
        if t < END {
            return Some(t);
        }
        *self = Self::Played;
        None
    }
}

// ---------- Easing ----------
fn win(t: f32, (a, b): (f32, f32)) -> f32 {
    ((t - a) / (b - a)).clamp(0., 1.)
}
fn smooth(p: f32) -> f32 {
    p * p * (3. - 2. * p)
}
fn ease_in_out(p: f32) -> f32 {
    if p < 0.5 {
        2. * p * p
    } else {
        1. - (-2. * p + 2.).powi(2) / 2.
    }
}
/// Close to cubic-bezier(.23, 1, .32, 1), the strong ease-out the UI uses.
fn ease_out_quint(p: f32) -> f32 {
    1. - (1. - p).powi(5)
}

// ---------- Colour ----------
/// The desktop mark's gradient, top to bottom, from its SVG. GPUI gradients
/// take two stops, so the cells use the same two.
const MARK_TOP: u32 = 0xbe85fb;
const MARK_BOTTOM: u32 = 0x7628ff;

fn hex(value: u32) -> Hsla {
    gpui::rgb(value).into()
}

/// The theme colours the intro paints with.
#[derive(Clone, Copy)]
pub struct Colors {
    check: Hsla,
    glow: Hsla,
}

impl Colors {
    pub fn new(theme: &Theme) -> Self {
        Self {
            check: theme.logo_check,
            glow: theme.logo_glow,
        }
    }
}

// ---------- Geometry ----------
#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn of(bounds: Bounds<Pixels>) -> Self {
        Self {
            x: bounds.origin.x.into(),
            y: bounds.origin.y.into(),
            w: bounds.size.width.into(),
            h: bounds.size.height.into(),
        }
    }
    fn bounds(self) -> Bounds<Pixels> {
        Bounds::new(point(px(self.x), px(self.y)), size(px(self.w), px(self.h)))
    }
    fn lerp(self, to: Self, s: f32) -> Self {
        Self {
            x: self.x + (to.x - self.x) * s,
            y: self.y + (to.y - self.y) * s,
            w: self.w + (to.w - self.w) * s,
            h: self.h + (to.h - self.h) * s,
        }
    }
    fn scaled(self, s: f32) -> Self {
        let (cx, cy) = (self.x + self.w / 2., self.y + self.h / 2.);
        Self {
            x: cx - self.w * s / 2.,
            y: cy - self.h * s / 2.,
            w: self.w * s,
            h: self.h * s,
        }
    }
    /// The A's box inside a desktop logo square.
    fn mark_in(square: Self) -> Self {
        let [x0, y0, x1, y1] = logo::square_fractions();
        Self {
            x: square.x + x0 * square.w,
            y: square.y + y0 * square.h,
            w: (x1 - x0) * square.w,
            h: (y1 - y0) * square.h,
        }
    }
    fn tip(self) -> Point {
        let r = logo::raster();
        (
            self.x + r.tip.0 / (r.width - 1) as f32 * self.w,
            self.y + r.tip.1 / (r.height - 1) as f32 * self.h,
        )
    }
    /// Maps a point in the SVG's path coordinates into this box.
    fn map(self, (x, y): Point) -> Point {
        let [x0, y0, x1, y1] = logo::vector().bounds;
        (
            self.x + (x - x0) / (x1 - x0) * self.w,
            self.y + (y - y0) / (y1 - y0) * self.h,
        )
    }
}

/// The afterglow's front: an ellipse around the check's tip in the splash's
/// metric, where x runs across the logo and y is squashed by .55.
#[derive(Clone, Copy)]
struct Front {
    tip: Point,
    radius: f32,
    /// ry / rx
    aspect: f32,
}

impl Front {
    fn new(mark: Rect, radius: f32) -> Self {
        Self {
            tip: mark.tip(),
            radius,
            aspect: mark.h / mark.w / 0.55,
        }
    }
    fn contains(self, (x, y): Point, inset: f32) -> bool {
        let dy = (y - self.tip.1) / self.aspect;
        (x - self.tip.0).hypot(dy) < self.radius - inset
    }
    fn polygon(self, radius: f32) -> Vec<Point> {
        ellipse(self.tip, radius, radius * self.aspect, 96)
    }
}

// ---------- Painting ----------
fn fill_contours(
    window: &mut Window,
    contours: impl IntoIterator<Item = Vec<Point>>,
    clip: Option<&[Point]>,
    background: impl Into<Background>,
    gradient_box: Option<Rect>,
) {
    let mut builder = PathBuilder::fill().with_style(PathStyle::Fill(
        FillOptions::default().with_fill_rule(FillRule::NonZero),
    ));
    let mut any = false;
    for contour in contours {
        let contour = match clip {
            Some(clip) => clip_convex(&contour, clip),
            None => contour,
        };
        if contour.len() < 3 {
            continue;
        }
        any = true;
        builder.move_to(point(px(contour[0].0), px(contour[0].1)));
        for &(x, y) in &contour[1..] {
            builder.line_to(point(px(x), px(y)));
        }
        builder.close();
    }
    if !any {
        return;
    }
    let Ok(mut path) = builder.build() else {
        return;
    };
    if path.vertices.is_empty() {
        return;
    }
    // a gradient spans the path's bounds, so a clipped piece keeps the whole mark's
    if let Some(gradient_box) = gradient_box {
        path.bounds = gradient_box.bounds();
    }
    window.paint_path(path, background);
}

fn mark_gradient() -> Background {
    linear_gradient(
        180.,
        linear_color_stop(hex(MARK_TOP), 0.),
        linear_color_stop(hex(MARK_BOTTOM), 1.),
    )
}

fn a_contours(mark: Rect) -> Vec<Vec<Point>> {
    logo::vector()
        .a
        .iter()
        .map(|contour| contour.iter().map(|&p| mark.map(p)).collect())
        .collect()
}

fn check_contour(mark: Rect) -> Vec<Point> {
    logo::vector().check.iter().map(|&p| mark.map(p)).collect()
}

/// The vector mark, optionally clipped to a convex region.
fn paint_vector(window: &mut Window, mark: Rect, clip: Option<&[Point]>, colors: Colors) {
    fill_contours(window, a_contours(mark), clip, mark_gradient(), Some(mark));
    fill_contours(window, [check_contour(mark)], clip, colors.check, None);
}

/// The landed mark, drawn the way the intro leaves it, in a desktop logo square.
pub fn paint_mark(window: &mut Window, square: Bounds<Pixels>, colors: Colors) {
    paint_vector(window, Rect::mark_in(Rect::of(square)), None, colors);
}

/// The whole mark in one flat colour.
fn paint_silhouette(window: &mut Window, mark: Rect, clip: Option<&[Point]>, fill: Hsla) {
    let mut shapes = a_contours(mark);
    shapes.push(check_contour(mark));
    fill_contours(window, shapes, clip, fill, None);
}

struct CellLook {
    purple: f32,
    check: f32,
    glow: f32,
    glow_amount: f32,
    /// Cells inside the front have resolved into the vector mark.
    front: Option<Front>,
}

/// A port of `splash_cell`: quadrant glyphs drawn as solid sub-cells, as
/// terminals render block elements, sampled at the raster's native 70×25 cells.
fn paint_cells(window: &mut Window, mark: Rect, look: &CellLook, colors: Colors) {
    let raster = logo::raster();
    let (columns, rows) = (raster.width / 2, raster.height / 2);
    let (cw, ch) = (mark.w / columns as f32, mark.h / rows as f32);
    let scale = window.scale_factor();
    // edges snap to device pixels so neighbouring sub-cells abut without seams
    let snap = |v: f32| (v * scale).round() / scale;
    let revealed = |x: usize, (start, end): (usize, usize), progress: f32| {
        progress >= 1. || (x as f32 - start as f32) / ((end - start).max(1) as f32) < progress
    };
    let sample = |x: usize, y: usize| {
        let pixel = raster.at(x, y)?;
        if !revealed(x, raster.occupied, look.purple) {
            return None;
        }
        if pixel == Pixel::White && !revealed(x, raster.white, look.check) {
            return Some(Pixel::Purple);
        }
        Some(pixel)
    };
    let (ex, ey) = (
        raster.tip.0 / (raster.width - 1) as f32,
        raster.tip.1 / (raster.height - 1) as f32,
    );
    let (top, bottom) = (hex(MARK_TOP), hex(MARK_BOTTOM));
    for row in 0..rows {
        let progress = (row * 2) as f32 / (raster.height - 1) as f32;
        let base = mix(top, bottom, progress);
        for column in 0..columns {
            let pixels = [
                sample(column * 2, row * 2),
                sample(column * 2 + 1, row * 2),
                sample(column * 2, row * 2 + 1),
                sample(column * 2 + 1, row * 2 + 1),
            ];
            if pixels.iter().all(Option::is_none) {
                continue;
            }
            let (x0, y0) = (mark.x + column as f32 * cw, mark.y + row as f32 * ch);
            if look
                .front
                .is_some_and(|front| front.contains((x0 + cw / 2., y0 + ch / 2.), 2.))
            {
                continue;
            }
            // afterglow_amount: a ring travels out from the check's tip
            let mut purple = base;
            if look.glow > 0. && look.glow < 1. {
                let dx = (column * 2) as f32 / (raster.width - 1) as f32 - ex;
                let dy = ((row * 2) as f32 / (raster.height - 1) as f32 - ey) * 0.55;
                let ring = (1. - ((dx.hypot(dy)) - look.glow * 1.25).abs() / 0.4).clamp(0., 1.);
                purple = mix(
                    purple,
                    colors.glow,
                    ring * (PI * look.glow).sin() * look.glow_amount,
                );
            }
            let xs = [snap(x0), snap(x0 + cw / 2.), snap(x0 + cw)];
            let ys = [snap(y0), snap(y0 + ch / 2.), snap(y0 + ch)];
            let has_white = pixels.contains(&Some(Pixel::White));
            if has_white && pixels.iter().all(Option::is_some) {
                let cell = Rect {
                    x: xs[0],
                    y: ys[0],
                    w: xs[2] - xs[0],
                    h: ys[2] - ys[0],
                };
                window.paint_quad(fill(cell.bounds(), purple));
            }
            for (index, pixel) in pixels.iter().enumerate() {
                let Some(pixel) = pixel else { continue };
                if has_white && *pixel == Pixel::Purple {
                    continue;
                }
                let (qx, qy) = (index % 2, index / 2);
                let quadrant = Rect {
                    x: xs[qx],
                    y: ys[qy],
                    w: xs[qx + 1] - xs[qx],
                    h: ys[qy + 1] - ys[qy],
                };
                let fill_color = match pixel {
                    Pixel::White => colors.check,
                    Pixel::Purple => purple,
                };
                window.paint_quad(fill(quadrant.bounds(), fill_color));
            }
        }
    }
}

/// Soft light along the front, on the mark only: nested bands of the
/// annulus, each one path so its segments rasterise without seams.
fn paint_front_glow(
    window: &mut Window,
    mark: Rect,
    front: Front,
    band: f32,
    alpha: f32,
    colors: Colors,
) {
    if alpha <= 0. || front.radius <= 0. {
        return;
    }
    const SEGMENTS: usize = 64;
    let shapes = {
        let mut shapes = a_contours(mark);
        shapes.push(check_contour(mark));
        shapes
    };
    for depth in [1., 0.6, 0.3] {
        let inner = (front.radius - band * depth).max(0.);
        let outer = front.radius + band * 0.05;
        let at = |radius: f32, i: usize| {
            let angle = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            (
                front.tip.0 + radius * angle.cos(),
                front.tip.1 + radius * front.aspect * angle.sin(),
            )
        };
        let mut pieces = Vec::new();
        for i in 0..SEGMENTS {
            let segment = [
                at(inner, i),
                at(outer, i),
                at(outer, i + 1),
                at(inner, i + 1),
            ];
            for shape in &shapes {
                let piece = clip_convex(shape, &segment);
                if piece.len() >= 3 {
                    pieces.push(piece);
                }
            }
        }
        fill_contours(window, pieces, None, colors.glow.alpha(alpha / 3.), None);
    }
}

/// A glow around the mark. GPUI has no blur, so the halo is a few faint,
/// slightly larger silhouettes behind it.
fn paint_halo_behind(window: &mut Window, mark: Rect, amount: f32, colors: Colors) {
    if amount <= 0. {
        return;
    }
    for step in 1..=5 {
        let grown = mark.scaled(1. + 0.018 * step as f32);
        paint_silhouette(window, grown, None, colors.glow.alpha(amount * 0.07));
    }
}

/// One frame of the intro at `t` ms over `area`, the window, with the mark
/// heading for the desktop logo square `slot`. Paints nothing once the mark
/// has landed, at [`LANDED`].
pub fn paint(
    window: &mut Window,
    t: f32,
    area: Bounds<Pixels>,
    slot: Bounds<Pixels>,
    colors: Colors,
) {
    if t >= LANDED {
        return;
    }
    let (area, landed) = (Rect::of(area), Rect::mark_in(Rect::of(slot)));
    let height = (area.h * HERO).min(HERO_MAX);
    let width = height * landed.w / landed.h;
    let hero = Rect {
        x: area.x + area.w / 2. - width / 2.,
        y: area.y + area.h * 0.42 - height / 2.,
        w: width,
        h: height,
    };
    let glow = ease_in_out(win(t, GLOW));
    let bump = (PI * smooth(win(t, BUMP))).sin();
    let mark = hero
        .lerp(landed, ease_out_quint(win(t, SETTLE)))
        .scaled(1. + 0.02 * bump);
    let front = Front::new(mark, glow * 1.25 * mark.w);
    let resolved = front.radius >= mark.w * 1.3;

    paint_halo_behind(window, mark, 0.5 * bump, colors);
    if !resolved {
        paint_cells(
            window,
            mark,
            &CellLook {
                purple: smooth(win(t, PURPLE)),
                check: smooth(win(t, CHECK)),
                glow,
                glow_amount: 0.6,
                front: (front.radius > 0.).then_some(front),
            },
            colors,
        );
    }
    if front.radius > 0. {
        let clip = (!resolved).then(|| front.polygon(front.radius));
        paint_vector(window, mark, clip.as_deref(), colors);
    }
    paint_front_glow(
        window,
        mark,
        front,
        mark.w * 0.16,
        0.4 * (PI * glow).sin(),
        colors,
    );
    if bump > 0. {
        paint_silhouette(window, mark, None, colors.glow.alpha(0.14 * bump));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_intro_plays_once() {
        let now = Instant::now();
        let mut playback = Playback::default();
        assert_eq!(playback.frame(|_| 0.), None);
        assert!(!playback.skip());
        assert!(playback.start(now));
        assert!(!playback.start(now));
        assert_eq!(
            playback.frame(|started| (started == now) as u8 as f32),
            Some(1.)
        );
        assert_eq!(playback.frame(|_| END), None);
        assert_eq!(playback, Playback::Played);
        // finished, it neither restarts nor skips
        assert!(!playback.start(now));
        assert!(!playback.skip());
        assert_eq!(playback.frame(|_| 0.), None);
    }

    #[test]
    fn a_key_skips_and_only_platform_shortcuts_run() {
        let key = |text| Keystroke::parse(text).unwrap();
        for (text, swallowed) in [
            ("a", true),
            ("enter", true),
            ("cmd-q", false),
            ("cmd-,", false),
        ] {
            let mut playback = Playback::default();
            playback.start(Instant::now());
            assert_eq!(playback.skip_key(&key(text)), swallowed, "{text}");
            assert_eq!(playback, Playback::Played);
            // once played, keys pass
            assert!(!playback.skip_key(&key(text)));
        }
    }

    #[test]
    fn the_gradient_and_check_match_the_logo_assets() {
        let dark = include_str!("../assets/logo-dark.svg").to_lowercase();
        let light = include_str!("../assets/logo-light.svg").to_lowercase();
        for svg in [&dark, &light] {
            assert!(svg.contains(&format!("offset=\"0\" stop-color=\"#{MARK_TOP:06x}\"")));
            assert!(svg.contains(&format!("offset=\"1\" stop-color=\"#{MARK_BOTTOM:06x}\"")));
        }
        let fill = |svg: &str| {
            let at = svg.rfind("fill=\"#").expect("the check has a flat fill") + 7;
            let digits = &svg[at..svg[at..].find('"').unwrap() + at];
            let digits = match digits.len() {
                3 => digits.chars().flat_map(|c| [c, c]).collect(),
                _ => digits.to_owned(),
            };
            hex(u32::from_str_radix(&digits, 16).unwrap())
        };
        assert_eq!(fill(&dark), Theme::dark().logo_check);
        assert_eq!(fill(&light), Theme::light().logo_check);
    }
}
