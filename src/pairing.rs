//! QR presentation for device invitations, shared by the CLI and TUI. It
//! renders an already encoded invitation and never retains or prints the
//! invitation text outside the QR rows.
use std::fmt;

use std::sync::OnceLock;

use anyhow::{Result, bail};
use qrcode::{Color, EcLevel, QrCode, Version};

use crate::config::QrGlyphsConfig;

pub(crate) const PAIRING_QUIET_ZONE_MODULES: usize = 4;
pub(crate) const TUI_PAIRING_QUIET_ZONE_MODULES: usize = 2;

const QR_LINE_STYLE: &str = "\x1b[30;47m";
const STYLE_RESET: &str = "\x1b[0m";

/// How QR modules are packed into terminal cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QrGlyphs {
    /// One module wide and two tall per cell, with glyphs every terminal font
    /// has.
    HalfBlock,
    /// Two modules wide and three tall per cell (U+1FB00 sextants): about half
    /// the width and two thirds the height, but only where the font has them.
    Sextant,
}

/// Terminals whose default fonts are known to draw sextants.
const SEXTANT_TERMINALS: [&str; 4] = ["alacritty", "kitty", "wezterm", "ghostty"];

/// Environment variables these terminals set, which pass through tmux and
/// other multiplexers.
const SEXTANT_TERMINAL_VARIABLES: [&str; 6] = [
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
    "KITTY_WINDOW_ID",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_PANE",
    "GHOSTTY_RESOURCES_DIR",
];

/// Resolves the configured glyphs; `auto` detects the terminal once per
/// process.
pub(crate) fn qr_glyphs(config: QrGlyphsConfig) -> QrGlyphs {
    static DETECTED: OnceLock<QrGlyphs> = OnceLock::new();
    match config {
        QrGlyphsConfig::HalfBlock => QrGlyphs::HalfBlock,
        QrGlyphsConfig::Sextant => QrGlyphs::Sextant,
        QrGlyphsConfig::Auto => *DETECTED.get_or_init(|| {
            detect_qr_glyphs(|name| std::env::var(name).ok(), tmux_client_terminal)
        }),
    }
}

/// Uses sextants only in terminals known to draw them. Inside tmux, the
/// client terminal reported by tmux decides first, since TERM and
/// TERM_PROGRAM describe tmux itself.
fn detect_qr_glyphs(
    env: impl Fn(&str) -> Option<String>,
    tmux_client_terminal: impl FnOnce() -> Option<String>,
) -> QrGlyphs {
    let known = |name: &str| {
        let name = name.to_ascii_lowercase();
        SEXTANT_TERMINALS
            .iter()
            .any(|terminal| name.contains(terminal))
    };
    let in_tmux = env("TMUX").is_some_and(|value| !value.is_empty());
    let supported = if in_tmux {
        tmux_client_terminal().is_some_and(|name| known(&name))
    } else {
        ["TERM_PROGRAM", "TERM"]
            .iter()
            .any(|name| env(name).is_some_and(|value| known(&value)))
    } || SEXTANT_TERMINAL_VARIABLES
        .iter()
        .any(|name| env(name).is_some_and(|value| !value.is_empty()));
    if supported {
        QrGlyphs::Sextant
    } else {
        QrGlyphs::HalfBlock
    }
}

