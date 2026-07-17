use std::io::Cursor;

use anyhow::{Context, Result, bail};
use image::codecs::gif::GifDecoder;
use image::codecs::jpeg::JpegDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, Limits, RgbaImage};

use super::blocking;
use super::validation::{validate_blob_size, validate_media_type};

pub(crate) const MAX_IMAGE_EDGE: u32 = 16_384;
pub(crate) const MAX_FRAME_PIXELS: u64 = 40_000_000;
pub(crate) const MAX_ANIMATION_FRAMES: usize = 100;
pub(crate) const MAX_ANIMATED_PIXELS: u64 = 100_000_000;
pub(crate) const MAX_DECODER_ALLOCATION: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageFacts {
    pub(crate) media_type: String,
    pub(crate) width: i64,
    pub(crate) height: i64,
    pub(crate) frame_count: usize,
}

#[derive(Debug)]
pub(crate) struct ValidatedImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) facts: ImageFacts,
}

pub(crate) async fn validate_image(
    bytes: Vec<u8>,
    declared_media_type: Option<String>,
) -> Result<ValidatedImage> {
    blocking::run(move || validate_image_blocking(bytes, declared_media_type.as_deref())).await
}

pub(crate) fn validate_image_blocking(
    bytes: Vec<u8>,
    declared_media_type: Option<&str>,
) -> Result<ValidatedImage> {
    let (facts, _) = decode(&bytes, declared_media_type, false)?;
    Ok(ValidatedImage { bytes, facts })
}

pub(crate) fn decode_first_frame(bytes: &[u8]) -> Result<RgbaImage> {
    decode(bytes, None, true)?
        .1
        .ok_or_else(|| anyhow::anyhow!("error empty-attachment-image"))
}

fn decode(
    bytes: &[u8],
    declared_media_type: Option<&str>,
    capture_first_frame: bool,
) -> Result<(ImageFacts, Option<RgbaImage>)> {
    validate_blob_size(bytes.len())?;
    if let Some(media_type) = declared_media_type {
        validate_media_type(media_type)?;
    }
    let format = image::guess_format(bytes).context("error invalid-attachment-image")?;
    let media_type = media_type_for_format(format)
        .ok_or_else(|| anyhow::anyhow!("error unsupported-attachment-image-format"))?;
    if declared_media_type.is_some_and(|declared| declared != media_type) {
        bail!(
            "error attachment-media-type-mismatch declared={} detected={media_type}",
            declared_media_type.unwrap_or_default()
        );
    }

    let limits = image_limits();
    let ((width, height), frame_count, first_frame) = match format {
        ImageFormat::Png => {
            let decoder = PngDecoder::with_limits(Cursor::new(bytes), limits)?;
            let dimensions = decoder.dimensions();
            validate_frame_dimensions(dimensions.0, dimensions.1)?;
            if decoder.is_apng()? {
                let (count, first) = validate_frames(
                    decoder.apng()?.into_frames(),
                    dimensions,
                    capture_first_frame,
                )?;
                (dimensions, count, first)
            } else {
                let image = DynamicImage::from_decoder(decoder)
                    .context("error invalid-attachment-image")?;
                (
                    dimensions,
                    1,
                    capture_first_frame.then(|| image.into_rgba8()),
                )
            }
        }
        ImageFormat::Jpeg => {
            let mut decoder = JpegDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(limits)?;
            let dimensions = decoder.dimensions();
            validate_frame_dimensions(dimensions.0, dimensions.1)?;
            let image =
                DynamicImage::from_decoder(decoder).context("error invalid-attachment-image")?;
            (
                dimensions,
                1,
                capture_first_frame.then(|| image.into_rgba8()),
            )
        }
        ImageFormat::Gif => {
            let mut decoder = GifDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(limits)?;
            let dimensions = decoder.dimensions();
            validate_frame_dimensions(dimensions.0, dimensions.1)?;
            let (count, first) =
                validate_frames(decoder.into_frames(), dimensions, capture_first_frame)?;
            (dimensions, count, first)
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(limits)?;
            let dimensions = decoder.dimensions();
            validate_frame_dimensions(dimensions.0, dimensions.1)?;
            if decoder.has_animation() {
                let (count, first) =
                    validate_frames(decoder.into_frames(), dimensions, capture_first_frame)?;
                (dimensions, count, first)
            } else {
                let image = DynamicImage::from_decoder(decoder)
                    .context("error invalid-attachment-image")?;
                (
                    dimensions,
                    1,
                    capture_first_frame.then(|| image.into_rgba8()),
                )
            }
        }
        _ => bail!("error unsupported-attachment-image-format"),
    };

    if frame_count == 0 {
        bail!("error empty-attachment-image");
    }
    Ok((
        ImageFacts {
            media_type: media_type.to_string(),
            width: i64::from(width),
            height: i64::from(height),
            frame_count,
        },
        first_frame,
    ))
}

