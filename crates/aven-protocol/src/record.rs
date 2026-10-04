//! Exact encrypted-record framing and descriptor-bound headers.

use crate::{codec, context::LocalSharedStatePackageContext};
use anyhow::{Context, Result, ensure};

pub const CHUNK_HEADER_BYTES: usize = 198;
pub const CHUNK_RECORD_OVERHEAD: usize = 222;

#[allow(clippy::too_many_arguments)]
pub fn chunk_header(
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    artifact_id: [u8; 32],
    family: u8,
    class: u8,
    index: u32,
    count: u32,
    total: u64,
    nonce: [u8; 24],
) -> Result<Vec<u8>> {
    let mut header = Vec::with_capacity(CHUNK_HEADER_BYTES);
    header.extend_from_slice(b"AVEN");
    header.push(2);
    header.extend_from_slice(&1_u16.to_be_bytes());
    header.push(1);
    codec::bytes(&mut header, &context.vault_id);
    codec::bytes(&mut header, &stream_id);
    codec::bytes(&mut header, &context.generation_id);
    codec::bytes(&mut header, &artifact_id);
    header.push(family);
    header.push(class);
    header.extend_from_slice(&index.to_be_bytes());
    header.extend_from_slice(&count.to_be_bytes());
    header.extend_from_slice(&total.to_be_bytes());
    codec::bytes(&mut header, &nonce);
    ensure!(header.len() == CHUNK_HEADER_BYTES, "invalid chunk header");
    Ok(header)
}

#[allow(clippy::too_many_arguments)]
pub fn validate_chunk_header(
    header: &[u8],
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    artifact_id: [u8; 32],
    family: u8,
    class: u8,
    index: u32,
    count: u32,
    total: u64,
) -> Result<[u8; 24]> {
    ensure!(
        header.len() == CHUNK_HEADER_BYTES,
        "error encrypted-local-shared-package-header-size"
    );
    let nonce_offset = CHUNK_HEADER_BYTES - 28;
    ensure!(
        u32::from_be_bytes(header[nonce_offset..nonce_offset + 4].try_into()?) == 24,
        "error encrypted-local-shared-package-nonce-size"
    );
    let nonce: [u8; 24] = header[nonce_offset + 4..].try_into()?;
    let expected = chunk_header(
        context,
        stream_id,
        artifact_id,
        family,
        class,
        index,
        count,
        total,
        nonce,
    )?;
    ensure!(
        header == expected,
        "error encrypted-local-shared-package-header-mismatch"
    );
    Ok(nonce)
}

pub fn split_record(record: &[u8]) -> Result<(&[u8], &[u8])> {
    ensure!(
        record.len() >= CHUNK_RECORD_OVERHEAD,
        "error encrypted-local-shared-package-record-truncated"
    );
    let header_len = usize::try_from(u32::from_be_bytes(record[0..4].try_into()?))?;
    ensure!(
        header_len == CHUNK_HEADER_BYTES,
        "error encrypted-local-shared-package-header-size"
    );
    let body_len_offset = 4_usize
        .checked_add(header_len)
        .context("record offset overflow")?;
    let body_start = body_len_offset
        .checked_add(4)
        .context("record offset overflow")?;
    ensure!(
        record.len() >= body_start,
        "error encrypted-local-shared-package-record-truncated"
    );
    let body_len = usize::try_from(u32::from_be_bytes(
        record[body_len_offset..body_start].try_into()?,
    ))?;
    ensure!(
        body_len >= 16 && body_start.checked_add(body_len) == Some(record.len()),
        "error encrypted-local-shared-package-record-length"
    );
    Ok((&record[4..body_len_offset], &record[body_start..]))
}
