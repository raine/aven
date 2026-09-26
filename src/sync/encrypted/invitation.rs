//! Text handoff for setup and device invitations. Debug output is redacted.
//!
//! Device invitation text is defined in `aven_core::sync::device_invitation`;
//! this module adds server origin validation. Setup invitations are
//! provisional: `aven-sync-setup-1:` and base64url of the setup ID and secret
//! followed by the server origin.
use std::fmt;

use anyhow::{Context, Result, ensure};
use aven_core::sync::device_invitation;
use aven_core::sync::seed_claim::{Secret, membership::Invitation};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use zeroize::Zeroizing;

const SETUP_PREFIX: &str = "aven-sync-setup-1:";
const MAX_SERVER_BYTES: usize = 2048;

pub(crate) struct SetupInvitation {
    pub(crate) server: String,
    pub(in crate::sync) setup_id: [u8; 32],
    pub(in crate::sync) secret: Secret,
}

pub(crate) struct DeviceInvitation {
    pub(crate) server: String,
    pub(in crate::sync) invitation: Invitation,
}

impl fmt::Debug for SetupInvitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SetupInvitation([REDACTED])")
    }
}

impl fmt::Debug for DeviceInvitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceInvitation([REDACTED])")
    }
}

/// Validates a server URL for encrypted transport and returns its origin.
pub(in crate::sync) fn server_origin(url: &str) -> Result<String> {
    crate::seed_bootstrap_http::Client::new(url).context(
        "error sync-server-url-invalid hint=\"use an origin such as https://sync.example.com, or http:// only with a loopback address; no path, query or credentials\"",
    )?;
    let origin = reqwest::Url::parse(url)?.origin().ascii_serialization();
    ensure!(
        origin.len() <= MAX_SERVER_BYTES,
        "error sync-server-url-too-long"
    );
    Ok(origin)
}

impl SetupInvitation {
    pub(in crate::sync) fn encode(&self) -> Zeroizing<String> {
        let mut secret = Zeroizing::new(self.setup_id.to_vec());
        secret.extend_from_slice(self.secret.expose());
        encode(SETUP_PREFIX, &secret, &self.server)
    }

    pub(crate) fn decode(text: &str) -> Result<Self> {
        let (secret, server) = decode(SETUP_PREFIX, text, 64)
            .ok_or_else(|| anyhow::anyhow!("error sync-setup-invitation-invalid"))?;
        Ok(Self {
            server,
            setup_id: secret[..32].try_into()?,
            secret: Secret::new(secret[32..].try_into()?),
        })
    }
}

impl DeviceInvitation {
    pub(in crate::sync) fn encode(&self) -> Result<Zeroizing<String>> {
        // Protected storage order is vault, inviter HPKE public key, PSK,
        // matching the invitation text.
        let fields = self.invitation.protected_storage_bytes();
        let secret: &[u8; device_invitation::SECRET_BYTES] = fields[..].try_into()?;
        device_invitation::encode(&self.server, secret)
    }

    pub(crate) fn decode(text: &str) -> Result<Self> {
        Self::decode_fields(text)
            .ok_or_else(|| anyhow::anyhow!("error sync-device-invitation-invalid"))
    }

    fn decode_fields(text: &str) -> Option<Self> {
        let decoded = device_invitation::decode(text)?;
        if server_origin(&decoded.server).ok()? != decoded.server {
            return None;
        }
        Some(Self {
            invitation: Invitation::from_protected_storage(decoded.secret.as_slice()).ok()?,
            server: decoded.server,
        })
    }
}

/// What pasted text looks like as an invitation. Holds only the server
/// origin, never the secret.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum InvitationCheck {
    #[default]
    Empty,
    Setup(String),
    Device(String),
    /// Has an invitation prefix but doesn't decode.
    Incomplete,
    Unknown,
}

impl InvitationCheck {
    pub(crate) fn of(text: &str) -> Self {
        let text = text.trim();
        if text.is_empty() {
            Self::Empty
        } else if let Ok(invitation) = SetupInvitation::decode(text) {
            Self::Setup(invitation.server)
        } else if let Ok(invitation) = DeviceInvitation::decode(text) {
            Self::Device(invitation.server)
        } else if text.starts_with(SETUP_PREFIX) || device_invitation::has_prefix(text) {
            Self::Incomplete
        } else {
            Self::Unknown
        }
    }
}

