use std::hash::{DefaultHasher, Hash, Hasher};

use crate::config::InlineImagesConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineImageBackend {
    None,
    Iterm2,
    Iterm2Tmux,
    Kitty,
    KittyTmux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineImageProtocol {
    Iterm2,
    Kitty,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InlineImageTerminal<'a> {
    pub(crate) term_program: Option<&'a str>,
    pub(crate) term: Option<&'a str>,
    pub(crate) kitty_window_id: Option<&'a str>,
    pub(crate) wezterm_pane: Option<&'a str>,
    pub(crate) ghostty_resources_dir: Option<&'a str>,
    pub(crate) in_tmux: bool,
}

pub(crate) fn active_backend(
    config: InlineImagesConfig,
    terminal: InlineImageTerminal<'_>,
) -> InlineImageBackend {
    if config == InlineImagesConfig::Off {
        return InlineImageBackend::None;
    }
    match (active_protocol(terminal), config, terminal.in_tmux) {
        (Some(InlineImageProtocol::Iterm2), InlineImagesConfig::Auto, false)
        | (Some(InlineImageProtocol::Iterm2), InlineImagesConfig::On, false) => {
            InlineImageBackend::Iterm2
        }
        (Some(InlineImageProtocol::Iterm2), InlineImagesConfig::On, true) => {
            InlineImageBackend::Iterm2Tmux
        }
        (Some(InlineImageProtocol::Kitty), InlineImagesConfig::Auto, false)
        | (Some(InlineImageProtocol::Kitty), InlineImagesConfig::On, false) => {
            InlineImageBackend::Kitty
        }
        (Some(InlineImageProtocol::Kitty), InlineImagesConfig::On, true) => {
            InlineImageBackend::KittyTmux
        }
        _ => InlineImageBackend::None,
    }
}

pub(crate) fn active_backend_from_env(config: InlineImagesConfig) -> InlineImageBackend {
    let term_program = std::env::var("TERM_PROGRAM").ok();
    let term = std::env::var("TERM").ok();
    let kitty_window_id = std::env::var("KITTY_WINDOW_ID").ok();
    let wezterm_pane = std::env::var("WEZTERM_PANE").ok();
    let ghostty_resources_dir = std::env::var("GHOSTTY_RESOURCES_DIR").ok();
    let in_tmux = std::env::var_os("TMUX").is_some();
    active_backend(
        config,
        InlineImageTerminal {
            term_program: term_program.as_deref(),
            term: term.as_deref(),
            kitty_window_id: kitty_window_id.as_deref(),
            wezterm_pane: wezterm_pane.as_deref(),
            ghostty_resources_dir: ghostty_resources_dir.as_deref(),
            in_tmux,
        },
    )
}

fn active_protocol(terminal: InlineImageTerminal<'_>) -> Option<InlineImageProtocol> {
    if terminal.term_program == Some("iTerm.app") {
        return Some(InlineImageProtocol::Iterm2);
    }
    let kitty_term_program = terminal.term_program.is_some_and(|term_program| {
        term_program.eq_ignore_ascii_case("kitty")
            || term_program.eq_ignore_ascii_case("WezTerm")
            || term_program.eq_ignore_ascii_case("ghostty")
    });
    if matches!(terminal.term, Some("xterm-kitty" | "xterm-ghostty"))
        || terminal.kitty_window_id.is_some()
        || kitty_term_program
        || terminal.wezterm_pane.is_some()
        || terminal.ghostty_resources_dir.is_some()
    {
        return Some(InlineImageProtocol::Kitty);
    }
    None
}

pub(crate) fn kitty_image_identifiers(
    source_hash: &str,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
) -> (u32, u32) {
    let mut hasher = DefaultHasher::new();
    (
        "aven-inline-image",
        std::process::id(),
        source_hash,
        x,
        y,
        width,
        height,
    )
        .hash(&mut hasher);
    let hash = hasher.finish();
    let image_number = (hash as u32 & 0x7fff_ffff).max(1);
    let placement_id = ((hash >> 32) as u32 & 0x7fff_ffff).max(1);
    (image_number, placement_id)
}

pub(crate) fn inline_image_escape(
    encoded_png: &str,
    byte_len: usize,
    width_cols: u16,
    height_rows: u16,
    backend: InlineImageBackend,
    kitty_ids: (u32, u32),
) -> String {
    match backend {
        InlineImageBackend::None => String::new(),
        InlineImageBackend::Iterm2 => {
            iterm2_escape(encoded_png, byte_len, width_cols, height_rows, false)
        }
        InlineImageBackend::Iterm2Tmux => {
            iterm2_escape(encoded_png, byte_len, width_cols, height_rows, true)
        }
        InlineImageBackend::Kitty => {
            kitty_escape(encoded_png, width_cols, height_rows, false, kitty_ids)
        }
        InlineImageBackend::KittyTmux => {
            kitty_escape(encoded_png, width_cols, height_rows, true, kitty_ids)
        }
    }
}

pub(crate) fn inline_image_delete_escape(
    kitty_ids: (u32, u32),
    backend: InlineImageBackend,
) -> Option<String> {
    let tmux = match backend {
        InlineImageBackend::Kitty => false,
        InlineImageBackend::KittyTmux => true,
        InlineImageBackend::None | InlineImageBackend::Iterm2 | InlineImageBackend::Iterm2Tmux => {
            return None;
        }
    };
    let (image_number, placement_id) = kitty_ids;
    let escape = format!("\x1b_Ga=d,d=N,q=2,I={image_number},p={placement_id}\x1b\\");
    Some(if tmux { tmux_wrap(&escape) } else { escape })
}

pub(crate) fn iterm2_escape(
    encoded_png: &str,
    byte_len: usize,
    width_cols: u16,
    height_rows: u16,
    tmux: bool,
) -> String {
    let escape = format!(
        "\x1b]1337;File=inline=1;width={}cells;height={}cells;size={byte_len};preserveAspectRatio=1:{encoded_png}\x07",
        width_cols.max(1),
        height_rows.max(1),
    );
    if tmux { tmux_wrap(&escape) } else { escape }
}

const KITTY_CHUNK_SIZE: usize = 4_000;

fn kitty_escape(
    encoded_png: &str,
    width_cols: u16,
    height_rows: u16,
    tmux: bool,
    kitty_ids: (u32, u32),
) -> String {
    let (image_number, placement_id) = kitty_ids;
    let chunks = encoded_png
        .as_bytes()
        .chunks(KITTY_CHUNK_SIZE)
        .collect::<Vec<_>>();
    let mut escape = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let more = usize::from(index + 1 < chunks.len());
        let chunk = std::str::from_utf8(chunk).expect("base64 is valid UTF-8");
        let apc = if index == 0 {
            format!(
                "\x1b_Ga=T,f=100,t=d,q=2,C=1,N=1,I={image_number},p={placement_id},c={},r={},m={more};{chunk}\x1b\\",
                width_cols.max(1),
                height_rows.max(1)
            )
        } else {
            format!("\x1b_Ga=T,q=2,m={more};{chunk}\x1b\\")
        };
        if tmux {
            escape.push_str(&tmux_wrap(&apc));
        } else {
            escape.push_str(&apc);
        }
    }
    if escape.is_empty() {
        let apc = format!(
            "\x1b_Ga=T,f=100,t=d,q=2,C=1,N=1,I={image_number},p={placement_id},c={},r={}\x1b\\",
            width_cols.max(1),
            height_rows.max(1)
        );
        if tmux {
            escape.push_str(&tmux_wrap(&apc));
        } else {
            escape.push_str(&apc);
        }
    }
    escape
}

