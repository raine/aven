//! Bookend, the first-launch intro: the TUI splash at hero size, resolved into
//! the vector mark by its afterglow, which then settles into the welcome
//! screen's header while the content rises in.
//!
//! just bookend (a bare `cargo run` window cannot take keyboard focus)
//!
//! R replays, space pauses, ←/→ step a frame (shift: 100 ms), enter jumps to
//! the end, S cycles the speed, T switches the theme, Q quits.
//! BOOKEND_T=1100 opens paused on that frame; BOOKEND_LIGHT=1 opens in the
//! light theme.
mod logo;

use std::cell::Cell;
use std::f32::consts::PI;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    App, Background, Bounds, Context, FillOptions, FillRule, FocusHandle, Hsla, Keystroke,
    PathBuilder, PathStyle, Pixels, Render, Rgba, Subscription, TitlebarOptions, Window,
    WindowBounds, WindowOptions, canvas, div, fill, linear_color_stop, linear_gradient, point,
    prelude::*, px, size,
};
use gpui_platform::application;

use logo::{Pixel, Point, clip_convex, ellipse, rect};

// ---------- Timeline, in ms ----------
const PURPLE: (f32, f32) = (0., 400.);
const CHECK: (f32, f32) = (320., 700.);
const GLOW: (f32, f32) = (750., 1450.);
const BUMP: (f32, f32) = (750., 1550.);
const SETTLE: (f32, f32) = (1300., 1820.);
const RISE_AT: f32 = 1620.;
const RISE_MS: f32 = 360.;
const RISE_STAGGER: f32 = 45.;
const ITEMS: usize = 6;
const END: f32 = RISE_AT + RISE_STAGGER * (ITEMS - 1) as f32 + RISE_MS;
/// The hero mark's height as a share of the window, and its cap in px.
const HERO: f32 = 0.5;
const HERO_MAX: f32 = 380.;
const SPEEDS: [f32; 4] = [1., 0.5, 0.25, 0.1];

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
type Rgb = [f32; 3];

fn mix(a: Rgb, b: Rgb, m: f32) -> Rgb {
    [
        a[0] + (b[0] - a[0]) * m,
        a[1] + (b[1] - a[1]) * m,
        a[2] + (b[2] - a[2]) * m,
    ]
}
fn color(c: Rgb, alpha: f32) -> Hsla {
    Rgba {
        r: c[0] / 255.,
        g: c[1] / 255.,
        b: c[2] / 255.,
        a: alpha,
    }
    .into()
}
fn hex(value: u32) -> Rgb {
    [
        (value >> 16 & 0xff) as f32,
        (value >> 8 & 0xff) as f32,
        (value & 0xff) as f32,
    ]
}

/// The desktop mark's gradient, top to bottom. GPUI gradients take two stops,
/// so the cells use the same two.
const MARK_TOP: u32 = 0xbe85fb;
const MARK_BOTTOM: u32 = 0x7628ff;

#[derive(Clone, Copy)]
struct Palette {
    page: Rgb,
    panel: Rgb,
    line: Rgb,
    ink: Rgb,
    muted: Rgb,
    faint: Rgb,
    check: Rgb,
    glow: Rgb,
}

