use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThemePref {
    Dark,
    Light,
    #[default]
    System,
}

/// A database opened earlier, with how many tasks it listed when it was left.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KnownDatabase {
    pub path: PathBuf,
    pub tasks: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub nav_width: f32,
    pub detail_width: f32,
    pub theme: ThemePref,
    pub compact: bool,
    pub zoom: f32,
    pub databases: Vec<KnownDatabase>,
    pub nav_open: bool,
    pub details_open: bool,
    pub window: Option<SavedWindow>,
    pub view: Option<SavedView>,
    pub row_fields: RowFields,
}

/// Which optional fields task rows show, the same in every view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RowFields {
    pub labels: bool,
    pub created: bool,
    pub updated: bool,
}

impl Default for RowFields {
    fn default() -> Self {
        Self {
            labels: true,
            created: true,
            updated: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WindowMode {
    #[default]
    Windowed,
    Zoomed,
    FullScreen,
}

/// The window's frame, with its unzoomed bounds when zoomed or full screen.
/// Positions are relative to `display`, named by its UUID.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedWindow {
    #[serde(default)]
    pub display: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub mode: WindowMode,
}

/// What the window showed. `view` is a name from `VIEWS`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedView {
    pub view: String,
    pub project: Option<String>,
    pub columns: bool,
    pub selected: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            nav_width: 228.,
            detail_width: 380.,
            theme: ThemePref::default(),
            compact: false,
            zoom: 1.,
            databases: Vec::new(),
            nav_open: true,
            details_open: true,
            window: None,
            view: None,
            row_fields: RowFields::default(),
        }
    }
}

impl Prefs {
    /// Desktop-local preferences beside the database.
    pub fn path(database: &Path) -> PathBuf {
        database.with_file_name("desktop-prefs.json")
    }

    /// Unfinished writing, beside the preferences.
    pub fn drafts_path(prefs: &Path) -> PathBuf {
        prefs.with_file_name("desktop-drafts.json")
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?).context("write preferences")
    }
}

/// Writing that outlives the app: the composer's draft and each task's note.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SavedDrafts {
    pub composer: Option<crate::draft::SavedDraft>,
    pub notes: Vec<(String, String)>,
}

impl SavedDrafts {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if *self == Self::default() {
            return match std::fs::remove_file(path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(error).context("clear drafts")
                }
                _ => Ok(()),
            };
        }
        std::fs::write(path, serde_json::to_vec(self)?).context("write drafts")
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::PendingImage;
    use super::*;
    use aven_core::choices::TaskPriority;
    use aven_core::choices::TaskStatus;
    use std::sync::Arc;

    #[test]
    fn preferences_keep_the_window_and_old_files_still_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prefs.json");
        std::fs::write(&path, r#"{"nav_width": 200.0}"#).unwrap();
        let mut prefs = Prefs::load(&path);
        assert_eq!(prefs.nav_width, 200.);
        assert!(prefs.nav_open && prefs.details_open);
        assert_eq!((prefs.theme, prefs.zoom), (ThemePref::System, 1.));
        assert_eq!(
            prefs.row_fields,
            RowFields {
                labels: true,
                created: true,
                updated: false,
            }
        );
        prefs.window = Some(SavedWindow {
            display: Some("37D8832A-2D66-02CA-B9F7-8F30A301B230".into()),
            x: 10.,
            y: 20.,
            width: 1000.,
            height: 700.,
            mode: WindowMode::Zoomed,
        });
        prefs.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), prefs);
    }

    #[test]
    fn drafts_survive_a_save_and_leave_no_file_when_empty() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("drafts.json");
        let draft = crate::draft::Draft {
            title: "Half written".to_string(),
            status: Some(TaskStatus::Todo),
            priority: TaskPriority::High,
            images: vec![PendingImage {
                name: "a.png".to_string(),
                bytes: Arc::new(vec![1, 2, 3]),
            }],
            ..Default::default()
        };
        let saved = SavedDrafts {
            composer: Some((&draft).into()),
            notes: vec![("task".to_string(), "A note".to_string())],
        };
        saved.save(&path).unwrap();
        let loaded = SavedDrafts::load(&path);
        assert_eq!(loaded, saved);
        assert_eq!(crate::draft::Draft::from(loaded.composer.unwrap()), draft);
        SavedDrafts::default().save(&path).unwrap();
        assert!(!path.exists());
    }
}
