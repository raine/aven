use std::process::Command;

use rand::Rng;

pub(crate) const BASE32: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub(crate) fn now() -> String {
    let output = Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%dT%H:%M:%SZ")
        .output();
    output
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

pub(crate) fn new_id() -> String {
    let mut bytes = [0u8; 10];
    rand::rng().fill_bytes(&mut bytes);
    encode_crockford(&bytes)
}

pub(crate) fn encode_crockford(bytes: &[u8; 10]) -> String {
    let mut value = u128::from_be_bytes([
        0, 0, 0, 0, 0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
        bytes[7], bytes[8], bytes[9],
    ]);
    let mut chars = [b'0'; 16];
    for i in (0..16).rev() {
        chars[i] = BASE32[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(chars.to_vec()).expect("base32 is utf8")
}
