use std::path::Path;

use anyhow::Result;
use base64::Engine;

use crate::config::InlineImagesConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineImageBackend {
    None,
    Iterm2,
    Iterm2Tmux,
}

pub(crate) fn active_backend(
    config: InlineImagesConfig,
    term_program: Option<&str>,
    in_tmux: bool,
) -> InlineImageBackend {
    if config == InlineImagesConfig::Off || term_program != Some("iTerm.app") {
        return InlineImageBackend::None;
    }
    match (config, in_tmux) {
        (InlineImagesConfig::Auto, true) => InlineImageBackend::None,
        (InlineImagesConfig::On, true) => InlineImageBackend::Iterm2Tmux,
        _ => InlineImageBackend::Iterm2,
    }
}

pub(crate) fn active_backend_from_env(config: InlineImagesConfig) -> InlineImageBackend {
    let term_program = std::env::var("TERM_PROGRAM").ok();
    let in_tmux = std::env::var_os("TMUX").is_some();
    active_backend(config, term_program.as_deref(), in_tmux)
}

pub(crate) fn iterm2_escape(
    path: &Path,
    width_cols: u16,
    height_rows: u16,
    tmux: bool,
) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let escape = format!(
        "\x1b]1337;File=inline=1;width={}cells;height={}cells;size={};preserveAspectRatio=1:{encoded}\x07",
        width_cols.max(1),
        height_rows.max(1),
        bytes.len()
    );
    Ok(if tmux { tmux_wrap(&escape) } else { escape })
}

fn tmux_wrap(escape: &str) -> String {
    format!("\x1bPtmux;{}\x1b\\", escape.replace('\x1b', "\x1b\x1b"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_uses_iterm_outside_tmux() {
        assert_eq!(
            active_backend(InlineImagesConfig::Auto, Some("iTerm.app"), false),
            InlineImageBackend::Iterm2
        );
    }

    #[test]
    fn auto_disables_previews_inside_tmux() {
        assert_eq!(
            active_backend(InlineImagesConfig::Auto, Some("iTerm.app"), true),
            InlineImageBackend::None
        );
    }

    #[test]
    fn unsupported_terminal_has_no_backend() {
        assert_eq!(
            active_backend(InlineImagesConfig::On, Some("Apple_Terminal"), false),
            InlineImageBackend::None
        );
    }

    #[test]
    fn forced_on_wraps_iterm_escape_for_tmux() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("chart.png");
        std::fs::write(&path, b"png bytes").unwrap();

        assert_eq!(
            active_backend(InlineImagesConfig::On, Some("iTerm.app"), true),
            InlineImageBackend::Iterm2Tmux
        );
        let escape = iterm2_escape(&path, 20, 6, true).unwrap();
        assert!(escape.starts_with("\x1bPtmux;\x1b\x1b]1337;File=inline=1;"));
        assert!(escape.contains("width=20cells;height=6cells;size=9;preserveAspectRatio=1:"));
        assert!(escape.ends_with("\x07\x1b\\"));
    }
}
