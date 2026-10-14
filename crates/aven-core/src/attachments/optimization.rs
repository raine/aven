use std::time::Duration;

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageOptimizationPolicy {
    Preserve,
    Optimize,
}

pub struct OptimizedBytes {
    pub bytes: Vec<u8>,
    pub optimized: bool,
}

pub async fn optimize_image_bytes(
    media_type: &str,
    bytes: Vec<u8>,
    policy: ImageOptimizationPolicy,
) -> Result<OptimizedBytes> {
    if policy == ImageOptimizationPolicy::Preserve || media_type != "image/png" {
        return Ok(OptimizedBytes {
            bytes,
            optimized: false,
        });
    }

    let optimized = crate::attachments::blocking::run(move || Ok(optimize_png(bytes))).await?;
    match optimized {
        Ok((original, optimized)) if optimized.len() < original.len() => Ok(OptimizedBytes {
            bytes: optimized,
            optimized: true,
        }),
        Ok((original, _)) | Err((original, _)) => Ok(OptimizedBytes {
            bytes: original,
            optimized: false,
        }),
    }
}

type PngOptimizationResult = std::result::Result<(Vec<u8>, Vec<u8>), (Vec<u8>, oxipng::PngError)>;

fn optimize_png(bytes: Vec<u8>) -> PngOptimizationResult {
    let mut options = oxipng::Options::from_preset(4);
    options.strip = oxipng::StripChunks::Safe;
    options.timeout = Some(Duration::from_secs(10));
    options.max_decompressed_size = Some(
        usize::try_from(crate::attachments::decode::MAX_DECODER_ALLOCATION).unwrap_or(usize::MAX),
    );
    match oxipng::optimize_from_memory(&bytes, &options) {
        Ok(optimized) => Ok((bytes, optimized)),
        Err(error) => Err((bytes, error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn optimization_preserves_pixels_and_respects_bypass_conditions() {
        use ImageOptimizationPolicy::{Optimize, Preserve};
        use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};

        let pixels = vec![255; 64 * 64 * 4];
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&pixels, 64, 64, ExtendedColorType::Rgba8)
            .unwrap();
        let optimized = optimize_image_bytes("image/png", bytes.clone(), Optimize)
            .await
            .unwrap();
        assert!(optimized.optimized);
        assert!(optimized.bytes.len() < bytes.len());
        assert_eq!(
            image::load_from_memory(&optimized.bytes)
                .unwrap()
                .to_rgba8()
                .into_raw(),
            pixels
        );

        // The declared media type and policy govern whether bytes are inspected.
        for (media_type, input, policy) in [
            ("image/png", bytes.as_slice(), Preserve),
            ("image/gif", bytes.as_slice(), Optimize),
            ("image/png", b"not a png".as_slice(), Optimize),
        ] {
            let result = optimize_image_bytes(media_type, input.to_vec(), policy)
                .await
                .unwrap();
            assert_eq!(result.bytes, input, "{media_type} {policy:?}");
            assert!(!result.optimized, "{media_type} {policy:?}");
        }
    }
}
