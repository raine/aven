//! The checked-in scene catalogue shared by both capture sides.
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The frozen prototype, relative to the repository root.
    pub target: String,
    pub tolerance: Tolerance,
    pub element: Vec<Element>,
    pub scene: Vec<Scene>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tolerance {
    pub default_px: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Element {
    pub name: String,
    pub selector: String,
    #[serde(default)]
    pub index: usize,
    pub tolerance_px: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub name: String,
    pub theme: Theme,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub prototype_steps: Vec<String>,
    #[serde(default)]
    pub gpui_keys: Vec<String>,
    /// Where the pointer rests in the window, in logical pixels.
    pub gpui_pointer: Option<[f32; 2]>,
    /// Takes the picture this long after the keys, to catch a motion in flight.
    pub gpui_frame_ms: Option<u64>,
    /// Element names, which must all exist on both sides.
    pub measure: Vec<String>,
    /// Element names that must be missing or empty on both sides.
    #[serde(default)]
    pub absent: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let manifest: Self = toml::from_str(&text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        for scene in &self.scene {
            if scene.measure.is_empty() {
                bail!("scene {} measures nothing", scene.name);
            }
            for name in scene.measure.iter().chain(&scene.absent) {
                self.element(name)
                    .with_context(|| format!("scene {}", scene.name))?;
            }
        }
        Ok(())
    }

    pub fn element(&self, name: &str) -> Result<&Element> {
        self.element
            .iter()
            .find(|element| element.name == name)
            .with_context(|| format!("unknown element {name}"))
    }

    /// Expected elements with their tolerances, in the scene's order.
    pub fn expectations(&self, scene: &Scene) -> Result<Vec<(String, f32)>> {
        scene
            .measure
            .iter()
            .map(|name| {
                let tolerance = self.element(name)?.tolerance_px;
                Ok((name.clone(), tolerance.unwrap_or(self.tolerance.default_px)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_manifest_covers_dark_light_and_narrow() {
        let manifest =
            Manifest::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes/manifest.toml"))
                .unwrap();
        let widest = manifest
            .scene
            .iter()
            .map(|scene| scene.width)
            .max()
            .unwrap();
        assert!(
            manifest
                .scene
                .iter()
                .any(|scene| scene.theme == Theme::Dark)
        );
        assert!(
            manifest
                .scene
                .iter()
                .any(|scene| scene.theme == Theme::Light)
        );
        assert!(manifest.scene.iter().any(|scene| scene.width < widest));
    }

    #[test]
    fn a_scene_cannot_name_an_unknown_element() {
        let manifest: Manifest = toml::from_str(
            r#"
            target = "x"
            tolerance = { default_px = 1.0 }
            element = []
            [[scene]]
            name = "a"
            theme = "dark"
            width = 1
            height = 1
            measure = ["ghost"]
            "#,
        )
        .unwrap();
        assert!(format!("{:#}", manifest.validate().unwrap_err()).contains("ghost"));
    }
}
