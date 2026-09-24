//! Provisional text handoff for setup and device invitations. Each value holds a
//! secret followed by the server origin, base64url encoded after a versioned
//! prefix. Debug output is redacted.
use std::fmt;

use anyhow::{Result, ensure};
use aven_core::sync::seed_claim::{Secret, membership::Invitation};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use zeroize::Zeroizing;

const SETUP_PREFIX: &str = "aven-sync-setup-1:";
const DEVICE_PREFIX: &str = "aven-sync-invite-1:";
const MAX_SERVER_BYTES: usize = 2048;
const DEVICE_SECRET_BYTES: usize = 96;

pub(in crate::sync) struct SetupInvitation {
    pub(in crate::sync) server: String,
    pub(in crate::sync) setup_id: [u8; 32],
    pub(in crate::sync) secret: Secret,
}

pub(in crate::sync) struct DeviceInvitation {
    pub(in crate::sync) server: String,
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
    crate::seed_bootstrap_http::Client::new(url)?;
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

    pub(in crate::sync) fn decode(text: &str) -> Result<Self> {
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
    pub(in crate::sync) fn encode(&self) -> Zeroizing<String> {
        encode(
            DEVICE_PREFIX,
            &self.invitation.protected_storage_bytes(),
            &self.server,
        )
    }

    pub(in crate::sync) fn decode(text: &str) -> Result<Self> {
        let (secret, server) = decode(DEVICE_PREFIX, text, DEVICE_SECRET_BYTES)
            .ok_or_else(|| anyhow::anyhow!("error sync-device-invitation-invalid"))?;
        Ok(Self {
            server,
            invitation: Invitation::from_protected_storage(&secret)
                .map_err(|_| anyhow::anyhow!("error sync-device-invitation-invalid"))?,
        })
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
        assert_eq!(format!("{decoded:?}"), "SetupInvitation([REDACTED])");

        let plaintext_remote = encode(SETUP_PREFIX, &[0; 64], "http://sync.example.net");
        assert!(SetupInvitation::decode(&plaintext_remote).is_err());
        let with_path = encode(SETUP_PREFIX, &[0; 64], "https://sync.example.net/x");
        assert!(SetupInvitation::decode(&with_path).is_err());
    }
}