impl Palette {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                page: hex(0x181716),
                panel: hex(0x1f1e1b),
                line: hex(0x2c2a27),
                ink: hex(0xeeece6),
                muted: hex(0xaca69e),
                faint: hex(0x918b83),
                check: hex(0xffffff),
                glow: [232., 213., 255.],
            }
        } else {
            Self {
                page: hex(0xfdfcfa),
                panel: hex(0xf6f4ef),
                line: hex(0xe7e2d9),
                ink: hex(0x1a1816),
                muted: hex(0x68625c),
                faint: hex(0x756e67),
                check: hex(0x2a1547),
                glow: [150., 95., 245.],
            }
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

fn mark_gradient(alpha: f32) -> Background {
    linear_gradient(
        180.,
        linear_color_stop(color(hex(MARK_TOP), alpha), 0.),
        linear_color_stop(color(hex(MARK_BOTTOM), alpha), 1.),
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

/// The vector mark, optionally clipped to a convex region. An unrevealed check
/// reads as part of the A, as in the splash.
fn paint_vector(
    window: &mut Window,
    mark: Rect,
    clip: Option<&[Point]>,
    check: f32,
    palette: Palette,
) {
    let check_shape = check_contour(mark);
    let mut shapes = a_contours(mark);
    if check < 1. {
        shapes.push(check_shape.clone());
    }
    fill_contours(window, shapes, clip, mark_gradient(1.), Some(mark));
    if check <= 0. {
        return;
    }
    let mut shape = check_shape;
    if check < 1. {
        let (x0, x1) = shape
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        shape = clip_convex(
            &shape,
            &rect(
                x0 - 1.,
                mark.y - mark.h,
                x0 + (x1 - x0) * check,
                mark.y + mark.h * 2.,
            ),
        );
    }
    fill_contours(window, [shape], clip, color(palette.check, 1.), None);
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
fn paint_cells(window: &mut Window, mark: Rect, look: &CellLook, palette: Palette) {
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
                    palette.glow,
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
                window.paint_quad(fill(cell.bounds(), color(purple, 1.)));
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
                    Pixel::White => palette.check,
                    Pixel::Purple => purple,
                };
                window.paint_quad(fill(quadrant.bounds(), color(fill_color, 1.)));
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
    palette: Palette,
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
        fill_contours(window, pieces, None, color(palette.glow, alpha / 3.), None);
    }
}

/// A glow around and over the mark. GPUI has no blur, so the halo is a few
/// faint, slightly larger silhouettes behind it and a tint over it.
fn paint_halo_behind(window: &mut Window, mark: Rect, amount: f32, palette: Palette) {
    if amount <= 0. {
        return;
    }
    for step in 1..=5 {
        let grown = mark.scaled(1. + 0.018 * step as f32);
        paint_silhouette(window, grown, None, color(palette.glow, amount * 0.07));
    }
}

/// One frame of the intro, until the mark lands in its header slot.
fn paint_intro(window: &mut Window, t: f32, area: Rect, slot: Rect, palette: Palette) {
    let landed = Rect::mark_in(slot);
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

    paint_halo_behind(window, mark, 0.5 * bump, palette);
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
            palette,
        );
    }
    if front.radius > 0. {
        let clip = (!resolved).then(|| front.polygon(front.radius));
        paint_vector(window, mark, clip.as_deref(), 1., palette);
    }
    paint_front_glow(
        window,
        mark,
        front,
        mark.w * 0.16,
        0.4 * (PI * glow).sin(),
        palette,
    );
    if bump > 0. {
        paint_silhouette(window, mark, None, color(palette.glow, 0.14 * bump));
    }
}

// ---------- The view ----------
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
    let start = RISE_AT + RISE_STAGGER * index as f32;
    let p = ease_out_quint(win(t, (start, start + RISE_MS)));
    div()
        .w_full()
        .flex()
        .justify_center()
        .relative()
        .top(px(8. * (1. - p)))
        .opacity(p)
        .child(child)
}

impl Render for Bookend {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.clock.now();
        if t < END && !self.clock.paused {
            window.request_animation_frame();
        }
        let palette = Palette::new(self.dark);
        let landed = t >= SETTLE.1;
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
                .border_color(color(palette.line, 1.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(color(palette.ink, 1.))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(palette.muted, 1.))
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
            .bg(color(palette.page, 1.))
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
                                        paint_vector(window, Rect::mark_in(Rect::of(bounds)), None, 1., palette);
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
                            .text_color(color(palette.ink, 1.))
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
                            .text_color(color(palette.muted, 1.))
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
                            .bg(color(palette.panel, 1.))
                            .border_1()
                            .border_color(color(palette.line, 1.))
                            .text_size(px(15.))
                            .text_color(color(palette.faint, 1.))
                            .child("What's the first thing on your mind?")
                            .child(
                                div()
                                    .px(px(10.))
                                    .py(px(6.))
                                    .rounded(px(7.))
                                    .bg(color(mix(palette.panel, hex(0xb68af5), 0.25), 1.))
                                    .text_size(px(13.))
                                    .text_color(color(palette.muted, 1.))
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
                            .text_color(color(palette.faint, 1.))
                            .child("Adds to ● Personal  PRS")
                            .child("More details ⌘↵"),
                    ))
                    .child(rise(
                        4,
                        t,
                        div()
                            .mt(px(34.))
                            .mb(px(12.))
                            .text_size(px(12.))
                            .text_color(color(palette.faint, 1.))
                            .child("Already use Aven?"),
                    ))
                    .child(rise(
                        5,
                        t,
                        div()
                            .w(px(460.))
                            .flex()
                            .gap(px(8.))
                            .child(card("Join existing sync", "Use an invitation from a computer that syncs"))
                            .child(card("Restore a backup", "Bring back tasks from an archive")),
                    )),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        if landed {
                            return;
                        }
                        if let Some(slot) = read_slot.get() {
                            paint_intro(window, t, Rect::of(bounds), Rect::of(slot), palette);
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
                    .text_color(color(palette.faint, 1.))
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
