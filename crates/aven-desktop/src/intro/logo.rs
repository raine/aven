//! The Aven mark in both forms the intro needs: the TUI splash raster and the
//! desktop vector paths, plus the polygon clipping that joins them.
use std::sync::OnceLock;

/// One pixel of the splash raster.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pixel {
    Purple,
    White,
}

/// The TUI splash raster, read with the same thresholds as `src/tui/ui/splash.rs`.
pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Option<Pixel>>,
    /// Horizontal extent of every pixel, which the purple sweep crosses.
    pub occupied: (usize, usize),
    /// Horizontal extent of the check, which the check sweep crosses.
    pub white: (usize, usize),
    /// The check's tip, where the afterglow starts.
    pub tip: (f32, f32),
}

impl Raster {
    pub fn at(&self, x: usize, y: usize) -> Option<Pixel> {
        self.pixels[y * self.width + x]
    }
}

pub fn raster() -> &'static Raster {
    static RASTER: OnceLock<Raster> = OnceLock::new();
    RASTER.get_or_init(|| {
        let image = image::load_from_memory(include_bytes!(
            "../../../../src/tui/ui/assets/aven_logo_terminal.png"
        ))
        .expect("the splash raster is a valid PNG")
        .to_luma_alpha8();
        let (width, height) = (image.width() as usize, image.height() as usize);
        let pixels = image
            .pixels()
            .map(|pixel| match pixel.0 {
                [_, alpha] if alpha < 64 => None,
                [luma, _] if luma > 160 => Some(Pixel::White),
                _ => Some(Pixel::Purple),
            })
            .collect::<Vec<_>>();
        let bounds = |include: &dyn Fn(Pixel) -> bool| {
            let mut bounds = (usize::MAX, 0);
            for (index, pixel) in pixels.iter().enumerate() {
                if pixel.is_some_and(include) {
                    bounds.0 = bounds.0.min(index % width);
                    bounds.1 = bounds.1.max(index % width);
                }
            }
            bounds
        };
        let occupied = bounds(&|_| true);
        let white = bounds(&|pixel| pixel == Pixel::White);
        let rows = (0..height)
            .filter(|y| pixels[y * width + white.1] == Some(Pixel::White))
            .collect::<Vec<_>>();
        let tip = (
            white.1 as f32,
            rows.iter().sum::<usize>() as f32 / rows.len() as f32,
        );
        Raster {
            width,
            height,
            pixels,
            occupied,
            white,
            tip,
        }
    })
}

pub type Point = (f32, f32);

/// The desktop mark's paths, in the SVG's path coordinates.
pub struct Vector {
    /// The A, which has two contours: the apex with the left leg, and the right leg.
    pub a: Vec<Vec<Point>>,
    pub check: Vec<Point>,
    /// x0, y0, x1, y1 of the A.
    pub bounds: [f32; 4],
}

pub fn vector() -> &'static Vector {
    static VECTOR: OnceLock<Vector> = OnceLock::new();
    VECTOR.get_or_init(|| {
        let svg = include_str!("../../assets/logo-dark.svg");
        let mut paths = svg.split(" d=\"").skip(1).map(|rest| {
            let data = &rest[..rest.find('"').expect("path data ends")];
            contours(data)
        });
        let a = paths.next().expect("the mark has an A");
        let check = paths
            .next()
            .and_then(|mut contours| contours.pop())
            .expect("the mark has a check");
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for &(x, y) in a.iter().flatten() {
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }
        Vector { a, check, bounds }
    })
}

/// Parses `M x y L x y … Z` path data, the only commands the mark uses.
fn contours(data: &str) -> Vec<Vec<Point>> {
    let mut contours: Vec<Vec<Point>> = Vec::new();
    let mut numbers = Vec::new();
    for token in data.split_whitespace() {
        match token {
            "M" => contours.push(Vec::new()),
            "L" | "Z" => {}
            number => {
                numbers.push(number.parse::<f32>().expect("path data is numeric"));
                if numbers.len() == 2 {
                    contours
                        .last_mut()
                        .expect("path data starts with M")
                        .push((numbers[0], numbers[1]));
                    numbers.clear();
                }
            }
        }
    }
    contours
}

/// Where the A's bounds sit inside the desktop logo's square, as fractions of
/// its side. The SVG scales its paths by 1.1 inside a 684-unit viewBox.
pub fn square_fractions() -> [f32; 4] {
    let [x0, y0, x1, y1] = vector().bounds;
    let x = |v: f32| (1.1 * v - 52.584 - 170.) / 684.;
    let y = |v: f32| (1.1 * v - 2.897 - 150.) / 684.;
    [x(x0), y(y0), x(x1), y(y1)]
}