fn validate_frames(
    frames: image::Frames<'_>,
    dimensions: (u32, u32),
    capture_first_frame: bool,
) -> Result<(usize, Option<RgbaImage>)> {
    let mut frame_count = 0_usize;
    let mut cumulative_pixels = 0_u64;
    let mut first_frame = None;
    for frame in frames {
        let frame = frame.context("error invalid-attachment-image")?;
        frame_count += 1;
        if frame_count > MAX_ANIMATION_FRAMES {
            bail!("error attachment-animation-frame-limit");
        }
        let frame_dimensions = frame.buffer().dimensions();
        if frame_dimensions != dimensions {
            bail!("error invalid-attachment-frame-dimensions");
        }
        validate_frame_dimensions(frame_dimensions.0, frame_dimensions.1)?;
        let pixels = u64::from(frame_dimensions.0) * u64::from(frame_dimensions.1);
        cumulative_pixels = cumulative_pixels
            .checked_add(pixels)
            .ok_or_else(|| anyhow::anyhow!("error attachment-animation-pixel-limit"))?;
        if cumulative_pixels > MAX_ANIMATED_PIXELS {
            bail!("error attachment-animation-pixel-limit");
        }
        if capture_first_frame && first_frame.is_none() {
            first_frame = Some(frame.into_buffer());
        }
    }
    Ok((frame_count, first_frame))
}

fn image_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_EDGE);
    limits.max_image_height = Some(MAX_IMAGE_EDGE);
    limits.max_alloc = Some(MAX_DECODER_ALLOCATION);
    limits
}

fn validate_frame_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_IMAGE_EDGE || height > MAX_IMAGE_EDGE {
        bail!("error attachment-image-dimension-limit");
    }
    if u64::from(width) * u64::from(height) > MAX_FRAME_PIXELS {
        bail!("error attachment-image-pixel-limit");
    }
    Ok(())
}

fn media_type_for_format(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::codecs::gif::GifEncoder;
    use image::{DynamicImage, Frame, ImageFormat, Rgba, RgbaImage};

    use super::*;

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255])));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn detects_supported_formats_and_dimensions() {
        for (format, media_type) in [
            (ImageFormat::Png, "image/png"),
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::Gif, "image/gif"),
            (ImageFormat::WebP, "image/webp"),
        ] {
            let image = validate_image_blocking(encoded(format), None).unwrap();
            assert_eq!(image.facts.media_type, media_type);
            assert_eq!((image.facts.width, image.facts.height), (3, 2));
        }
    }

    #[test]
    fn rejects_declared_media_type_mismatch() {
        let error = validate_image_blocking(encoded(ImageFormat::Png), Some("image/jpeg"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("attachment-media-type-mismatch"));
    }

    #[test]
    fn rejects_excessive_animation_frames() {
        let frames = (0..=MAX_ANIMATION_FRAMES)
            .map(|_| Frame::new(RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))));
        let mut bytes = Vec::new();
        GifEncoder::new(&mut bytes).encode_frames(frames).unwrap();
        let error = validate_image_blocking(bytes, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("attachment-animation-frame-limit"));
    }

    #[test]
    fn rejects_malformed_and_unsupported_images() {
        assert!(validate_image_blocking(b"not an image".to_vec(), None).is_err());
        assert!(validate_image_blocking(b"BMunsupported".to_vec(), None).is_err());
    }
}