fn tmux_client_terminal() -> Option<String> {
    let output = std::process::Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "#{client_termtype} #{client_termname}",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingQr {
    width: usize,
    quiet_zone: usize,
    version: i16,
    alphanumeric: bool,
    rows: Vec<String>,
}

impl PairingQr {
    fn encode(payload: &[u8], glyphs: QrGlyphs) -> Result<Self> {
        Self::encode_with(payload, EcLevel::M, PAIRING_QUIET_ZONE_MODULES, glyphs)
    }

    fn encode_with(
        payload: &[u8],
        level: EcLevel,
        quiet_zone: usize,
        glyphs: QrGlyphs,
    ) -> Result<Self> {
        let alphanumeric = payload.iter().all(|&byte| is_qr_alphanumeric(byte));
        let code = if alphanumeric {
            alphanumeric_bits(payload, level).and_then(|bits| QrCode::with_bits(bits, level).ok())
        } else {
            QrCode::with_error_correction_level(payload, level).ok()
        };
        let Some(code) = code else {
            bail!("error pairing-qr-too-large");
        };
        let source_width = code.width();
        let version = match code.version() {
            Version::Normal(version) => version,
            Version::Micro(_) => bail!("error pairing-qr-version"),
        };
        let colors = code.into_colors();
        let modules = source_width + 2 * quiet_zone;
        let dark = |x, y| source_is_dark(&colors, source_width, quiet_zone, x, y);
        let rows = match glyphs {
            QrGlyphs::HalfBlock => half_block_rows(modules, dark),
            QrGlyphs::Sextant => sextant_rows(modules, dark),
        };

        Ok(Self {
            width: rows.first().map_or(0, |row| row.chars().count()),
            quiet_zone,
            version,
            alphanumeric,
            rows,
        })
    }

    /// Terminal columns the rows occupy.
    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn rows(&self) -> &[String] {
        &self.rows
    }
}

/// QR alphanumeric mode stores two characters in 11 bits, against 16 in
/// byte mode.
fn is_qr_alphanumeric(byte: u8) -> bool {
    byte.is_ascii_digit() || byte.is_ascii_uppercase() || b" $%*+-./:".contains(&byte)
}

/// One alphanumeric segment in the smallest version that holds it.
fn alphanumeric_bits(payload: &[u8], level: EcLevel) -> Option<qrcode::bits::Bits> {
    (1..=40).find_map(|version| {
        let mut bits = qrcode::bits::Bits::new(Version::Normal(version));
        bits.push_alphanumeric_data(payload).ok()?;
        bits.push_terminator(level).ok()?;
        Some(bits)
    })
}

fn source_is_dark(
    colors: &[Color],
    source_width: usize,
    quiet_zone: usize,
    x: usize,
    y: usize,
) -> bool {
    let Some(source_x) = x.checked_sub(quiet_zone) else {
        return false;
    };
    let Some(source_y) = y.checked_sub(quiet_zone) else {
        return false;
    };
    source_x < source_width
        && source_y < source_width
        && colors[source_y * source_width + source_x] == Color::Dark
}

/// Packs `modules`² modules, where `dark(x, y)` is false outside the
/// symbol, one column and two rows per cell.
fn half_block_rows(modules: usize, dark: impl Fn(usize, usize) -> bool) -> Vec<String> {
    (0..modules)
        .step_by(2)
        .map(|y| {
            (0..modules)
                .map(|x| half_block_cell(dark(x, y), dark(x, y + 1)))
                .collect()
        })
        .collect()
}

/// Packs modules two columns and three rows per cell. Modules past an odd
/// edge are light, extending the quiet zone.
fn sextant_rows(modules: usize, dark: impl Fn(usize, usize) -> bool) -> Vec<String> {
    (0..modules)
        .step_by(3)
        .map(|y| {
            (0..modules)
                .step_by(2)
                .map(|x| {
                    let mut mask = 0;
                    for (bit, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1), (0, 2), (1, 2)]
                        .into_iter()
                        .enumerate()
                    {
                        if dark(x + dx, y + dy) {
                            mask |= 1 << bit;
                        }
                    }
                    sextant_cell(mask)
                })
                .collect()
        })
        .collect()
}

/// The glyph whose filled sixths match `mask`: bit 0 is top left, bit 1 top
/// right, then the middle and bottom pairs.
fn sextant_cell(mask: usize) -> char {
    ratatui::symbols::pixel::SEXTANTS[mask]
}

fn half_block_cell(top_dark: bool, bottom_dark: bool) -> char {
    match (top_dark, bottom_dark) {
        (false, false) => ' ',
        (true, false) => '▀',
        (false, true) => '▄',
        (true, true) => '█',
    }
}

impl fmt::Debug for PairingQr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingQr")
            .field("width", &self.width)
            .field("quiet_zone", &self.quiet_zone)
            .field("version", &self.version)
            .finish()
    }
}

