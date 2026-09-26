//! Device invitation text, shared by every client.
//!
//! An invitation is `AVEN:` followed by unpadded RFC 4648 base32 (`A`–`Z`,
//! `2`–`7`) of `U8(1) || U8(len(server)) || server || vault_id ||
//! inviter_hpke_public_key || psk`, where the last three fields are 32 bytes
//! each. Every character is in the QR alphanumeric set, so the QR code needs
//! no byte mode. Decoding ignores surrounding whitespace and letter case.
//! This module checks structure only; callers validate the server origin.
use std::fmt;

use anyhow::{Result, ensure};
use zeroize::Zeroizing;

pub const PREFIX: &str = "AVEN:";
const VERSION: u8 = 1;
/// Server origins longer than this don't fit the one-byte length.
pub const MAX_SERVER_BYTES: usize = 255;
/// Vault ID, inviter HPKE public key and PSK, in that order.
pub const SECRET_BYTES: usize = 96;
const MAX_PAYLOAD_BYTES: usize = 2 + MAX_SERVER_BYTES + SECRET_BYTES;
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A decoded invitation. Debug output is redacted.
pub struct DeviceInvitationText {
    pub server: String,
    pub secret: Zeroizing<[u8; SECRET_BYTES]>,
}

impl fmt::Debug for DeviceInvitationText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceInvitationText([REDACTED])")
    }
}

pub fn encode(server: &str, secret: &[u8; SECRET_BYTES]) -> Result<Zeroizing<String>> {
    ensure!(!server.is_empty(), "error sync-server-url-invalid");
    ensure!(
        server.len() <= MAX_SERVER_BYTES,
        "error sync-server-url-too-long hint=\"device invitations need a server origin of at most 255 bytes\""
    );
    let mut payload = Zeroizing::new(Vec::with_capacity(2 + server.len() + SECRET_BYTES));
    payload.push(VERSION);
    payload.push(server.len() as u8);
    payload.extend_from_slice(server.as_bytes());
    payload.extend_from_slice(secret);
    let mut text = Zeroizing::new(String::with_capacity(
        PREFIX.len() + payload.len() * 8 / 5 + 1,
    ));
    text.push_str(PREFIX);
    base32_encode(&payload, &mut text);
    Ok(text)
}

pub fn decode(text: &str) -> Option<DeviceInvitationText> {
    let text = Zeroizing::new(text.trim().to_ascii_uppercase());
    let encoded = text.strip_prefix(PREFIX)?;
    if encoded.len() > MAX_PAYLOAD_BYTES.div_ceil(5) * 8 {
        return None;
    }
    let payload = base32_decode(encoded)?;
    let (&version, rest) = payload.split_first()?;
    let (&server_len, rest) = rest.split_first()?;
    let server_len = usize::from(server_len);
    if version != VERSION || server_len == 0 || rest.len() != server_len + SECRET_BYTES {
        return None;
    }
    let (server, secret) = rest.split_at(server_len);
    Some(DeviceInvitationText {
        server: std::str::from_utf8(server).ok()?.to_string(),
        secret: Zeroizing::new(secret.try_into().ok()?),
    })
}

/// Whether `text` starts like a device invitation, in any letter case.
pub fn has_prefix(text: &str) -> bool {
    text.trim()
        .get(..PREFIX.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(PREFIX))
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

    fn secret() -> [u8; SECRET_BYTES] {
        let mut secret = [5; SECRET_BYTES];
        secret[32..64].fill(6);
        secret[64..].fill(7);
        secret
    }

    #[test]
    fn round_trips_with_the_documented_layout() {
        let text = encode(SERVER, &secret()).unwrap();
        let mut payload = vec![1, SERVER.len() as u8];
        payload.extend(SERVER.as_bytes());
        payload.extend(secret());
        let mut expected = PREFIX.to_string();
        base32_encode(&payload, &mut expected);
        assert_eq!(text.as_str(), expected);
        assert!(
            text.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b':')
        );

        let decoded = decode(&format!("  {}\n", text.as_str())).unwrap();
        assert_eq!(decoded.server, SERVER);
        assert_eq!(*decoded.secret, secret());
        assert_eq!(format!("{decoded:?}"), "DeviceInvitationText([REDACTED])");
    }

    #[test]
    fn accepts_lowercase_input() {
        let text = encode(SERVER, &secret()).unwrap();
        let decoded = decode(&text.to_ascii_lowercase()).unwrap();
        assert_eq!(decoded.server, SERVER);
        assert!(has_prefix(" aven:x"));
    }

    #[test]
    fn refuses_truncation_other_prefixes_and_bad_base32() {
        let text = encode(SERVER, &secret()).unwrap();
        for end in [
            PREFIX.len(),
            PREFIX.len() + 1,
            text.len() / 2,
            text.len() - 1,
        ] {
            assert!(decode(&text[..end]).is_none(), "{end}");
            assert!(has_prefix(&text[..end]));
        }
        assert!(decode(&text.replacen("AVEN:", "AVEX:", 1)).is_none());
        assert!(!has_prefix("aven-sync-setup-1:abc"));
        assert!(decode(&format!("{}A", text.as_str())).is_none());
        let body = &text[PREFIX.len()..];
        for bad in ["1", "8", "=", "-", " "] {
            let broken = format!("{PREFIX}{bad}{}", &body[1..]);
            assert!(decode(&broken).is_none(), "{bad}");
        }
        assert!(decode("AVEN:").is_none());
        // Nonzero trailing bits are not canonical.
        let last = text.chars().last().unwrap();
        let bumped = ALPHABET[(ALPHABET.iter().position(|&c| c as char == last).unwrap() + 1) % 32];
        assert!(decode(&format!("{}{}", &text[..text.len() - 1], bumped as char)).is_none());
    }

    #[test]
    fn refuses_other_versions_and_lengths() {
        let mut payload = vec![2, SERVER.len() as u8];
        payload.extend(SERVER.as_bytes());
        payload.extend(secret());
        let mut text = PREFIX.to_string();
        base32_encode(&payload, &mut text);
        assert!(decode(&text).is_none());

        payload[0] = 1;
        payload.push(0);
        let mut text = PREFIX.to_string();
        base32_encode(&payload, &mut text);
        assert!(decode(&text).is_none());
    }

    #[test]
    fn refuses_oversize_servers() {
        let server = format!("https://{}.example", "a".repeat(250));
        assert!(server.len() > MAX_SERVER_BYTES);
        let error = encode(&server, &secret()).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("error sync-server-url-too-long")
        );
        let longest = format!("https://{}", "a".repeat(MAX_SERVER_BYTES - 8));
        let text = encode(&longest, &secret()).unwrap();
        assert_eq!(decode(&text).unwrap().server, longest);
    }
}
