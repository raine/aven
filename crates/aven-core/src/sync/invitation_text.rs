//! Invitation text, shared by every client.
//!
//! Setup and device invitations share one payload: `U8(1) || U8(kind) ||
//! U8(len(server)) || server || secret`, where kind 1 is a setup invitation
//! with a 64-byte secret (setup ID, then setup secret) and kind 2 is a device
//! invitation with a 96-byte secret (vault ID, inviter HPKE public key, PSK).
//! The server is an origin encrypted sync accepts.
//!
//! The prefix picks the text alphabet, since base32 text is also valid
//! base64url:
//! - Device invitations are `AVEN:` and unpadded RFC 4648 base32 (`A`–`Z`,
//!   `2`–`7`). Every character is in the QR alphanumeric set, so the QR code
//!   needs no byte mode. Decoding ignores letter case.
//! - Setup invitations are `aven-setup:` and unpadded base64url, which is
//!   shorter; they are pasted, never scanned.
//!
//! Decoding ignores surrounding whitespace.
use std::fmt;

use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use zeroize::Zeroizing;

use super::client::{MAX_SERVER_BYTES, server_origin};

pub const DEVICE_PREFIX: &str = "AVEN:";
pub const SETUP_PREFIX: &str = "aven-setup:";
const VERSION: u8 = 1;
const MAX_PAYLOAD_BYTES: usize = 3 + MAX_SERVER_BYTES + Kind::Device.secret_len();
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Setup,
    Device,
}

impl Kind {
    pub const fn secret_len(self) -> usize {
        match self {
            Self::Setup => 64,
            Self::Device => 96,
        }
    }

    fn byte(self) -> u8 {
        match self {
            Self::Setup => 1,
            Self::Device => 2,
        }
    }

    /// The kind whose prefix `text` starts with, even if the rest doesn't
    /// decode.
    pub fn of_prefix(text: &str) -> Option<Self> {
        let text = text.trim();
        if text
            .get(..DEVICE_PREFIX.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(DEVICE_PREFIX))
        {
            Some(Self::Device)
        } else if text.starts_with(SETUP_PREFIX) {
            Some(Self::Setup)
        } else {
            None
        }
    }
}

/// A decoded invitation. Debug output is redacted.
pub struct InvitationText {
    pub kind: Kind,
    pub server: String,
    pub secret: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for InvitationText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InvitationText([REDACTED])")
    }
}

/// Encodes an invitation; `server` must be an accepted origin.
pub fn encode(kind: Kind, server: &str, secret: &[u8]) -> Result<Zeroizing<String>> {
    ensure!(
        server_origin(server)? == server,
        "error sync-server-url-invalid"
    );
    ensure!(
        secret.len() == kind.secret_len(),
        "error sync-invitation-secret"
    );
    let mut payload = Zeroizing::new(Vec::with_capacity(3 + server.len() + secret.len()));
    payload.extend([VERSION, kind.byte(), server.len() as u8]);
    payload.extend_from_slice(server.as_bytes());
    payload.extend_from_slice(secret);
    let mut text = Zeroizing::new(String::with_capacity(
        SETUP_PREFIX.len() + payload.len() * 8 / 5 + 1,
    ));
    match kind {
        Kind::Device => {
            text.push_str(DEVICE_PREFIX);
            base32_encode(&payload, &mut text);
        }
        Kind::Setup => {
            text.push_str(SETUP_PREFIX);
            URL_SAFE_NO_PAD.encode_string(payload.as_slice(), &mut text);
        }
    }
    Ok(text)
}

/// Decodes either kind of invitation, refusing servers encrypted sync can't
/// use.
pub fn decode(text: &str) -> Option<InvitationText> {
    let kind = Kind::of_prefix(text)?;
    let text = text.trim();
    let prefix_len = match kind {
        Kind::Device => DEVICE_PREFIX.len(),
        Kind::Setup => SETUP_PREFIX.len(),
    };
    let encoded = &text[prefix_len..];
    if encoded.len() > MAX_PAYLOAD_BYTES.div_ceil(5) * 8 {
        return None;
    }
    let payload = match kind {
        Kind::Device => base32_decode(&Zeroizing::new(encoded.to_ascii_uppercase()))?,
        Kind::Setup => Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded).ok()?),
    };
    let [version, kind_byte, server_len, rest @ ..] = payload.as_slice() else {
        return None;
    };
    let server_len = usize::from(*server_len);
    if *version != VERSION
        || *kind_byte != kind.byte()
        || rest.len() != server_len + kind.secret_len()
    {
        return None;
    }
    let (server, secret) = rest.split_at(server_len);
    let server = std::str::from_utf8(server).ok()?;
    if server_origin(server).ok()? != server {
        return None;
    }
    Some(InvitationText {
        kind,
        server: server.to_string(),
        secret: Zeroizing::new(secret.to_vec()),
    })
}

fn base32_encode(bytes: &[u8], out: &mut String) {
    let mut buffer = 0u16;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u16::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[usize::from((buffer >> bits) & 31)] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[usize::from((buffer << (5 - bits)) & 31)] as char);
    }
}