fn encode(prefix: &str, secret: &[u8], server: &str) -> Zeroizing<String> {
    let mut bytes = Zeroizing::new(secret.to_vec());
    bytes.extend_from_slice(server.as_bytes());
    Zeroizing::new(format!(
        "{prefix}{}",
        URL_SAFE_NO_PAD.encode(bytes.as_slice())
    ))
}

fn decode(prefix: &str, text: &str, secret_len: usize) -> Option<(Zeroizing<Vec<u8>>, String)> {
    let encoded = text.trim().strip_prefix(prefix)?;
    if encoded.len() > (secret_len + MAX_SERVER_BYTES) * 4 / 3 + 4 {
        return None;
    }
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded).ok()?);
    if bytes.len() <= secret_len {
        return None;
    }
    let server = std::str::from_utf8(&bytes[secret_len..]).ok()?;
    let origin = server_origin(server).ok()?;
    (origin == server).then(|| (Zeroizing::new(bytes[..secret_len].to_vec()), origin))
}

/// Encoded setup and device invitations for `server`, for tests outside
/// this module.
#[cfg(test)]
pub(crate) fn sample_invitations(server: &str) -> (Zeroizing<String>, Zeroizing<String>) {
    let setup = SetupInvitation {
        server: server.into(),
        setup_id: [3; 32],
        secret: Secret::new([4; 32]),
    };
    let mut storage = vec![5; 32];
    storage.extend([6; 32]);
    storage.extend([7; 32]);
    let device = DeviceInvitation {
        server: server.into(),
        invitation: Invitation::from_protected_storage(&storage).expect("sample invitation"),
    };
    (setup.encode(), device.encode().expect("sample invitation"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invitations_round_trip_and_refuse_other_kinds() {
        let setup = SetupInvitation {
            server: "https://sync.example.net".into(),
            setup_id: [3; 32],
            secret: Secret::new([4; 32]),
        };
        let text = setup.encode();
        let decoded = SetupInvitation::decode(&format!("  {}\n", text.as_str())).unwrap();
        assert_eq!(decoded.server, setup.server);
        assert_eq!(decoded.setup_id, setup.setup_id);
        assert_eq!(decoded.secret.expose(), setup.secret.expose());
        assert!(DeviceInvitation::decode(&text).is_err());
        assert_eq!(
            InvitationCheck::of(&text),
            InvitationCheck::Setup(setup.server.clone())
        );
        assert_eq!(
            InvitationCheck::of(&text[..60]),
            InvitationCheck::Incomplete
        );
        assert_eq!(InvitationCheck::of("hello"), InvitationCheck::Unknown);
        assert_eq!(InvitationCheck::of("  "), InvitationCheck::Empty);
        assert_eq!(format!("{decoded:?}"), "SetupInvitation([REDACTED])");

        let mut storage = vec![5; 32];
        storage.extend([6; 32]);
        storage.extend([7; 32]);
        let device = DeviceInvitation {
            server: "https://sync.example.net".into(),
            invitation: Invitation::from_protected_storage(&storage).unwrap(),
        };
        let text = device.encode().unwrap();
        assert!(text.starts_with(device_invitation::PREFIX));
        let decoded = DeviceInvitation::decode(&text.to_ascii_lowercase()).unwrap();
        assert_eq!(decoded.server, device.server);
        assert_eq!(
            *decoded.invitation.protected_storage_bytes(),
            *device.invitation.protected_storage_bytes()
        );
        assert!(SetupInvitation::decode(&text).is_err());
        assert_eq!(
            InvitationCheck::of(&text),
            InvitationCheck::Device(device.server.clone())
        );
        assert_eq!(
            InvitationCheck::of(&text[..40].to_ascii_lowercase()),
            InvitationCheck::Incomplete
        );

        // The text format accepts any origin; decoding here also refuses
        // origins encrypted sync can't use.
        for server in ["http://sync.example.net", "https://sync.example.net/x"] {
            let text = device_invitation::encode(server, &[1; 96]).unwrap();
            assert!(DeviceInvitation::decode(&text).is_err(), "{server}");
            assert_eq!(InvitationCheck::of(&text), InvitationCheck::Incomplete);
        }

        let plaintext_remote = encode(SETUP_PREFIX, &[0; 64], "http://sync.example.net");
        assert!(SetupInvitation::decode(&plaintext_remote).is_err());
        let with_path = encode(SETUP_PREFIX, &[0; 64], "https://sync.example.net/x");
        assert!(SetupInvitation::decode(&with_path).is_err());
    }
}