/// Clips `subject` to the convex polygon `clip` (Sutherland–Hodgman). Both
/// wind the same way; a concave subject can leave zero-area seams, which fill
/// to nothing.
pub fn clip_convex(subject: &[Point], clip: &[Point]) -> Vec<Point> {
    let mut output = subject.to_vec();
    let orientation = signed_area(clip).signum();
    for (index, &a) in clip.iter().enumerate() {
        if output.is_empty() {
            break;
        }
        let b = clip[(index + 1) % clip.len()];
        let inside =
            |p: Point| orientation * ((b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)) >= 0.;
        let input = std::mem::take(&mut output);
        for (index, &current) in input.iter().enumerate() {
            let previous = input[(index + input.len() - 1) % input.len()];
            match (inside(previous), inside(current)) {
                (true, true) => output.push(current),
                (true, false) => output.push(intersect(previous, current, a, b)),
                (false, true) => {
                    output.push(intersect(previous, current, a, b));
                    output.push(current);
                }
                (false, false) => {}
            }
        }
    }
    output
}

fn signed_area(polygon: &[Point]) -> f32 {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
        .sum::<f32>()
}

fn intersect(p: Point, q: Point, a: Point, b: Point) -> Point {
    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
    let (ex, ey) = (b.0 - a.0, b.1 - a.1);
    let denominator = dx * ey - dy * ex;
    if denominator.abs() < f32::EPSILON {
        return q;
    }
    let t = ((a.0 - p.0) * ey - (a.1 - p.1) * ex) / denominator;
    (p.0 + dx * t, p.1 + dy * t)
}

/// An ellipse as a convex polygon.
pub fn ellipse(center: Point, rx: f32, ry: f32, segments: usize) -> Vec<Point> {
    (0..segments)
        .map(|i| {
            let angle = i as f32 / segments as f32 * std::f32::consts::TAU;
            (center.0 + rx * angle.cos(), center.1 + ry * angle.sin())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Point> {
        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }

    fn area(polygon: &[Point]) -> f32 {
        signed_area(polygon).abs() / 2.
    }

    #[test]
    fn clipping_keeps_the_overlap() {
        let square = rect(0., 0., 10., 10.);
        let overlap = clip_convex(&square, &rect(5., -5., 20., 5.));
        assert!((area(&overlap) - 25.).abs() < 1e-4);
        for &(x, y) in &overlap {
            assert!((5. ..=10.).contains(&x) && (0. ..=5.).contains(&y));
        }
    }

    #[test]
    fn clipping_ignores_the_clip_winding() {
        let square = rect(0., 0., 10., 10.);
        let mut clip = rect(5., -5., 20., 5.);
        clip.reverse();
        assert!((area(&clip_convex(&square, &clip)) - 25.).abs() < 1e-4);
    }

    #[test]
    fn clipping_to_a_containing_or_disjoint_region() {
        let square = rect(0., 0., 10., 10.);
        let inside = clip_convex(&square, &rect(-1., -1., 11., 11.));
        assert!((area(&inside) - 100.).abs() < 1e-4);
        assert!(clip_convex(&square, &rect(20., 20., 30., 30.)).is_empty());
    }

    #[test]
    fn clipping_to_an_ellipse() {
        let circle = ellipse((0., 0.), 5., 5., 256);
        let clipped = clip_convex(&rect(-10., -10., 10., 10.), &circle);
        assert!((area(&clipped) - std::f32::consts::PI * 25.).abs() < 0.1);
        let half = clip_convex(&circle, &rect(0., -10., 10., 10.));
        assert!((area(&half) - area(&circle) / 2.).abs() < 0.1);
    }

    #[test]
    fn the_a_sits_inside_the_logo_square() {
        let [x0, y0, x1, y1] = square_fractions();
        assert!(0. < x0 && x0 < x1 && x1 < 1.);
        assert!(0. < y0 && y0 < y1 && y1 < 1.);
        // centred across the square
        assert!((x0 - (1. - x1)).abs() < 0.01);
    }

    #[test]
    fn the_vector_has_an_a_and_a_check() {
        let vector = vector();
        assert_eq!(vector.a.len(), 2);
        assert!(vector.a.iter().all(|contour| contour.len() >= 3));
        assert!(vector.check.len() >= 3);
        let [x0, y0, x1, y1] = vector.bounds;
        for &(x, y) in &vector.check {
            assert!((x0..=x1).contains(&x) && (y0..=y1).contains(&y));
        }
    }

    #[test]
    fn the_raster_bounds_and_tip() {
        let raster = raster();
        // two pixels per terminal cell each way
        assert!(raster.width.is_multiple_of(2) && raster.height.is_multiple_of(2));
        assert_eq!(raster.pixels.len(), raster.width * raster.height);
        let (start, end) = raster.occupied;
        assert!(start < end && end < raster.width);
        let (white_start, white_end) = raster.white;
        assert!(start <= white_start && white_start < white_end && white_end <= end);
        for (index, pixel) in raster.pixels.iter().enumerate() {
            let x = index % raster.width;
            if pixel.is_some() {
                assert!((start..=end).contains(&x));
            }
            if *pixel == Some(Pixel::White) {
                assert!((white_start..=white_end).contains(&x));
            }
        }
        // the tip is the check's rightmost column, among its white rows
        assert_eq!(raster.tip.0, white_end as f32);
        let y = raster.tip.1.round() as usize;
        assert!(y < raster.height);
        assert!((0..raster.height).any(|y| raster.at(white_end, y) == Some(Pixel::White)));
        assert!(raster.at(white_end, y) == Some(Pixel::White));
    }
}