/// Decodes canonical unpadded base32: impossible lengths and nonzero
/// trailing bits are refused.
fn base32_decode(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    if matches!(text.len() % 8, 1 | 3 | 6) {
        return None;
    }
    let mut out = Zeroizing::new(Vec::with_capacity(text.len() * 5 / 8));
    let mut buffer = 0u16;
    let mut bits = 0;
    for &symbol in text.as_bytes() {
        let value = ALPHABET.iter().position(|&c| c == symbol)? as u16;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    (buffer == 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = "https://sync.example.com";

    fn secret(kind: Kind) -> Vec<u8> {
        (0..kind.secret_len()).map(|i| (i / 32 + 5) as u8).collect()
    }

    fn payload(version: u8, kind: u8, server: &str, secret: &[u8]) -> Vec<u8> {
        let mut payload = vec![version, kind, server.len() as u8];
        payload.extend(server.as_bytes());
        payload.extend(secret);
        payload
    }

    fn device_text(payload: &[u8]) -> String {
        let mut text = DEVICE_PREFIX.to_string();
        base32_encode(payload, &mut text);
        text
    }

    fn setup_text(payload: &[u8]) -> String {
        format!("{SETUP_PREFIX}{}", URL_SAFE_NO_PAD.encode(payload))
    }

    #[test]
    fn round_trips_with_the_documented_layout() {
        let device = encode(Kind::Device, SERVER, &secret(Kind::Device)).unwrap();
        assert_eq!(
            device.as_str(),
            device_text(&payload(1, 2, SERVER, &secret(Kind::Device)))
        );
        assert!(
            device
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b':')
        );
        let setup = encode(Kind::Setup, SERVER, &secret(Kind::Setup)).unwrap();
        assert_eq!(
            setup.as_str(),
            setup_text(&payload(1, 1, SERVER, &secret(Kind::Setup)))
        );

        for (kind, text) in [(Kind::Device, &device), (Kind::Setup, &setup)] {
            let decoded = decode(&format!("  {}\n", text.as_str())).unwrap();
            assert_eq!(decoded.kind, kind);
            assert_eq!(decoded.server, SERVER);
            assert_eq!(*decoded.secret, secret(kind));
            assert_eq!(format!("{decoded:?}"), "InvitationText([REDACTED])");
            assert_eq!(Kind::of_prefix(text), Some(kind));
        }
        assert_eq!(
            decode(&device.to_ascii_lowercase()).unwrap().kind,
            Kind::Device
        );
        assert_eq!(Kind::of_prefix(" aven:x"), Some(Kind::Device));
        assert_eq!(Kind::of_prefix("hello"), None);
    }

    #[test]
    fn refuses_truncation_and_bad_alphabets() {
        for kind in [Kind::Device, Kind::Setup] {
            let text = encode(kind, SERVER, &secret(kind)).unwrap();
            let prefix_len = text.find(':').unwrap() + 1;
            for end in [prefix_len, prefix_len + 1, text.len() / 2, text.len() - 1] {
                assert!(decode(&text[..end]).is_none(), "{kind:?} {end}");
                assert_eq!(Kind::of_prefix(&text[..end]), Some(kind));
            }
            assert!(decode(&format!("{}A", text.as_str())).is_none());
        }
        let text = encode(Kind::Device, SERVER, &secret(Kind::Device)).unwrap();
        let body = &text[DEVICE_PREFIX.len()..];
        for bad in ["1", "8", "=", "-", " "] {
            let broken = format!("{DEVICE_PREFIX}{bad}{}", &body[1..]);
            assert!(decode(&broken).is_none(), "{bad}");
        }
        // Nonzero trailing bits are not canonical.
        let last = text.chars().last().unwrap();
        let bumped = ALPHABET[(ALPHABET.iter().position(|&c| c as char == last).unwrap() + 1) % 32];
        assert!(decode(&format!("{}{}", &text[..text.len() - 1], bumped as char)).is_none());
        // Setup text in the device alphabet is still refused.
        assert!(decode(&format!("{SETUP_PREFIX}{body}")).is_none());
    }

    #[test]
    fn refuses_other_versions_kinds_and_lengths() {
        let device = secret(Kind::Device);
        let setup = secret(Kind::Setup);
        assert!(decode(&device_text(&payload(2, 2, SERVER, &device))).is_none());
        assert!(decode(&device_text(&payload(1, 1, SERVER, &device))).is_none());
        assert!(decode(&device_text(&payload(1, 1, SERVER, &setup))).is_none());
        assert!(decode(&setup_text(&payload(1, 2, SERVER, &device))).is_none());
        assert!(decode(&setup_text(&payload(1, 1, SERVER, &device))).is_none());
        let mut long = payload(1, 2, SERVER, &device);
        long.push(0);
        assert!(decode(&device_text(&long)).is_none());
        assert!(encode(Kind::Setup, SERVER, &device).is_err());
    }

    #[test]
    fn accepts_http_server_origins() {
        for server in [
            "http://127.0.0.1:3746",
            "http://100.100.20.30:3746",
            "http://sync.private.example:3746",
        ] {
            for kind in [Kind::Device, Kind::Setup] {
                let text = encode(kind, server, &secret(kind)).unwrap();
                assert_eq!(decode(&text).unwrap().server, server);
            }
        }
    }

    #[test]
    fn refuses_servers_encrypted_sync_cannot_use() {
        for server in [
            "ftp://sync.example.net",
            "https://sync.example.net/x",
            "https://sync.example.net/",
        ] {
            assert!(encode(Kind::Device, server, &secret(Kind::Device)).is_err());
            let text = device_text(&payload(1, 2, server, &secret(Kind::Device)));
            assert!(decode(&text).is_none(), "{server}");
            let text = setup_text(&payload(1, 1, server, &secret(Kind::Setup)));
            assert!(decode(&text).is_none(), "{server}");
        }
        let longest = format!("https://{}", "a".repeat(MAX_SERVER_BYTES - 8));
        for kind in [Kind::Device, Kind::Setup] {
            let text = encode(kind, &longest, &secret(kind)).unwrap();
            assert_eq!(decode(&text).unwrap().server, longest);
        }
        let error =
            encode(Kind::Device, &format!("{longest}a"), &secret(Kind::Device)).unwrap_err();
        assert!(format!("{error:#}").contains("error sync-server-url-too-long"));
    }
}
