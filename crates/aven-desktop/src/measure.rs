//! Named element boxes from both sides of a scene, and their comparison.
//!
//! GPUI keeps its own debug bounds private to the crate and records them only
//! with debug assertions, so tagged elements report through a paint probe in
//! capture builds. Elsewhere `measured` adds nothing to the element.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    fn largest_difference(&self, other: &Self) -> f32 {
        [
            (self.x - other.x).abs(),
            (self.y - other.y).abs(),
            (self.width - other.width).abs(),
            (self.height - other.height).abs(),
        ]
        .into_iter()
        .fold(0., f32::max)
    }
}

pub type NamedBounds = BTreeMap<String, Bounds>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub element: String,
    pub prototype: Option<Bounds>,
    pub gpui: Option<Bounds>,
    pub tolerance: f32,
    pub difference: Option<f32>,
    pub passed: bool,
}

/// One result per expected element. A box missing from either side fails.
pub fn compare(
    elements: &[(String, f32)],
    prototype: &NamedBounds,
    gpui: &NamedBounds,
) -> Vec<Comparison> {
    elements
        .iter()
        .map(|(element, tolerance)| {
            let left = prototype.get(element).copied();
            let right = gpui.get(element).copied();
            let difference = left.zip(right).map(|(a, b)| a.largest_difference(&b));
            Comparison {
                element: element.clone(),
                prototype: left,
                gpui: right,
                tolerance: *tolerance,
                difference,
                passed: difference.is_some_and(|value| value <= *tolerance),
            }
        })
        .collect()
}

#[cfg(feature = "capture")]
mod probe {
    use super::{Bounds, NamedBounds};
    use gpui::{App, Div, Global, ParentElement as _, Styled as _, canvas, px};

    #[derive(Default)]
    pub struct MeasuredBounds(pub NamedBounds);

    impl Global for MeasuredBounds {}

    pub fn measured(element: Div, name: impl Into<String>) -> Div {
        let name = name.into();
        element.relative().child(
            canvas(
                move |bounds, _, cx: &mut App| {
                    let bounds = Bounds {
                        x: bounds.origin.x / px(1.),
                        y: bounds.origin.y / px(1.),
                        width: bounds.size.width / px(1.),
                        height: bounds.size.height / px(1.),
                    };
                    cx.default_global::<MeasuredBounds>().0.insert(name, bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}

#[cfg(feature = "capture")]
pub use probe::{MeasuredBounds, measured};

#[cfg(not(feature = "capture"))]
pub fn measured(element: gpui::Div, _name: impl Into<String>) -> gpui::Div {
    element
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32) -> Bounds {
        Bounds {
            x,
            y: 10.,
            width: 100.,
            height: 28.,
        }
    }

    fn named(bounds: Bounds) -> NamedBounds {
        BTreeMap::from([("row".to_string(), bounds)])
    }

    #[test]
    fn passes_inside_the_tolerance_and_fails_beyond_it() {
        let elements = [("row".to_string(), 1.0)];
        assert!(compare(&elements, &named(at(0.)), &named(at(1.)))[0].passed);
        let shifted = &compare(&elements, &named(at(0.)), &named(at(1.5)))[0];
        assert!(!shifted.passed);
        assert_eq!(shifted.difference, Some(1.5));
    }

    #[test]
    fn every_edge_counts() {
        let elements = [("row".to_string(), 1.0)];
        let mut taller = at(0.);
        taller.height += 2.;
        assert!(!compare(&elements, &named(at(0.)), &named(taller))[0].passed);
    }

    #[test]
    fn a_box_missing_from_either_side_fails() {
        let elements = [("row".to_string(), 1.0)];
        let empty = NamedBounds::new();
        assert!(!compare(&elements, &empty, &named(at(0.)))[0].passed);
        assert!(!compare(&elements, &named(at(0.)), &empty)[0].passed);
    }
}
