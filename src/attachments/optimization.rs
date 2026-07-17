use std::time::Duration;

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageOptimizationPolicy {
    Preserve,
    Optimize,
}

pub(crate) struct OptimizedBytes {
    pub(crate) bytes: Vec<u8>,
    pub(crate) optimized: bool,
}

pub(crate) async fn optimize_image_bytes(
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
    async fn preserve_policy_keeps_bytes() {
        let bytes = b"png bytes".to_vec();
        let optimized = optimize_image_bytes(
            "image/png",
            bytes.clone(),
            ImageOptimizationPolicy::Preserve,
        )
        .await
        .unwrap();

        assert_eq!(optimized.bytes, bytes);
        assert!(!optimized.optimized);
    }

    #[tokio::test]
    async fn non_png_keeps_bytes() {
        let bytes = b"gif bytes".to_vec();
        let optimized = optimize_image_bytes(
            "image/gif",
            bytes.clone(),
            ImageOptimizationPolicy::Optimize,
        )
        .await
        .unwrap();

        assert_eq!(optimized.bytes, bytes);
        assert!(!optimized.optimized);
    }

    #[tokio::test]
    async fn invalid_png_keeps_bytes() {
        let bytes = b"not a png".to_vec();
        let optimized = optimize_image_bytes(
            "image/png",
            bytes.clone(),
            ImageOptimizationPolicy::Optimize,
        )
        .await
        .unwrap();

        assert_eq!(optimized.bytes, bytes);
        assert!(!optimized.optimized);
    }
}