/// Display-safe server origin plus the QR of one device invitation.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingPresentation {
    server_identity: String,
    qr: PairingQr,
    expires_at: Option<u64>,
}

impl PairingPresentation {
    pub(crate) fn new(server_origin: &str, invitation: &str, glyphs: QrGlyphs) -> Result<Self> {
        Ok(Self {
            server_identity: server_origin.to_string(),
            qr: PairingQr::encode(invitation.as_bytes(), glyphs)?,
            expires_at: None,
        })
    }

    pub(crate) fn new_tui(
        server_origin: &str,
        invitation: &str,
        expires_at: u64,
        glyphs: QrGlyphs,
    ) -> Result<Self> {
        Ok(Self {
            server_identity: server_origin.to_string(),
            qr: PairingQr::encode_with(
                invitation.as_bytes(),
                EcLevel::L,
                TUI_PAIRING_QUIET_ZONE_MODULES,
                glyphs,
            )?,
            expires_at: Some(expires_at),
        })
    }

    pub(crate) fn server_identity(&self) -> &str {
        &self.server_identity
    }

    pub(crate) fn qr(&self) -> &PairingQr {
        &self.qr
    }

    pub(crate) fn expires_at(&self) -> Option<u64> {
        self.expires_at
    }
}

impl fmt::Debug for PairingPresentation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingPresentation")
            .field("server_identity", &self.server_identity)
            .field("qr", &self.qr)
            .finish()
    }
}

/// Terminal columns to check and whether to style the rows, for real
/// terminals only; redirected output is neither measured nor styled.
pub(crate) fn output_options<F>(
    is_terminal: bool,
    no_color_present: bool,
    terminal_size: F,
) -> (Option<usize>, bool)
where
    F: FnOnce() -> Option<u16>,
{
    if !is_terminal {
        return (None, false);
    }
    (
        terminal_size()
            .map(usize::from)
            .filter(|columns| *columns > 0),
        !no_color_present,
    )
}