fn tmux_wrap(escape: &str) -> String {
    format!("\x1bPtmux;{}\x1b\\", escape.replace('\x1b', "\x1b\x1b"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal() -> InlineImageTerminal<'static> {
        InlineImageTerminal::default()
    }

    impl<'a> InlineImageTerminal<'a> {
        fn term_program(mut self, term_program: Option<&'a str>) -> Self {
            self.term_program = term_program;
            self
        }

        fn term(mut self, term: Option<&'a str>) -> Self {
            self.term = term;
            self
        }

        fn kitty_window_id(mut self, kitty_window_id: Option<&'a str>) -> Self {
            self.kitty_window_id = kitty_window_id;
            self
        }

        fn wezterm_pane(mut self, wezterm_pane: Option<&'a str>) -> Self {
            self.wezterm_pane = wezterm_pane;
            self
        }

        fn ghostty_resources_dir(mut self, ghostty_resources_dir: Option<&'a str>) -> Self {
            self.ghostty_resources_dir = ghostty_resources_dir;
            self
        }

        fn in_tmux(mut self, in_tmux: bool) -> Self {
            self.in_tmux = in_tmux;
            self
        }
    }

    #[test]
    fn auto_uses_iterm_outside_tmux() {
        assert_eq!(
            active_backend(
                InlineImagesConfig::Auto,
                terminal().term_program(Some("iTerm.app")),
            ),
            InlineImageBackend::Iterm2
        );
    }

    #[test]
    fn auto_uses_kitty_graphics_for_known_terminals() {
        for terminal in [
            terminal().term(Some("xterm-kitty")),
            terminal().term(Some("xterm-ghostty")),
            terminal().term_program(Some("kitty")),
            terminal().term_program(Some("WezTerm")),
            terminal().term_program(Some("wezterm")),
            terminal().wezterm_pane(Some("1")),
            terminal().term_program(Some("Ghostty")),
            terminal().term_program(Some("ghostty")),
            terminal().ghostty_resources_dir(Some("/Applications/Ghostty.app/Contents/Resources")),
            terminal().kitty_window_id(Some("3")),
        ] {
            assert_eq!(
                active_backend(InlineImagesConfig::Auto, terminal),
                InlineImageBackend::Kitty
            );
        }
    }

    #[test]
    fn auto_disables_iterm_previews_inside_tmux() {
        assert_eq!(
            active_backend(
                InlineImagesConfig::Auto,
                terminal().term_program(Some("iTerm.app")).in_tmux(true),
            ),
            InlineImageBackend::None
        );
    }

    #[test]
    fn auto_disables_kitty_previews_inside_tmux() {
        assert_eq!(
            active_backend(
                InlineImagesConfig::Auto,
                terminal().term(Some("xterm-kitty")).in_tmux(true),
            ),
            InlineImageBackend::None
        );
    }

    #[test]
    fn unsupported_terminal_has_no_backend() {
        assert_eq!(
            active_backend(
                InlineImagesConfig::On,
                terminal().term_program(Some("Apple_Terminal")),
            ),
            InlineImageBackend::None
        );
    }

    #[test]
    fn forced_on_wraps_iterm_escape_for_tmux() {
        assert_eq!(
            active_backend(
                InlineImagesConfig::On,
                terminal().term_program(Some("iTerm.app")).in_tmux(true),
            ),
            InlineImageBackend::Iterm2Tmux
        );
        let escape = iterm2_escape("cG5nIGJ5dGVz", 9, 20, 6, true);
        assert!(escape.starts_with("\x1bPtmux;\x1b\x1b]1337;File=inline=1;"));
        assert!(escape.contains("width=20cells;height=6cells;size=9;preserveAspectRatio=1:"));
        assert!(escape.ends_with("\x07\x1b\\"));
    }

    #[test]
    fn forced_on_wraps_kitty_escape_for_tmux_backend() {
        assert_eq!(
            active_backend(
                InlineImagesConfig::On,
                terminal().term(Some("xterm-kitty")).in_tmux(true),
            ),
            InlineImageBackend::KittyTmux
        );
    }

    #[test]
    fn kitty_escape_transmits_encoded_png_payload() {
        let escape = kitty_escape("cG5nIGJ5dGVz", 20, 6, false, (11, 12));

        assert!(escape.starts_with("\x1b_Ga=T,f=100,t=d,q=2,C=1,N=1,I=11,p=12,c=20,r=6,m=0;"));
        assert!(escape.contains("cG5nIGJ5dGVz"));
        assert!(escape.ends_with("\x1b\\"));
    }

    #[test]
    fn kitty_escape_chunks_large_payloads_with_bounded_packets() {
        let escape = kitty_escape(&"a".repeat(100_000), 20, 6, false, (11, 12));

        assert!(escape.contains("m=1;"));
        assert!(escape.contains("\x1b_Ga=T,q=2,m=0;"));
        assert!(
            escape
                .split_inclusive("\x1b\\")
                .all(|packet| packet.len() <= 4_096)
        );
    }

    #[test]
    fn forced_on_wraps_kitty_escape_for_tmux_with_bounded_packets() {
        let escape = kitty_escape(&"a".repeat(100_000), 20, 6, true, (11, 12));

        assert!(escape.starts_with("\x1bPtmux;\x1b\x1b_Ga=T,"));
        assert!(escape.ends_with("\x1b\x1b\\\x1b\\"));
        let starts = escape
            .match_indices("\x1bPtmux;")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert!(
            starts
                .windows(2)
                .all(|window| window[1] - window[0] <= 4_096)
        );
        assert!(escape.len() - starts.last().copied().unwrap() <= 4_096);
    }

    #[test]
    fn dispatcher_preserves_iterm2_escape() {
        let escape = inline_image_escape(
            "cG5nIGJ5dGVz",
            9,
            20,
            6,
            InlineImageBackend::Iterm2,
            (11, 12),
        );

        assert!(escape.starts_with("\x1b]1337;File=inline=1;"));
        assert!(escape.contains("width=20cells;height=6cells;size=9;preserveAspectRatio=1:"));
    }

    #[test]
    fn kitty_identifiers_are_stable_and_placement_specific() {
        let first = kitty_image_identifiers(&"0".repeat(64), 4, 7, 20, 6);
        assert_eq!(first, kitty_image_identifiers(&"0".repeat(64), 4, 7, 20, 6));
        assert_ne!(first, kitty_image_identifiers(&"0".repeat(64), 4, 8, 20, 6));
        assert_ne!(first.0, 0);
        assert_ne!(first.1, 0);
    }

    #[test]
    fn kitty_delete_escape_targets_aven_owned_identifiers() {
        assert_eq!(
            inline_image_delete_escape((11, 12), InlineImageBackend::Kitty).unwrap(),
            "\x1b_Ga=d,d=N,q=2,I=11,p=12\x1b\\"
        );
    }

    #[test]
    fn kitty_tmux_delete_escape_is_wrapped() {
        let escape = inline_image_delete_escape((11, 12), InlineImageBackend::KittyTmux).unwrap();

        assert!(escape.starts_with("\x1bPtmux;\x1b\x1b_Ga=d,d=N,q=2,I=11,p=12"));
        assert!(escape.ends_with("\x1b\x1b\\\x1b\\"));
    }

    #[test]
    fn iterm2_delete_escape_is_absent() {
        assert_eq!(
            inline_image_delete_escape((11, 12), InlineImageBackend::Iterm2),
            None
        );
    }
}
