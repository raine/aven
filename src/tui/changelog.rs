use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Size;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::tui::app::App;
use crate::tui::markdown::render_markdown_without_link_urls;
use crate::tui::navigation::scroll_with_delta;
use crate::tui::overlay::{ChangelogState, OverlayState};

const LOADING_MESSAGE: &str = "## Loading changelog…";
const UNAVAILABLE_MESSAGE: &str =
    "## Changelog unavailable\n\nAven could not load the changelog from GitHub.";
const CHANGELOG_URL_PREFIX: &str = "https://raw.githubusercontent.com/raine/aven";
const CACHE_SCHEMA: u32 = 1;

#[derive(Deserialize, Serialize)]
struct ChangelogCache {
    schema: u32,
    git_ref: String,
    markdown: String,
}

pub(super) struct ChangelogController {
    fetch: Option<JoinHandle<Result<FetchedChangelog>>>,
    fetch_ref: Option<String>,
    visible_ref: Option<String>,
    cached: Option<(String, String)>,
}

struct FetchedChangelog {
    git_ref: String,
    markdown: String,
}

impl ChangelogController {
    pub(super) fn new() -> Self {
        Self {
            fetch: None,
            fetch_ref: None,
            visible_ref: None,
            cached: None,
        }
    }

    pub(super) fn work_pending(&self) -> bool {
        self.fetch.is_some()
    }
}

impl App {
    pub(super) fn show_changelog(&mut self) {
        let git_ref = self.update.changelog_ref();
        self.changelog.visible_ref = Some(git_ref.clone());
        if cache_enabled()
            && self.changelog.cached.is_none()
            && let Some(cache) = load_cache().filter(|cache| cache.git_ref == git_ref)
        {
            self.changelog.cached = Some((cache.git_ref, cache.markdown));
        }
        if let Some((_, markdown)) = self
            .changelog
            .cached
            .as_ref()
            .filter(|(cached_ref, _)| cached_ref == &git_ref)
        {
            self.overlay = Some(OverlayState::Changelog(ChangelogState {
                markdown: markdown.clone(),
                scroll: 0,
            }));
            return;
        }

        self.overlay = Some(OverlayState::Changelog(ChangelogState {
            markdown: LOADING_MESSAGE.to_string(),
            scroll: 0,
        }));
        if self.changelog.fetch_ref.as_deref() != Some(&git_ref) {
            if let Some(fetch) = self.changelog.fetch.take() {
                fetch.abort();
            }
            self.changelog.fetch_ref = Some(git_ref.clone());
            self.changelog.fetch = Some(tokio::spawn(async move {
                let markdown = fetch_changelog(&git_ref)
                    .await
                    .map(|body| mark_installed_release(&body, crate::update::CURRENT_VERSION))?;
                Ok(FetchedChangelog { git_ref, markdown })
            }));
        }
    }

    pub(super) async fn poll_changelog(&mut self) -> bool {
        let Some(fetch) = self.changelog.fetch.as_ref() else {
            return false;
        };
        if !fetch.is_finished() {
            return false;
        }

        let result = self
            .changelog
            .fetch
            .take()
            .expect("finished changelog fetch must exist")
            .await;
        self.changelog.fetch_ref = None;
        let (git_ref, markdown) = match result {
            Ok(Ok(result)) => (result.git_ref, result.markdown),
            Ok(Err(_)) | Err(_) => (String::new(), UNAVAILABLE_MESSAGE.to_string()),
        };
        let visible =
            git_ref.is_empty() || self.changelog.visible_ref.as_deref() == Some(git_ref.as_str());
        if !git_ref.is_empty() {
            if cache_enabled() {
                save_cache(&ChangelogCache {
                    schema: CACHE_SCHEMA,
                    git_ref: git_ref.clone(),
                    markdown: markdown.clone(),
                });
            }
            self.changelog.cached = Some((git_ref, markdown.clone()));
        }
        if visible && let Some(OverlayState::Changelog(state)) = self.overlay.as_mut() {
            state.markdown = markdown;
            state.scroll = 0;
            return true;
        }
        false
    }

    pub(super) fn handle_changelog_key(
        &mut self,
        mut state: ChangelogState,
        key: KeyEvent,
        terminal_size: Size,
    ) {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            return;
        }
        let page_rows = changelog_dialog_size(terminal_size).1.saturating_sub(3);
        let half_page = page_rows.saturating_add(1) / 2;
        let delta = match key.code {
            KeyCode::Char('j') | KeyCode::Down => Some(1),
            KeyCode::Char('k') | KeyCode::Up => Some(-1),
            KeyCode::Char('d') => Some(half_page as isize),
            KeyCode::Char('u') => Some(-(half_page as isize)),
            KeyCode::PageDown => Some(page_rows as isize),
            KeyCode::PageUp => Some(-(page_rows as isize)),
            _ => None,
        };
        if let Some(delta) = delta {
            let cap = changelog_scroll_cap(&state.markdown, terminal_size);
            state.scroll = scroll_with_delta(state.scroll, delta, cap);
        }
        self.overlay = Some(OverlayState::Changelog(state));
    }
}

