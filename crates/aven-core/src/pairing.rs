use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

const PAIRING_INVITATION_URI_PREFIX: &str = "aven://pair/v1/";
const PAIRING_INVITATION_ROOT: &str = "aven://pair/";
const MAX_PAIRING_PAYLOAD_BYTES: usize = 4096;
const MAX_ENCODED_PAYLOAD_CHARS: usize = (MAX_PAIRING_PAYLOAD_BYTES * 4).div_ceil(3);
const REDACTED: &str = "[REDACTED]";

#[derive(Clone, PartialEq, Eq)]
pub struct PairingInvitation {
    server_url: String,
    server_origin: String,
    auth_token: String,
}

impl PairingInvitation {
    pub fn new(server_url: String, auth_token: String) -> Result<Self, PairingInvitationError> {
        let url = Url::parse(&server_url).map_err(|_| PairingInvitationError::InvalidServerUrl)?;
        if !crate::sync::wire::sync_server_url_is_valid_url(&url) {
            return Err(PairingInvitationError::InvalidServerUrl);
        }
        if server_host_is_loopback(url.host()) {
            return Err(PairingInvitationError::LoopbackServer);
        }

        let auth_token = auth_token.trim().to_string();
        if auth_token.is_empty() {
            return Err(PairingInvitationError::EmptyAuthToken);
        }

        let invitation = Self {
            server_url: url.to_string(),
            server_origin: url.origin().ascii_serialization(),
            auth_token,
        };
        invitation.serialized_payload()?;
        Ok(invitation)
    }

    pub fn decode(uri: &str) -> Result<Self, PairingInvitationError> {
        let rest = uri
            .strip_prefix(PAIRING_INVITATION_ROOT)
            .ok_or(PairingInvitationError::InvalidInvitation)?;
        let (version, payload) = rest
            .split_once('/')
            .ok_or(PairingInvitationError::InvalidInvitation)?;
        version
            .strip_prefix('v')
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or(PairingInvitationError::InvalidInvitation)?;
        if version != "v1" {
            return Err(PairingInvitationError::UnsupportedVersion);
        }
        if payload.is_empty()
            || payload
                .bytes()
                .any(|byte| matches!(byte, b'/' | b'?' | b'#'))
        {
            return Err(PairingInvitationError::InvalidInvitation);
        }
        if payload.len() > MAX_ENCODED_PAYLOAD_CHARS {
            return Err(PairingInvitationError::PayloadTooLarge);
        }

        let decoded = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| PairingInvitationError::InvalidInvitation)?;
        if decoded.len() > MAX_PAIRING_PAYLOAD_BYTES {
            return Err(PairingInvitationError::PayloadTooLarge);
        }
        let wire: PairingInvitationWire = serde_json::from_slice(&decoded)
            .map_err(|_| PairingInvitationError::InvalidInvitation)?;
        Self::new(wire.server_url, wire.auth_token)
    }

    pub fn encode(&self) -> Result<String, PairingInvitationError> {
        let payload = self.serialized_payload()?;
        Ok(format!(
            "{PAIRING_INVITATION_URI_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(payload)
        ))
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn server_origin(&self) -> &str {
        &self.server_origin
    }

    pub fn auth_token(&self) -> &str {
        &self.auth_token
    }

    fn serialized_payload(&self) -> Result<Vec<u8>, PairingInvitationError> {
        let payload = serde_json::to_vec(&PairingInvitationWire {
            server_url: self.server_url.clone(),
            auth_token: self.auth_token.clone(),
        })
        .map_err(|_| PairingInvitationError::InvalidInvitation)?;
        if payload.len() > MAX_PAIRING_PAYLOAD_BYTES {
            return Err(PairingInvitationError::PayloadTooLarge);
        }
        Ok(payload)
    }
}

fn server_host_is_loopback(host: Option<Host<&str>>) -> bool {
    match host {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => {
            address.is_loopback()
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| mapped.is_loopback())
        }
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.');
            domain.eq_ignore_ascii_case("localhost")
                || domain
                    .rsplit_once('.')
                    .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("localhost"))
        }
        None => false,
    }
}

impl fmt::Debug for PairingInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingInvitation")
            .field("server_origin", &self.server_origin)
            .field("auth_token", &REDACTED)
            .finish()
    }
}

