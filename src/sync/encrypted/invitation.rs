//! Text handoff for setup and device invitations. Debug output is redacted.
//!
//! Device invitations use the pairing URI `aven://pair/v2/` followed by
//! unpadded base64url of `U8(2) || B(server_url) || B(vault_id) ||
//! B(inviter_hpke_public_key) || B(psk)`, where `B(x) = U32(len(x)) || x`
//! big-endian, with the 4096-byte decoded pairing invitation cap. Setup
//! invitations are provisional: `aven-sync-setup-1:` and base64url of the
//! setup ID and secret followed by the server origin.
use std::fmt;

use anyhow::{Result, ensure};
use aven_core::sync::seed_claim::{Secret, membership::Invitation};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use zeroize::Zeroizing;

const SETUP_PREFIX: &str = "aven-sync-setup-1:";
const DEVICE_PREFIX: &str = "aven://pair/v2/";
const DEVICE_VERSION: u8 = 2;
const MAX_DEVICE_BYTES: usize = 4096;
const MAX_SERVER_BYTES: usize = 2048;

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
        // Protected storage order is vault, inviter HPKE public key, PSK.
        let fields = self.invitation.protected_storage_bytes();
        let mut bytes = Zeroizing::new(vec![DEVICE_VERSION]);
        for field in [
            self.server.as_bytes(),
            &fields[..32],
            &fields[32..64],
            &fields[64..],
        ] {
            bytes.extend_from_slice(&(field.len() as u32).to_be_bytes());
            bytes.extend_from_slice(field);
        }
        Zeroizing::new(format!(
            "{DEVICE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(bytes.as_slice())
        ))
    }

    pub(in crate::sync) fn decode(text: &str) -> Result<Self> {
        Self::decode_fields(text)
            .ok_or_else(|| anyhow::anyhow!("error sync-device-invitation-invalid"))
    }

    fn decode_fields(text: &str) -> Option<Self> {
        let encoded = text.trim().strip_prefix(DEVICE_PREFIX)?;
        if encoded.len() > MAX_DEVICE_BYTES.div_ceil(3) * 4 {
            return None;
        }
        let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded).ok()?);
        let (&version, mut rest) = bytes.split_first()?;
        if version != DEVICE_VERSION || bytes.len() > MAX_DEVICE_BYTES {
            return None;
        }
        let mut field = || -> Option<&[u8]> {
            let (length, tail) = rest.split_first_chunk::<4>()?;
            let length = usize::try_from(u32::from_be_bytes(*length)).ok()?;
            let (value, tail) = (tail.len() >= length).then(|| tail.split_at(length))?;
            rest = tail;
            Some(value)
        };
        let server = std::str::from_utf8(field()?).ok()?.to_string();
        let mut secret = Zeroizing::new(Vec::with_capacity(96));
        for _ in 0..3 {
            let value = field()?;
            if value.len() != 32 {
                return None;
            }
            secret.extend_from_slice(value);
        }
        if !rest.is_empty() || server_origin(&server).ok()? != server {
            return None;
        }
        Some(Self {
            server,
            invitation: Invitation::from_protected_storage(&secret).ok()?,
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

        let mut storage = vec![5; 32];
        storage.extend([6; 32]);
        storage.extend([7; 32]);
        let device = DeviceInvitation {
            server: "https://sync.example.net".into(),
            invitation: Invitation::from_protected_storage(&storage).unwrap(),
        };
        let text = device.encode();
        let mut expected = vec![2, 0, 0, 0, 24];
        expected.extend(b"https://sync.example.net");
        for byte in [5, 6, 7] {
            expected.extend([0, 0, 0, 32]);
            expected.extend([byte; 32]);
        }
        assert_eq!(
            text.as_str(),
            format!("aven://pair/v2/{}", URL_SAFE_NO_PAD.encode(&expected))
        );
        let decoded = DeviceInvitation::decode(&text).unwrap();
        assert_eq!(decoded.server, device.server);
        assert_eq!(
            *decoded.invitation.protected_storage_bytes(),
            *device.invitation.protected_storage_bytes()
        );
        assert!(SetupInvitation::decode(&text).is_err());
        expected[0] = 1;
        assert!(
            DeviceInvitation::decode(&format!(
                "aven://pair/v2/{}",
                URL_SAFE_NO_PAD.encode(&expected)
            ))
            .is_err()
        );

        let plaintext_remote = encode(SETUP_PREFIX, &[0; 64], "http://sync.example.net");
        assert!(SetupInvitation::decode(&plaintext_remote).is_err());
        let with_path = encode(SETUP_PREFIX, &[0; 64], "https://sync.example.net/x");
        assert!(SetupInvitation::decode(&with_path).is_err());
    }
}
