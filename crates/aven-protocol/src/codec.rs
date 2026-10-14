//! Canonical framing shared by the sync formats. Domain-separated inputs
//! must stay byte-identical across every format that uses them.
use sha2::{Digest, Sha256};

/// Appends `value` with its u32 big-endian length.
pub fn bytes(out: &mut Vec<u8>, value: &[u8]) {
    // Every framed field is bounded far below 4 GiB.
    out.extend_from_slice(
        &u32::try_from(value.len())
            .expect("framed field fits u32")
            .to_be_bytes(),
    );
    out.extend_from_slice(value);
}

/// Canonical concatenation: the label, then each field, all length-framed.
pub fn cce(label: &str, fields: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    bytes(&mut out, label.as_bytes());
    for field in fields {
        bytes(&mut out, field);
    }
    out
}

pub fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

#[cfg(test)]
mod tests {
    #[test]
    fn canonical_fields_keep_their_length_prefixes() {
        assert_eq!(
            super::cce("x", &[b"yz", b""]),
            vec![0, 0, 0, 1, b'x', 0, 0, 0, 2, b'y', b'z', 0, 0, 0, 0],
        );
    }
}
