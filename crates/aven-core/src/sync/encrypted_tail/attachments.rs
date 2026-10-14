//! Opaque image representations, separate from local plaintext content addressing.
pub(crate) mod client;
pub(crate) mod codec;
pub(crate) mod server;
pub use aven_protocol::wire::images::{Operation, Reply, Status, Ticket};
pub use client::{Download, ImageSourceUnavailable, Upload};
pub use codec::{CHUNK_BYTES, HTTP_LIMIT, TRANSFER_BYTES};