impl fmt::Display for PairingInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Aven pairing invitation for {} with {REDACTED} token",
            self.server_origin
        )
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingInvitationWire {
    server_url: String,
    auth_token: String,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingInvitationError {
    InvalidInvitation,
    UnsupportedVersion,
    PayloadTooLarge,
    EmptyAuthToken,
    InvalidServerUrl,
    LoopbackServer,
}

impl fmt::Display for PairingInvitationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInvitation => "invalid pairing invitation",
            Self::UnsupportedVersion => "unsupported pairing invitation version",
            Self::PayloadTooLarge => "pairing invitation payload is too large",
            Self::EmptyAuthToken => "pairing invitation auth token is empty",
            Self::InvalidServerUrl => "pairing invitation server URL is invalid",
            Self::LoopbackServer => "pairing invitation server URL is loopback",
        })
    }
}

impl std::error::Error for PairingInvitationError {}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde::Serialize;

    use super::*;

    const TEST_SERVER: &str = "https://sync.example.test:8443/aven";
    const TEST_TOKEN: &str = "pairing-token-fixture-0123456789";
    const BOUNDARY_SERVER: &str = "https://sync.example.test/aven";

    #[derive(Serialize)]
    struct TestWire<'a> {
        server_url: &'a str,
        auth_token: String,
    }

    fn fixture() -> PairingInvitation {
        PairingInvitation::new(TEST_SERVER.to_string(), TEST_TOKEN.to_string()).unwrap()
    }

    fn raw_json_with_size(target: usize) -> Vec<u8> {
        let empty = serde_json::to_vec(&TestWire {
            server_url: BOUNDARY_SERVER,
            auth_token: String::new(),
        })
        .unwrap();
        let bytes = serde_json::to_vec(&TestWire {
            server_url: BOUNDARY_SERVER,
            auth_token: "t".repeat(target - empty.len()),
        })
        .unwrap();
        assert_eq!(bytes.len(), target);
        bytes
    }

    fn raw_uri(json: &[u8]) -> String {
        format!(
            "{PAIRING_INVITATION_URI_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(json)
        )
    }

    #[test]
    fn version_one_encoding_is_stable_and_round_trips() {
        let invitation = fixture();
        let encoded = invitation.encode().unwrap();
        assert_eq!(
            encoded,
            "aven://pair/v1/eyJzZXJ2ZXJfdXJsIjoiaHR0cHM6Ly9zeW5jLmV4YW1wbGUudGVzdDo4NDQzL2F2ZW4iLCJhdXRoX3Rva2VuIjoicGFpcmluZy10b2tlbi1maXh0dXJlLTAxMjM0NTY3ODkifQ"
        );
        assert_eq!(PairingInvitation::decode(&encoded).unwrap(), invitation);
        assert_eq!(invitation.server_url(), TEST_SERVER);
        assert_eq!(invitation.server_origin(), "https://sync.example.test:8443");
        assert_eq!(invitation.auth_token(), TEST_TOKEN);
    }

    #[test]
    fn server_url_is_canonical_across_host_url_parsers() {
        let invitation = PairingInvitation::new(
            r"https://sync.example.test\@redirect.example.test/private".to_string(),
            TEST_TOKEN.to_string(),
        )
        .unwrap();

        assert_eq!(
            invitation.server_url(),
            "https://sync.example.test/@redirect.example.test/private"
        );
        assert_eq!(invitation.server_origin(), "https://sync.example.test");
    }

    #[test]
    fn unsupported_versions_are_distinct_from_invalid_invitations() {
        let payload = fixture()
            .encode()
            .unwrap()
            .strip_prefix(PAIRING_INVITATION_URI_PREFIX)
            .unwrap()
            .to_string();
        for uri in [
            String::new(),
            "https://sync.example.test/aven".to_string(),
            format!("aven://PAIR/v1/{payload}"),
            format!("aven://pair/v/{payload}"),
            format!("aven://pair/vx/{payload}"),
            format!("aven://pair/v1/{payload}/extra"),
            format!("aven://pair/v1/{payload}?source=paste"),
            format!("aven://pair/v1/{payload}#fragment"),
            format!("{PAIRING_INVITATION_URI_PREFIX}not*base64"),
            raw_uri(br#"["#),
        ] {
            assert_eq!(
                PairingInvitation::decode(&uri).unwrap_err(),
                PairingInvitationError::InvalidInvitation,
                "uri={uri}"
            );
        }
        for version in ["v0", "v01", "v2"] {
            assert_eq!(
                PairingInvitation::decode(&format!("aven://pair/{version}/{payload}")).unwrap_err(),
                PairingInvitationError::UnsupportedVersion
            );
        }
    }

    #[test]
    fn invalid_values_and_literal_loopback_hosts_are_rejected() {
        let cases = [
            (
                "https://sync.example.test",
                "   ",
                PairingInvitationError::EmptyAuthToken,
            ),
            ("not a url", "t", PairingInvitationError::InvalidServerUrl),
            (
                "ftp://sync.example.test",
                "t",
                PairingInvitationError::InvalidServerUrl,
            ),
            (
                "https://user@sync.example.test",
                "t",
                PairingInvitationError::InvalidServerUrl,
            ),
            (
                "https://sync.example.test/?value=1",
                "t",
                PairingInvitationError::InvalidServerUrl,
            ),
            (
                "http://127.0.0.1:8080",
                "t",
                PairingInvitationError::LoopbackServer,
            ),
            (
                "http://127.5.5.5",
                "t",
                PairingInvitationError::LoopbackServer,
            ),
            (
                "http://[::1]:8080",
                "t",
                PairingInvitationError::LoopbackServer,
            ),
            (
                "http://[::ffff:127.0.0.1]",
                "t",
                PairingInvitationError::LoopbackServer,
            ),
            (
                "http://localhost:8080",
                "t",
                PairingInvitationError::LoopbackServer,
            ),
            (
                "http://localhost.:8080",
                "t",
                PairingInvitationError::LoopbackServer,
            ),
            (
                "http://app.localhost:8080",
                "t",
                PairingInvitationError::LoopbackServer,
            ),
        ];
        for (server, token, expected) in cases {
            assert_eq!(
                PairingInvitation::new(server.to_string(), token.to_string()).unwrap_err(),
                expected,
                "server={server}"
            );
        }
    }

    #[test]
    fn private_http_public_https_and_trimmed_tokens_round_trip() {
        for server in [
            "http://192.168.1.20:8080/aven",
            "https://sync.example.com/aven",
        ] {
            let invitation =
                PairingInvitation::new(server.to_string(), "  token  \n".to_string()).unwrap();
            assert_eq!(invitation.auth_token(), "token");
            assert_eq!(
                PairingInvitation::decode(&invitation.encode().unwrap()).unwrap(),
                invitation
            );
        }

        let padded =
            raw_uri(br#"{"server_url":"https://sync.example.test/aven","auth_token":"  token  "}"#);
        assert_eq!(
            PairingInvitation::decode(&padded).unwrap().auth_token(),
            "token"
        );
    }

    #[test]
    fn payload_bounds_are_checked_before_decode_allocation() {
        let fitting_json = raw_json_with_size(MAX_PAIRING_PAYLOAD_BYTES);
        let fitting = PairingInvitation::decode(&raw_uri(&fitting_json)).unwrap();
        assert!(fitting.encode().is_ok());

        let oversized_json = raw_json_with_size(MAX_PAIRING_PAYLOAD_BYTES + 1);
        assert_eq!(
            PairingInvitation::decode(&raw_uri(&oversized_json)).unwrap_err(),
            PairingInvitationError::PayloadTooLarge
        );
        assert_eq!(
            PairingInvitation::decode(&format!(
                "{PAIRING_INVITATION_URI_PREFIX}{}",
                "*".repeat(MAX_ENCODED_PAYLOAD_CHARS + 1)
            ))
            .unwrap_err(),
            PairingInvitationError::PayloadTooLarge
        );
    }

    #[test]
    fn formatting_and_errors_reveal_only_the_server_origin() {
        let invitation = PairingInvitation::new(
            format!("https://sync.example.test/{TEST_TOKEN}"),
            TEST_TOKEN.to_string(),
        )
        .unwrap();
        let encoded = invitation.encode().unwrap();
        let payload = encoded.strip_prefix(PAIRING_INVITATION_URI_PREFIX).unwrap();

        for rendered in [format!("{invitation:?}"), invitation.to_string()] {
            assert!(rendered.contains("https://sync.example.test"));
            assert!(!rendered.contains(TEST_TOKEN));
            assert!(!rendered.contains(&encoded));
            assert!(!rendered.contains(payload));
            assert!(rendered.contains(REDACTED));
        }

        let error = PairingInvitation::decode(&format!("aven://pair/v2/{payload}")).unwrap_err();
        for rendered in [format!("{error:?}"), error.to_string()] {
            assert!(!rendered.contains(TEST_TOKEN));
            assert!(!rendered.contains(&encoded));
            assert!(!rendered.contains(payload));
        }
    }
}
