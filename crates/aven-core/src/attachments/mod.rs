pub(crate) mod blocking;
pub(crate) mod decode;
pub(crate) mod lifecycle;
pub(crate) mod optimization;
pub(crate) mod storage;
pub(crate) mod validation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentBytesState {
    Present,
    PendingDownload,
    Unavailable,
}

pub use blocking::{run as run_blocking, run_preview};
pub use decode::{decode_first_frame, validate_image_blocking};
pub use lifecycle::{
    ByteCount, DEFAULT_MAINTENANCE_LIMIT, DEFAULT_ORIGINAL_QUOTA_BYTES,
    DEFAULT_PREVIEW_QUOTA_BYTES, LifecyclePolicy, LifecycleReport, PruneSummary,
    prune_preview_cache,
};
pub use optimization::{ImageOptimizationPolicy, OptimizedBytes, optimize_image_bytes};
pub use storage::{object_path, sha256_hex};
pub use validation::{
    MAX_ALT_TEXT_LEN, MAX_BLOB_BYTES, MAX_FILENAME_LEN, SUPPORTED_MEDIA_TYPES, validate_alt_text,
    validate_attachment_id, validate_blob_size, validate_dimensions, validate_filename,
    validate_media_type, validate_sha256,
};

pub fn default_blob_dir(database_path: &std::path::Path) -> std::path::PathBuf {
    let mut value = database_path.as_os_str().to_os_string();
    value.push(".blobs");
    std::path::PathBuf::from(value)
}
