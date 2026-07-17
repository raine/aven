use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};

use super::blocking;
use super::decode::{decode_first_frame, validate_image_blocking};
use super::storage::{object_path, sha256_hex};
use super::validation::validate_sha256;

pub(crate) const PREVIEW_LONG_EDGE: u32 = 1_200;
pub(crate) const MAX_PREVIEW_PIXELS: u64 = 1_000_000;
pub(crate) const MAX_PREVIEW_BYTES: usize = 5 * 1024 * 1024;
pub(crate) const PREVIEW_PROFILE: &str = "v1-edge1200-pixels1000000-png";

pub(crate) fn cached_preview_path(blob_dir: &Path, source_hash: &str) -> Result<PathBuf> {
    validate_sha256(source_hash)?;
    Ok(blob_dir
        .join("cache")
        .join("previews")
        .join(PREVIEW_PROFILE)
        .join(format!("{source_hash}.png")))
}

#[cfg(test)]
pub(crate) async fn ensure_preview(blob_dir: &Path, source_hash: &str) -> Result<PathBuf> {
    let blob_dir = blob_dir.to_path_buf();
    let source_hash = source_hash.to_string();
    blocking::run(move || ensure_preview_blocking(&blob_dir, &source_hash)).await
}

pub(crate) async fn load_preview_png(blob_dir: &Path, source_hash: &str) -> Result<Vec<u8>> {
    let blob_dir = blob_dir.to_path_buf();
    let source_hash = source_hash.to_string();
    blocking::run(move || {
        let path = ensure_preview_blocking(&blob_dir, &source_hash)?;
        read_validated_preview(&path)
    })
    .await
}

fn ensure_preview_blocking(blob_dir: &Path, source_hash: &str) -> Result<PathBuf> {
    let preview = cached_preview_path(blob_dir, source_hash)?;
    if preview.exists() && validate_cached_preview(&preview).is_ok() {
        return Ok(preview);
    }

    let source = object_path(blob_dir, source_hash)?;
    let source_bytes = fs::read(&source)
        .with_context(|| format!("could not read attachment object {}", source.display()))?;
    if sha256_hex(&source_bytes) != source_hash {
        bail!("error attachment-preview-source-hash-mismatch");
    }
    let first_frame = decode_first_frame(&source_bytes)?;
    let (target_width, target_height) =
        preview_dimensions(first_frame.width(), first_frame.height());
    let resized = image::imageops::resize(
        &first_frame,
        target_width,
        target_height,
        FilterType::Lanczos3,
    );
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(resized).write_to(&mut encoded, ImageFormat::Png)?;
    let encoded = encoded.into_inner();
    if encoded.len() > MAX_PREVIEW_BYTES {
        bail!("error attachment-preview-byte-limit");
    }
    let validated = validate_image_blocking(encoded, Some("image/png"))?;
    if u64::try_from(validated.facts.width).unwrap_or(u64::MAX)
        * u64::try_from(validated.facts.height).unwrap_or(u64::MAX)
        > MAX_PREVIEW_PIXELS
    {
        bail!("error attachment-preview-pixel-limit");
    }

    let parent = preview
        .parent()
        .ok_or_else(|| anyhow::anyhow!("preview path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create preview in {}", parent.display()))?;
    std::io::Write::write_all(&mut temp, &validated.bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(&preview)
        .map_err(|error| error.error)
        .with_context(|| format!("could not replace {}", preview.display()))?;
    Ok(preview)
}

fn validate_cached_preview(path: &Path) -> Result<()> {
    read_validated_preview(path).map(|_| ())
}

fn read_validated_preview(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    if bytes.len() > MAX_PREVIEW_BYTES {
        bail!("error attachment-preview-byte-limit");
    }
    let validated = validate_image_blocking(bytes, Some("image/png"))?;
    let width = u64::try_from(validated.facts.width).unwrap_or(u64::MAX);
    let height = u64::try_from(validated.facts.height).unwrap_or(u64::MAX);
    if width > u64::from(PREVIEW_LONG_EDGE)
        || height > u64::from(PREVIEW_LONG_EDGE)
        || width.saturating_mul(height) > MAX_PREVIEW_PIXELS
    {
        bail!("error attachment-preview-dimension-limit");
    }
    Ok(validated.bytes)
}

fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    let long_scale = f64::from(PREVIEW_LONG_EDGE) / f64::from(width.max(height));
    let pixel_scale = (MAX_PREVIEW_PIXELS as f64 / (f64::from(width) * f64::from(height))).sqrt();
    let scale = long_scale.min(pixel_scale).min(1.0);
    let mut target_width = (f64::from(width) * scale).floor().max(1.0) as u32;
    let mut target_height = (f64::from(height) * scale).floor().max(1.0) as u32;
    while u64::from(target_width) * u64::from(target_height) > MAX_PREVIEW_PIXELS {
        if target_width >= target_height {
            target_width -= 1;
        } else {
            target_height -= 1;
        }
    }
    (target_width, target_height)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, RgbaImage};

    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::new(width, height))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[tokio::test]
    async fn creates_repeatable_disposable_preview() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = png(2_000, 1_000);
        let hash = sha256_hex(&bytes);
        let source = object_path(temp.path(), &hash).unwrap();
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, bytes).unwrap();

        let (first, concurrent) = tokio::join!(
            ensure_preview(temp.path(), &hash),
            ensure_preview(temp.path(), &hash)
        );
        let first = first.unwrap();
        assert_eq!(first, concurrent.unwrap());
        let first_bytes = fs::read(&first).unwrap();
        let loaded = load_preview_png(temp.path(), &hash).await.unwrap();
        assert_eq!(loaded, first_bytes);
        assert_ne!(loaded, fs::read(&source).unwrap());
        let decoded = image::load_from_memory_with_format(&loaded, ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1_200, 600));
        let second = ensure_preview(temp.path(), &hash).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first_bytes, fs::read(second).unwrap());

        fs::write(&first, b"partial preview").unwrap();
        let regenerated = ensure_preview(temp.path(), &hash).await.unwrap();
        assert_eq!(first_bytes, fs::read(regenerated).unwrap());
    }

    #[tokio::test]
    async fn missing_source_fails_without_creating_cache_entry() {
        let temp = tempfile::tempdir().unwrap();
        let hash = "0".repeat(64);

        assert!(load_preview_png(temp.path(), &hash).await.is_err());
        assert!(!cached_preview_path(temp.path(), &hash).unwrap().exists());
    }

    #[test]
    fn bounds_preview_dimensions() {
        assert_eq!(preview_dimensions(2_000, 1_000), (1_200, 600));
        let (width, height) = preview_dimensions(16_000, 16_000);
        assert!(u64::from(width) * u64::from(height) <= MAX_PREVIEW_PIXELS);
        assert!(width <= PREVIEW_LONG_EDGE && height <= PREVIEW_LONG_EDGE);
    }
}