pub(crate) fn changelog_scroll_cap(markdown: &str, terminal_size: Size) -> u16 {
    let (width, height) = changelog_dialog_size(terminal_size);
    let content_width = width.saturating_sub(6).max(1) as usize;
    let visible_rows = height.saturating_sub(3) as usize;
    render_markdown_without_link_urls(markdown, content_width)
        .len()
        .saturating_add(2)
        .saturating_sub(visible_rows)
        .try_into()
        .unwrap_or(u16::MAX)
}

pub(crate) fn changelog_dialog_size(terminal_size: Size) -> (u16, u16) {
    (
        terminal_size.width.saturating_sub(8).clamp(56, 96),
        terminal_size.height.saturating_sub(4).clamp(12, 30),
    )
}

fn cache_path() -> Option<PathBuf> {
    cache_base(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        dirs::home_dir().as_deref(),
    )
    .map(|base| base.join("aven").join("changelog.json"))
}

fn cache_base(xdg_cache_home: Option<&std::ffi::OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    xdg_cache_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|path| path.join(".cache")))
}

fn cache_enabled() -> bool {
    !cfg!(test)
}

fn load_cache() -> Option<ChangelogCache> {
    let contents = std::fs::read_to_string(cache_path()?).ok()?;
    let cache = serde_json::from_str::<ChangelogCache>(&contents).ok()?;
    (cache.schema == CACHE_SCHEMA).then_some(cache)
}

fn save_cache(cache: &ChangelogCache) {
    let Some(path) = cache_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) else {
        return;
    };
    let Ok(json) = serde_json::to_vec(cache) else {
        return;
    };
    use std::io::Write;
    if file.write_all(&json).is_ok() {
        let _ = file.persist(path);
    }
}

async fn fetch_changelog(git_ref: &str) -> Result<String> {
    let url = format!("{CHANGELOG_URL_PREFIX}/{git_ref}/CHANGELOG.md");
    let response = crate::update::client()?
        .get(url)
        .send()
        .await
        .context("fetch changelog from GitHub")?
        .error_for_status()
        .context("GitHub returned an error for the changelog")?;
    let source = response.text().await.context("read GitHub changelog")?;
    parse_changelog(Some(&source))
}

fn mark_installed_release(markdown: &str, version: &str) -> String {
    let heading = format!("## v{version}");
    markdown
        .lines()
        .map(|line| {
            if line == heading || line.starts_with(&format!("{heading} ")) {
                format!("{line}  `installed`")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_changelog(source: Option<&str>) -> Result<String> {
    let source = source.map(str::trim).filter(|source| !source.is_empty());
    let Some(source) = source else {
        bail!("changelog source is unavailable");
    };

    let body = if let Some(frontmatter) = source.strip_prefix("---\n") {
        let Some((_, body)) = frontmatter.split_once("\n---\n") else {
            bail!("changelog front matter is malformed");
        };
        body.trim()
    } else {
        source
    };

    if body.is_empty() || !body.lines().any(|line| line.starts_with("## ")) {
        bail!("changelog has no release sections");
    }

    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_changelog_uses_release_content_without_front_matter() {
        let body = parse_changelog(Some(
            "---\ntitle: Changelog\n---\n## Unreleased\n\n## v0.1.19",
        ))
        .unwrap();

        assert_eq!(body, "## Unreleased\n\n## v0.1.19");
    }

    #[test]
    fn installed_release_heading_gets_a_badge() {
        let marked = mark_installed_release(
            "## Unreleased\n\n## v1.2.3 (2026-07-30)\n\n- Fixed it.",
            "1.2.3",
        );

        assert!(marked.contains("## v1.2.3 (2026-07-30)  `installed`"));
        assert!(!marked.lines().next().unwrap().contains("installed"));
    }

    #[test]
    fn unavailable_and_malformed_sources_use_a_stable_fallback() {
        for source in [
            None,
            Some(""),
            Some("---\ntitle: Changelog"),
            Some("# Notes"),
        ] {
            let markdown =
                parse_changelog(source).unwrap_or_else(|_| UNAVAILABLE_MESSAGE.to_string());

            assert_eq!(markdown, UNAVAILABLE_MESSAGE);
        }
    }

    #[test]
    fn scroll_cap_accounts_for_rendered_markdown_wrapping() {
        let markdown = format!(
            "## Release\n\n{}",
            "- a long changelog entry that wraps over several lines in a narrow reader\n".repeat(8)
        );

        assert!(changelog_scroll_cap(&markdown, Size::new(56, 12)) > 0);
    }
}
