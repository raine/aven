#![allow(dead_code)]

use anyhow::{Result, bail};

pub(crate) const ATTACHMENT_REF_PREFIX: &str = "aven-attachment:";
pub(crate) const MAX_BLOB_BYTES: usize = 25 * 1024 * 1024;
pub(crate) const MAX_FILENAME_LEN: usize = 255;
pub(crate) const MAX_ALT_TEXT_LEN: usize = 500;
pub(crate) const SUPPORTED_MEDIA_TYPES: &[&str] =
    &["image/png", "image/jpeg", "image/gif", "image/webp"];

pub(crate) fn validate_attachment_id(value: &str) -> Result<()> {
    if value.len() != 16 || !value.bytes().all(|byte| crate::ids::BASE32.contains(&byte)) {
        bail!("error invalid-attachment-id input={value}");
    }
    Ok(())
}

pub(crate) fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        bail!("error invalid-sha256 input={value}");
    }
    Ok(())
}

pub(crate) fn validate_media_type(value: &str) -> Result<()> {
    if !SUPPORTED_MEDIA_TYPES.contains(&value) {
        bail!("error unsupported-attachment-media-type input={value}");
    }
    Ok(())
}

pub(crate) fn validate_blob_size(byte_size: usize) -> Result<()> {
    if byte_size == 0 || byte_size > MAX_BLOB_BYTES {
        bail!("error invalid-attachment-size bytes={byte_size}");
    }
    Ok(())
}

pub(crate) fn validate_filename(value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty()
        || value.len() > MAX_FILENAME_LEN
        || value
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
    {
        bail!("error invalid-attachment-filename");
    }
    Ok(())
}

pub(crate) fn validate_alt_text(value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() > MAX_ALT_TEXT_LEN || value.chars().any(char::is_control) {
        bail!("error invalid-attachment-alt-text");
    }
    Ok(())
}

pub(crate) fn validate_dimensions(width: Option<i64>, height: Option<i64>) -> Result<()> {
    match (width, height) {
        (None, None) => Ok(()),
        (Some(width), Some(height)) if width > 0 && height > 0 => Ok(()),
        _ => bail!("error invalid-attachment-dimensions"),
    }
}

pub(crate) fn parse_attachment_ref(value: &str) -> Option<&str> {
    value
        .strip_prefix(ATTACHMENT_REF_PREFIX)
        .filter(|id| validate_attachment_id(id).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_attachment_id() {
        assert!(validate_attachment_id("7KQ9A1X4MV2P8D6R").is_ok());
        assert!(validate_attachment_id("short").is_err());
        assert!(validate_attachment_id("7KQ9A1X4MV2P8D6U").is_err()); // 'U' is not base32
        assert!(validate_attachment_id("7KQ9A1X4MV2P8D6r").is_err()); // lowercase not allowed
    }

    #[test]
    fn validates_sha256() {
        let valid = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        assert!(validate_sha256(valid).is_ok());
        assert!(validate_sha256("short").is_err());
        assert!(validate_sha256(&format!("{valid}g")).is_err());
        assert!(validate_sha256(&valid.to_ascii_uppercase()).is_err());
    }

    #[test]
    fn validates_media_type() {
        assert!(validate_media_type("image/png").is_ok());
        assert!(validate_media_type("image/jpeg").is_ok());
        assert!(validate_media_type("image/gif").is_ok());
        assert!(validate_media_type("image/webp").is_ok());
        assert!(validate_media_type("image/svg+xml").is_err());
        assert!(validate_media_type("").is_err());
    }

    #[test]
    fn validates_blob_size() {
        assert!(validate_blob_size(1).is_ok());
        assert!(validate_blob_size(25 * 1024 * 1024).is_ok());
        assert!(validate_blob_size(0).is_err());
        assert!(validate_blob_size(25 * 1024 * 1024 + 1).is_err());
    }

    #[test]
    fn validates_filename() {
        assert!(validate_filename(Some("photo.png")).is_ok());
        assert!(validate_filename(None).is_ok());
        assert!(validate_filename(Some("")).is_err());
        assert!(validate_filename(Some("a/photo.png")).is_err());
        assert!(validate_filename(Some("a\\photo.png")).is_err());
        let long = "a".repeat(256);
        assert!(validate_filename(Some(&long)).is_err());
    }

    #[test]
    fn validates_alt_text() {
        assert!(validate_alt_text(Some("a photo of a cat")).is_ok());
        assert!(validate_alt_text(None).is_ok());
        assert!(validate_alt_text(Some("")).is_ok());
        let long = "a".repeat(501);
        assert!(validate_alt_text(Some(&long)).is_err());
        assert!(validate_alt_text(Some("a\nb")).is_err());
    }

    #[test]
    fn validates_dimensions() {
        assert!(validate_dimensions(None, None).is_ok());
        assert!(validate_dimensions(Some(1920), Some(1080)).is_ok());
        assert!(validate_dimensions(Some(0), Some(1080)).is_err());
        assert!(validate_dimensions(Some(1920), None).is_err());
        assert!(validate_dimensions(None, Some(1080)).is_err());
    }

    #[test]
    fn parses_attachment_ref() {
        assert_eq!(
            parse_attachment_ref("aven-attachment:7KQ9A1X4MV2P8D6R"),
            Some("7KQ9A1X4MV2P8D6R")
        );
        assert_eq!(parse_attachment_ref("aven-attachment:short"), None);
        assert_eq!(parse_attachment_ref("not-a-ref"), None);
        assert_eq!(parse_attachment_ref(""), None);
    }
}