pub(crate) fn render_terminal_qr(
    qr: &PairingQr,
    terminal_columns: Option<usize>,
    styled: bool,
) -> Result<String> {
    if let Some(columns) = terminal_columns
        && columns < qr.width()
    {
        bail!(
            "error pairing-terminal-too-narrow required_columns={} available_columns={} hint=\"widen the terminal\"",
            qr.width(),
            columns
        );
    }

    let mut rendered = String::with_capacity(qr.rows().len() * (qr.width() + 16));
    for row in qr.rows() {
        if styled {
            rendered.push_str(QR_LINE_STYLE);
        }
        rendered.push_str(row);
        if styled {
            rendered.push_str(STYLE_RESET);
        }
        rendered.push('\n');
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SERVER: &str = "https://sync.example.test:8443";
    const TEST_INVITATION: &str = "AVEN:AEMGC5TFNYXGK6DBNVYGYZJOORSXG5A";

    fn presentation() -> PairingPresentation {
        PairingPresentation::new(
            TEST_SERVER,
            TEST_INVITATION,
            crate::pairing::QrGlyphs::HalfBlock,
        )
        .unwrap()
    }

    #[test]
    fn qr_rows_are_deterministic_and_include_the_quiet_zone() {
        let first = presentation();
        let second = presentation();
        let qr = first.qr();

        assert_eq!(first, second);
        assert_eq!(qr.quiet_zone, PAIRING_QUIET_ZONE_MODULES);
        assert_eq!(qr.rows().len(), qr.width().div_ceil(2));
        assert!(
            qr.rows()
                .iter()
                .all(|row| row.chars().count() == qr.width())
        );
        for row in qr.rows() {
            assert!(row.chars().take(qr.quiet_zone).all(|cell| cell == ' '));
            assert!(
                row.chars()
                    .rev()
                    .take(qr.quiet_zone)
                    .all(|cell| cell == ' ')
            );
        }
        assert!(
            qr.rows()
                .iter()
                .take(qr.quiet_zone / 2)
                .all(|row| row.chars().all(|cell| cell == ' '))
        );
    }

    #[test]
    fn realistic_invitation_uses_one_alphanumeric_segment_up_to_version_7() {
        let (_, invitation) =
            crate::sync::encrypted::sample_invitations("https://sync.example.com");
        let presentation = PairingPresentation::new_tui(
            TEST_SERVER,
            &invitation,
            123,
            crate::pairing::QrGlyphs::HalfBlock,
        )
        .unwrap();
        let qr = presentation.qr();

        assert!(invitation.starts_with("AVEN:"));
        assert!(qr.alphanumeric);
        assert!(qr.version <= 7, "version {}", qr.version);
        // Version 7 has 45 modules, plus the two-module quiet zone.
        assert_eq!(qr.quiet_zone, TUI_PAIRING_QUIET_ZONE_MODULES);
        assert_eq!(qr.width(), 49);
        assert_eq!(qr.rows().len(), 25);

        // The same bytes in byte mode need a larger symbol.
        let bytes = QrCode::with_error_correction_level(
            invitation.to_ascii_lowercase().as_bytes(),
            EcLevel::L,
        )
        .unwrap();
        assert!(bytes.width() > 45);
    }

    #[test]
    fn presentation_debug_omits_invitation_and_qr_rows() {
        let presentation = presentation();
        let debug = format!("{presentation:?}");

        assert!(debug.contains(TEST_SERVER));
        assert!(!debug.contains("AVEN:"));
        for row in presentation.qr().rows() {
            let visible = row.trim();
            if !visible.is_empty() {
                assert!(!debug.contains(visible));
            }
        }
    }

    #[test]
    fn qr_capacity_has_one_actionable_error() {
        let error = PairingPresentation::new(
            TEST_SERVER,
            &"t".repeat(3000),
            crate::pairing::QrGlyphs::HalfBlock,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "error pairing-qr-too-large");
    }

    #[test]
    fn output_options_only_measure_and_style_real_terminals() {
        let redirected = output_options(false, false, || panic!("redirected output measured tty"));
        assert_eq!(redirected, (None, false));
        assert_eq!(output_options(true, false, || Some(120)), (Some(120), true));
        assert_eq!(output_options(true, true, || Some(120)), (Some(120), false));
        assert_eq!(output_options(true, false, || None), (None, true));
    }

    #[test]
    fn terminal_render_honors_width_and_style_modes() {
        let presentation = presentation();
        let qr = presentation.qr();
        let width = qr.width();

        let styled = render_terminal_qr(qr, Some(width), true).unwrap();
        assert!(
            styled
                .lines()
                .all(|line| line.starts_with(QR_LINE_STYLE) && line.ends_with(STYLE_RESET))
        );
        let plain = render_terminal_qr(qr, Some(width), false).unwrap();
        assert!(!plain.contains('\u{1b}'));
        assert_eq!(plain.lines().count(), qr.rows().len());

        let message = format!(
            "{:#}",
            render_terminal_qr(qr, Some(width - 1), false).unwrap_err()
        );
        assert!(message.contains("pairing-terminal-too-narrow"));
        assert!(!message.contains("AVEN:"));
    }

    #[test]
    fn sextant_masks_map_positions_to_glyphs() {
        assert_eq!(sextant_cell(0), ' ');
        assert_eq!(sextant_cell(0b000001), '🬀');
        assert_eq!(sextant_cell(0b000010), '🬁');
        assert_eq!(sextant_cell(0b010101), '▌');
        assert_eq!(sextant_cell(0b101010), '▐');
        assert_eq!(sextant_cell(0b111111), '█');

        // Each module sets the bit for its place in the 2x3 cell.
        for (bit, (x, y)) in [(0, 0), (1, 0), (0, 1), (1, 1), (0, 2), (1, 2)]
            .into_iter()
            .enumerate()
        {
            let rows = sextant_rows(2, |dx, dy| (dx, dy) == (x, y));
            assert_eq!(rows, [sextant_cell(1 << bit).to_string()]);
        }
    }

    #[test]
    fn sextant_rows_pad_odd_edges_with_light_modules() {
        let all_dark = |x: usize, y: usize| x < 5 && y < 5;
        let rows = sextant_rows(5, all_dark);

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.chars().count() == 3));
        // The last column holds one module; the last row holds two.
        assert_eq!(rows[0], "██▌");
        assert_eq!(
            rows[1],
            format!("{0}{0}{1}", sextant_cell(0b1111), sextant_cell(0b0101))
        );
    }

    #[test]
    fn sextant_qr_is_smaller_and_keeps_the_quiet_zone() {
        let (_, invitation) =
            crate::sync::encrypted::sample_invitations("https://sync.example.com");
        let half = PairingPresentation::new_tui(TEST_SERVER, &invitation, 123, QrGlyphs::HalfBlock)
            .unwrap();
        let sextant =
            PairingPresentation::new_tui(TEST_SERVER, &invitation, 123, QrGlyphs::Sextant).unwrap();
        let qr = sextant.qr();

        assert_eq!(half.qr().width(), 49);
        assert_eq!(half.qr().rows().len(), 25);
        assert_eq!(qr.width(), 25);
        assert_eq!(qr.rows().len(), 17);
        assert!(qr.rows().iter().all(|row| row.chars().count() == 25));
        // The two-module quiet zone fills the first cell column and the top
        // two module rows of the first cell row.
        assert!(qr.rows().iter().all(|row| row.starts_with(' ')));
        let bottom_only = [0, 0b010000, 0b100000, 0b110000].map(sextant_cell);
        assert!(qr.rows()[0].chars().all(|cell| bottom_only.contains(&cell)));
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn detection_prefers_sextants_only_in_known_terminals() {
        let no_tmux = || -> Option<String> { panic!("tmux queried outside tmux") };
        for vars in [
            &[("TERM", "alacritty")][..],
            &[("TERM", "xterm-kitty")],
            &[("TERM_PROGRAM", "WezTerm")],
            &[("TERM_PROGRAM", "ghostty")],
            &[("ALACRITTY_WINDOW_ID", "1"), ("TERM", "xterm-256color")],
        ] {
            assert_eq!(
                detect_qr_glyphs(env(vars), no_tmux),
                QrGlyphs::Sextant,
                "{vars:?}"
            );
        }
        for vars in [
            &[][..],
            &[
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("TERM", "xterm-256color"),
            ],
            &[("TERM_PROGRAM", "iTerm.app")],
            &[("TERM_PROGRAM", "vscode")],
            &[("WT_SESSION", "1")],
        ] {
            assert_eq!(
                detect_qr_glyphs(env(vars), no_tmux),
                QrGlyphs::HalfBlock,
                "{vars:?}"
            );
        }
    }

    #[test]
    fn detection_inside_tmux_looks_past_tmux_itself() {
        let tmux = [
            ("TMUX", "/tmp/tmux-501/default,1,0"),
            ("TERM", "tmux-256color"),
        ];
        assert_eq!(
            detect_qr_glyphs(env(&tmux), || Some(" alacritty".into())),
            QrGlyphs::Sextant
        );
        assert_eq!(
            detect_qr_glyphs(env(&tmux), || Some("xterm-256color".into())),
            QrGlyphs::HalfBlock
        );
        let alacritty = [
            ("TMUX", "/tmp/tmux-501/default,1,0"),
            ("TERM_PROGRAM", "tmux"),
            ("ALACRITTY_SOCKET", "/tmp/alacritty.sock"),
        ];
        assert_eq!(
            detect_qr_glyphs(env(&alacritty), || Some("xterm-256color".into())),
            QrGlyphs::Sextant
        );
        // TERM inside tmux names tmux, not the terminal.
        let screen = [("TMUX", "x"), ("TERM", "screen"), ("TERM_PROGRAM", "tmux")];
        assert_eq!(detect_qr_glyphs(env(&screen), || None), QrGlyphs::HalfBlock);
    }

    #[test]
    fn configured_glyphs_override_detection() {
        assert_eq!(qr_glyphs(QrGlyphsConfig::Sextant), QrGlyphs::Sextant);
        assert_eq!(qr_glyphs(QrGlyphsConfig::HalfBlock), QrGlyphs::HalfBlock);
    }
}
