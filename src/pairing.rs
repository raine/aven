//! QR presentation for device invitations, shared by the CLI and TUI. It
//! renders an already encoded invitation and never retains or prints the
//! invitation text outside the QR rows.
use std::fmt;

use anyhow::{Result, bail};
use qrcode::{Color, QrCode};

pub(crate) const PAIRING_QUIET_ZONE_MODULES: usize = 4;

const QR_LINE_STYLE: &str = "\x1b[30;47m";
const STYLE_RESET: &str = "\x1b[0m";

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingQr {
    width: usize,
    quiet_zone: usize,
    rows: Vec<String>,
}

impl PairingQr {
    fn encode(payload: &[u8]) -> Result<Self> {
        let Ok(code) = QrCode::new(payload) else {
            bail!("error pairing-qr-too-large");
        };
        let source_width = code.width();
        let colors = code.into_colors();
        let quiet_zone = PAIRING_QUIET_ZONE_MODULES;
        let width = source_width + 2 * quiet_zone;
        let mut rows = Vec::with_capacity(width.div_ceil(2));

        for y in (0..width).step_by(2) {
            let mut row = String::with_capacity(width);
            for x in 0..width {
                row.push(terminal_cell(
                    source_is_dark(&colors, source_width, quiet_zone, x, y),
                    source_is_dark(&colors, source_width, quiet_zone, x, y + 1),
                ));
            }
            rows.push(row);
        }

        Ok(Self {
            width,
            quiet_zone,
            rows,
        })
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn rows(&self) -> &[String] {
        &self.rows
    }
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

fn terminal_cell(top_dark: bool, bottom_dark: bool) -> char {
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
            .finish()
    }
}

/// Display-safe server origin plus the QR of one device invitation.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingPresentation {
    server_identity: String,
    qr: PairingQr,
}

impl PairingPresentation {
    pub(crate) fn new(server_origin: &str, invitation: &str) -> Result<Self> {
        Ok(Self {
            server_identity: server_origin.to_string(),
            qr: PairingQr::encode(invitation.as_bytes())?,
        })
    }

    pub(crate) fn server_identity(&self) -> &str {
        &self.server_identity
    }

    pub(crate) fn qr(&self) -> &PairingQr {
        &self.qr
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
    const TEST_INVITATION: &str = "aven://pair/v2/AgAAAB1pbnZpdGF0aW9uLWZpeHR1cmUtc2VjcmV0";

    fn presentation() -> PairingPresentation {
        PairingPresentation::new(TEST_SERVER, TEST_INVITATION).unwrap()
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
    fn presentation_debug_omits_invitation_and_qr_rows() {
        let presentation = presentation();
        let debug = format!("{presentation:?}");

        assert!(debug.contains(TEST_SERVER));
        assert!(!debug.contains("aven://pair/"));
        for row in presentation.qr().rows() {
            let visible = row.trim();
            if !visible.is_empty() {
                assert!(!debug.contains(visible));
            }
        }
    }

    #[test]
    fn qr_capacity_has_one_actionable_error() {
        let error = PairingPresentation::new(TEST_SERVER, &"t".repeat(3000)).unwrap_err();
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
        assert!(!message.contains("aven://pair/"));
    }
}
