pub(crate) mod optimization;
pub(crate) mod storage;
pub(crate) mod validation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachmentBytesState {
    Present,
    PendingDownload,
    Unavailable,
}

#[allow(unused_imports)]
pub(crate) use optimization::{ImageOptimizationPolicy, OptimizedBytes, optimize_image_bytes};
pub(crate) use storage::{blob_inventory_row, object_path, sha256_hex, store_blob};
#[allow(unused_imports)]
pub(crate) use validation::{
    MAX_ALT_TEXT_LEN, MAX_BLOB_BYTES, MAX_FILENAME_LEN, SUPPORTED_MEDIA_TYPES, validate_alt_text,
    validate_attachment_id, validate_blob_size, validate_dimensions, validate_filename,
    validate_media_type, validate_sha256,
};
