//! Exact workspace identifier validation for ordinary image requests and moves.

/// A workspace identifier is not sixteen uppercase Crockford Base32 characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidWorkspaceId;

impl std::fmt::Display for InvalidWorkspaceId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("workspace ID must be 16 Crockford Base32 characters")
    }
}

impl std::error::Error for InvalidWorkspaceId {}

/// Validate without normalization or allocation. Identifiers locate workspace
/// storage; valid syntax does not authorize access.
pub fn validate(value: &str) -> Result<(), InvalidWorkspaceId> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    if value.len() == 16 && value.bytes().all(|byte| ALPHABET.contains(&byte)) {
        Ok(())
    } else {
        Err(InvalidWorkspaceId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_alphabet_length_and_unicode_acceptance() {
        for valid in ["0123456789ABCDEF", "GHJKMNPQRSTVWXYZ", "0000000000000000"] {
            validate(valid).unwrap();
        }
        for byte in 0..=127_u8 {
            let mut id = [b'0'; 16];
            id[7] = byte;
            let id = std::str::from_utf8(&id).unwrap();
            let expected = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte);
            assert_eq!(validate(id).is_ok(), expected, "byte {byte}");
        }
        for invalid in [
            "",
            "0123456789ABCDE",
            "0123456789ABCDEFG",
            "0123456789abcdef",
            "0123456789ABCDEI",
            "0123456789ABCDEL",
            "0123456789ABCDEO",
            "0123456789ABCDEU",
            "00000000000000é",
            "000000000000000é",
            "０００００００００００００００００",
            "000000000000000\0",
        ] {
            assert_eq!(validate(invalid), Err(InvalidWorkspaceId), "{invalid:?}");
        }
        assert_eq!(
            InvalidWorkspaceId.to_string(),
            "workspace ID must be 16 Crockford Base32 characters"
        );
    }
}
