use std::fmt;

use aven_core::api::{PairingInvitation, PairingInvitationError};
use qrcode::{Color, QrCode};

use crate::config::{self, AppConfig};

pub(crate) const PAIRING_QUIET_ZONE_MODULES: usize = 4;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingQr {
    width: usize,
    quiet_zone: usize,
    rows: Vec<String>,
}

impl PairingQr {
    fn encode(payload: &[u8]) -> Result<Self, PairingError> {
        let code = QrCode::new(payload).map_err(|_| PairingError::InvitationTooLarge)?;
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

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingPresentation {
    server_identity: String,
    qr: PairingQr,
}

impl PairingPresentation {
    pub(crate) fn new(server_url: String, auth_token: String) -> Result<Self, PairingError> {
        let invitation =
            PairingInvitation::new(server_url, auth_token).map_err(PairingError::from)?;
        let server_identity = invitation.server_origin().to_string();
        let uri = invitation.encode().map_err(PairingError::from)?;
        let qr = PairingQr::encode(uri.as_bytes())?;

        Ok(Self {
            server_identity,
            qr,
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingError {
    MissingServer,
    MissingToken,
    InvalidServerUrl,
    LoopbackServer,
    InvitationTooLarge,
    InvitationInvalid,
}

impl PairingError {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::MissingServer => "configure sync.server_url",
            Self::MissingToken => "configure a nonempty sync.auth_token",
            Self::InvalidServerUrl => {
                "sync.server_url must be an http or https URL without credentials, query, or fragment"
            }
            Self::LoopbackServer => "sync.server_url must be reachable from the phone",
            Self::InvitationTooLarge => "shorten sync.auth_token or sync.server_url",
            Self::InvitationInvalid => "check sync.server_url and sync.auth_token",
        }
    }
}

impl From<PairingInvitationError> for PairingError {
    fn from(error: PairingInvitationError) -> Self {
        match error {
            PairingInvitationError::EmptyAuthToken => Self::MissingToken,
            PairingInvitationError::InvalidServerUrl => Self::InvalidServerUrl,
            PairingInvitationError::LoopbackServer => Self::LoopbackServer,
            PairingInvitationError::PayloadTooLarge => Self::InvitationTooLarge,
            PairingInvitationError::InvalidInvitation
            | PairingInvitationError::UnsupportedVersion => Self::InvitationInvalid,
            _ => Self::InvitationInvalid,
        }
    }
}

impl fmt::Debug for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason())
    }
}

impl std::error::Error for PairingError {}

pub(crate) fn pairing_invitation(
    config: &AppConfig,
    server_override: Option<&str>,
) -> Result<PairingInvitation, PairingError> {
    let environment = std::env::var("AVEN_SYNC_SERVER").ok();
    let (server, token) = pairing_inputs(config, server_override, environment.as_deref())?;
    PairingInvitation::new(server, token).map_err(PairingError::from)
}

pub(crate) fn pairing_presentation(
    config: &AppConfig,
    server_override: Option<&str>,
) -> Result<PairingPresentation, PairingError> {
    let environment = std::env::var("AVEN_SYNC_SERVER").ok();
    pairing_presentation_from(config, server_override, environment.as_deref())
}

pub(crate) fn pairing_presentation_from(
    config: &AppConfig,
    server_override: Option<&str>,
    environment: Option<&str>,
) -> Result<PairingPresentation, PairingError> {
    let (server, token) = pairing_inputs(config, server_override, environment)?;
    PairingPresentation::new(server, token)
}

pub(crate) fn pairing_input_error(
    config: &AppConfig,
    server_override: Option<&str>,
    environment: Option<&str>,
) -> Option<PairingError> {
    pairing_inputs(config, server_override, environment).err()
}

fn pairing_inputs(
    config: &AppConfig,
    server_override: Option<&str>,
    environment: Option<&str>,
) -> Result<(String, String), PairingError> {
    let server = config::resolve_sync_server_from(server_override, environment, config)
        .map_err(|_| PairingError::MissingServer)?;
    let server = server.trim();
    if server.is_empty() {
        return Err(PairingError::MissingServer);
    }
    let token = config.sync_auth_token().ok_or(PairingError::MissingToken)?;
    Ok((server.to_string(), token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SERVER: &str = "https://sync.example.test:8443/aven";
    const TEST_TOKEN: &str = "pairing-token-fixture-0123456789";

    fn presentation() -> PairingPresentation {
        PairingPresentation::new(TEST_SERVER.to_string(), TEST_TOKEN.to_string()).unwrap()
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
        assert!(
            qr.rows()
                .iter()
                .rev()
                .take(qr.quiet_zone / 2)
                .all(|row| row.chars().all(|cell| cell == ' '))
        );
    }

    #[test]
    fn presentation_debug_omits_secrets_paths_and_qr_rows() {
        let presentation = PairingPresentation::new(
            format!("https://sync.example.test/{TEST_TOKEN}"),
            TEST_TOKEN.to_string(),
        )
        .unwrap();
        let debug = format!("{presentation:?}");

        assert!(debug.contains("https://sync.example.test"));
        assert!(!debug.contains(TEST_TOKEN));
        assert!(!debug.contains("aven://pair/"));
        for row in presentation.qr().rows() {
            let visible = row.trim();
            if !visible.is_empty() {
                assert!(!debug.contains(visible));
            }
        }
    }

    #[test]
    fn resolver_uses_precedence_and_classifies_blank_selected_inputs() {
        let mut config = AppConfig::default();
        config.sync.server_url = Some(" https://configured.example.test/aven ".to_string());
        config.sync.auth_token = Some("  token  ".to_string());

        let explicit = pairing_presentation_from(
            &config,
            Some(" https://explicit.example.test/aven "),
            Some("https://environment.example.test/aven"),
        )
        .unwrap();
        assert_eq!(explicit.server_identity(), "https://explicit.example.test");

        let environment = pairing_presentation_from(
            &config,
            None,
            Some(" https://environment.example.test/aven "),
        )
        .unwrap();
        assert_eq!(
            environment.server_identity(),
            "https://environment.example.test"
        );

        let configured = pairing_presentation_from(&config, None, None).unwrap();
        assert_eq!(
            configured.server_identity(),
            "https://configured.example.test"
        );

        assert_eq!(
            pairing_presentation_from(&config, Some("   "), None).unwrap_err(),
            PairingError::MissingServer
        );
        assert_eq!(
            pairing_presentation_from(&config, None, Some("   ")).unwrap_err(),
            PairingError::MissingServer
        );
        config.sync.auth_token = Some("   ".to_string());
        assert_eq!(
            pairing_presentation_from(&config, None, None).unwrap_err(),
            PairingError::MissingToken
        );
    }

    #[test]
    fn pairing_errors_have_static_secret_safe_diagnostics() {
        for error in [
            PairingError::MissingServer,
            PairingError::MissingToken,
            PairingError::InvalidServerUrl,
            PairingError::LoopbackServer,
            PairingError::InvitationTooLarge,
            PairingError::InvitationInvalid,
        ] {
            for rendered in [error.to_string(), format!("{error:?}")] {
                assert!(!rendered.contains(TEST_TOKEN));
                assert!(!rendered.contains("aven://pair/"));
            }
        }
    }

    #[test]
    fn qr_capacity_has_one_actionable_error() {
        let error =
            PairingPresentation::new(TEST_SERVER.to_string(), "t".repeat(3000)).unwrap_err();
        assert_eq!(error, PairingError::InvitationTooLarge);
    }
}
