pub(crate) use aven_core::attachments::*;

pub(crate) mod blocking {
    pub(crate) use aven_core::attachments::{run_blocking as run, run_preview};
}

pub(crate) mod decode {
    pub(crate) use aven_core::attachments::{decode_first_frame, validate_image_blocking};
}

pub(crate) mod lifecycle {
    pub(crate) use aven_core::attachments::{
        DEFAULT_MAINTENANCE_LIMIT, DEFAULT_ORIGINAL_QUOTA_BYTES, DEFAULT_PREVIEW_QUOTA_BYTES,
        LifecyclePolicy, prune_preview_cache,
    };
}

pub(crate) mod optimization {
    pub(crate) use aven_core::attachments::ImageOptimizationPolicy;
}

pub(crate) mod export;

pub(crate) mod storage {
    pub(crate) use aven_core::attachments::{object_path, sha256_hex};
}

pub(crate) mod validation {
    pub(crate) use aven_core::attachments::validate_sha256;
}

pub(crate) mod preview;
