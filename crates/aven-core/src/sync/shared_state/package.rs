mod freeze;
pub mod publication;
pub mod upload;

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::NeverDispatchedLocalSharedCapture;
use super::validated::ValidatedSeed;
use crate::data_safety::export_types::AvenExport;
use crate::db::{self, Database};
use crate::sync::codec;

const CHUNK_PLAINTEXT_BYTES: usize = 1_048_576;
const CHUNK_HEADER_BYTES: usize = 198;
const CHUNK_RECORD_OVERHEAD: usize = 222;
const TAG_BYTES: usize = 16;
const MAX_STATE_CHUNKS: usize = 256;
const MAX_IMAGE_PLAINTEXT_BYTES: usize = crate::attachments::validation::MAX_BLOB_BYTES;
const MAX_PACKAGE_IMAGE_COUNT: usize = 1024;
const MAX_PACKAGE_IMAGE_PLAINTEXT_BYTES: usize = 256 * 1024 * 1024;
const IMAGE_FAMILY: u8 = 1;
const IMAGE_CLASS: u8 = 0;
const STATE_COMPONENT: &str = "state";
const MANIFEST_COMPONENT: &str = "manifest";
const IMAGE_COMPONENT: &str = "image";

pub use aven_protocol::context::{LocalSharedStatePackageContext, LocalSharedStatePackageKey};

/// Exact frozen upload components for the one never-dispatched local package.
///
/// The descriptor commits to the catalogs, and the catalogs commit to every
/// encrypted record, so this value has no separately stored metadata. Upload
/// components use the publication codec.
#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedLocalSharedStatePackage {
    candidate_id: String,
    upload: publication::Package,
}

impl fmt::Debug for EncryptedLocalSharedStatePackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedLocalSharedStatePackage")
            .field("candidate_id", &self.candidate_id)
            .field("state_records", &self.upload.state.len())
            .field("manifest_records", &self.upload.manifest.len())
            .field("image_count", &self.upload.images.len())
            .finish()
    }
}

impl EncryptedLocalSharedStatePackage {
    /// Copies the exact frozen descriptor, catalogs and encrypted records.
    /// Returning these bytes does not make this local package dispatchable.
    pub fn upload_package(&self) -> publication::Package {
        self.upload.clone()
    }

    #[cfg(test)]
    pub(crate) fn upload(&self) -> &publication::Package {
        &self.upload
    }

    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }
}

/// Encrypted records that are either freshly owned or borrowed from a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EncryptedArtifact<'a> {
    total_plaintext_bytes: u64,
    aggregate_commitment: [u8; 32],
    pub(crate) chunks: Vec<EncryptedChunk<'a>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EncryptedChunk<'a> {
    record_commitment: [u8; 32],
    pub(crate) record: std::borrow::Cow<'a, [u8]>,
}

impl EncryptedArtifact<'_> {
    /// Moves the records out without copying owned bytes.
    pub(crate) fn into_records(self) -> Vec<Vec<u8>> {
        self.chunks
            .into_iter()
            .map(|chunk| chunk.record.into_owned())
            .collect()
    }
}

/// One freshly encrypted selected image, before the package is assembled.
pub(super) struct EncryptedImage {
    pub(super) sha256: String,
    pub(super) object_id: [u8; 32],
    pub(super) artifact: EncryptedArtifact<'static>,
}

struct SelectedImagePlaintext {
    source_sha256: String,
    bytes: Zeroizing<Vec<u8>>,
}

impl Database {
    /// Reports frozen package ownership, including missing-byte corruption.
    /// A true result requires the host to load existing protected authority,
    /// never generate a replacement key.
    pub async fn has_local_shared_state_package_never_dispatched(&self) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_shared_capture_publication LIMIT 1)
                 OR EXISTS(SELECT 1 FROM local_shared_capture_journal
                           WHERE frozen_descriptor_commitment IS NOT NULL)",
        )
        .fetch_one(&mut *conn)
        .await?;
        Ok(exists != 0)
    }

    /// Encrypts the durable never-dispatched capture or returns its frozen package.
    ///
    /// The first successful call freezes the descriptor, catalogs and encrypted
    /// records in one SQLite transaction under the capture candidate ID.
    /// Tentative preparation before that commit can be retried. Later calls
    /// authenticate and return the exact saved bytes without reading image files.
    /// `membership_predecessor` is context only, not proof of authorization.
    /// Changed keys/context, incomplete data and incompatible local-only packages
    /// fail without replacement. Cancellation releases bytes and ownership together.
    /// SQLite commit/reopen is not a claim of platform power-loss durability.
    pub async fn package_local_shared_state_never_dispatched(
        &self,
        blob_dir: &Path,
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
        membership_predecessor: [u8; 32],
    ) -> Result<EncryptedLocalSharedStatePackage> {
        let proof = self
            .package_and_validate(blob_dir, context, key, membership_predecessor, None)
            .await
            .map_err(|error| {
                if error.to_string() == "error seed-freeze-unsupported" {
                    anyhow::anyhow!(
                        "error seed-freeze-unsupported hint=cancel-never-dispatched-capture-and-recapture"
                    )
                } else {
                    error
                }
            })?;
        let frozen = upload::FrozenUpload::new(
            proof.identity().candidate().to_string(),
            proof.descriptor().to_vec(),
            proof.metadata().catalogs.clone(),
        )?;
        Ok(EncryptedLocalSharedStatePackage {
            candidate_id: proof.identity().candidate().to_string(),
            upload: frozen.package(self).await?,
        })
    }

    /// Freezes the package and returns the proof made by its single keyed pass.
    ///
    /// `capture`, when given, is the value capture returned and stands in for
    /// reading the stored snapshot again. It is bound to the stored document
    /// by its digest before the freeze commits, so a stale or foreign value
    /// fails closed.
    pub(crate) async fn package_and_validate(
        &self,
        blob_dir: &Path,
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
        membership_predecessor: [u8; 32],
        capture: Option<NeverDispatchedLocalSharedCapture>,
    ) -> Result<ValidatedSeed> {
        freeze::freeze_and_validate(
            self,
            blob_dir,
            context,
            key,
            membership_predecessor,
            capture,
        )
        .await
    }

    /// Authenticates, decrypts, validates, and atomically installs a package.
    ///
    /// All package bytes are checked before the target database transaction is
    /// opened. Installation still enforces the existing empty-target boundary.
    pub async fn decrypt_and_install_local_shared_state_package(
        &self,
        package: &EncryptedLocalSharedStatePackage,
        key: &LocalSharedStatePackageKey,
    ) -> Result<super::SharedStateInstallReport> {
        let capture = publication::decrypt_capture(&package.upload, key)?;
        self.install_shared_state(&capture).await
    }
}

#[cfg(test)]
fn encrypt_package(
    capture: &NeverDispatchedLocalSharedCapture,
    context: LocalSharedStatePackageContext,
    selected_images: &[SelectedImagePlaintext],
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<(
    EncryptedLocalSharedStatePackage,
    publication::AttachmentIndex,
)> {
    let stream_id = decode_context_id(capture.stream_id(), "stream")?;
    let mut images = Vec::with_capacity(selected_images.len());
    for image in selected_images {
        let object_id = random_id()?;
        let image_key = derive_image_key(key, context, object_id)?;
        let artifact = encrypt_artifact(
            &image.bytes,
            context,
            stream_id,
            object_id,
            IMAGE_FAMILY,
            IMAGE_CLASS,
            &image_key,
        )?;
        images.push(EncryptedImage {
            sha256: image.source_sha256.clone(),
            object_id,
            artifact,
        });
    }
    let (upload, attachments) =
        publication::build(capture, context, images, key, membership_predecessor)?;
    Ok((
        EncryptedLocalSharedStatePackage {
            candidate_id: capture.candidate_id().to_string(),
            upload,
        },
        attachments,
    ))
}

fn derive_bootstrap_class_key(
    key: &LocalSharedStatePackageKey,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    candidate: [u8; 32],
    class: u8,
) -> Result<Zeroizing<[u8; 32]>> {
    let hkdf = Hkdf::<Sha256>::new(
        Some(b"aven-e2ee/v1/generation"),
        key.protected_storage_bytes().as_slice(),
    );
    let info = codec::cce(
        "aven-e2ee/v1/key/bootstrap-artifact",
        &[
            &context.vault_id,
            &context.generation_id,
            &stream_id,
            &candidate,
            &[class],
        ],
    );
    let mut output = [0_u8; 32];
    hkdf.expand(&info, &mut output)
        .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-key-derivation"))?;
    Ok(Zeroizing::new(output))
}

pub(crate) fn derive_image_key(
    key: &LocalSharedStatePackageKey,
    context: LocalSharedStatePackageContext,
    object_id: [u8; 32],
) -> Result<Zeroizing<[u8; 32]>> {
    let hkdf = Hkdf::<Sha256>::new(
        Some(b"aven-e2ee/v1/generation"),
        key.protected_storage_bytes().as_slice(),
    );
    let info = codec::cce(
        "aven-e2ee/v1/key/image-object",
        &[&context.vault_id, &context.generation_id, &object_id],
    );
    let mut output = [0_u8; 32];
    hkdf.expand(&info, &mut output)
        .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-key-derivation"))?;
    Ok(Zeroizing::new(output))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encrypt_artifact(
    plaintext: &[u8],
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    artifact_id: [u8; 32],
    family: u8,
    class: u8,
    key: &[u8; 32],
) -> Result<EncryptedArtifact<'static>> {
    let mut writer = ArtifactWriter::new(context, stream_id, artifact_id, family, class, key)?;
    writer.begin(plaintext.len())?;
    writer.write(plaintext)?;
    writer.finish()
}

/// Encrypts an artifact of known length as its plaintext arrives, directly
/// into its chunk records, so the whole plaintext never has to exist at once.
pub(crate) struct ArtifactWriter {
    cipher: XChaCha20Poly1305,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    artifact_id: [u8; 32],
    family: u8,
    class: u8,
    total: Option<u64>,
    count: usize,
    chunks: Vec<EncryptedChunk<'static>>,
    /// The open chunk: header and body length, then plaintext so far.
    record: Vec<u8>,
    body: usize,
    chunk_len: usize,
    nonce: [u8; 24],
}

impl ArtifactWriter {
    pub(crate) fn new(
        context: LocalSharedStatePackageContext,
        stream_id: [u8; 32],
        artifact_id: [u8; 32],
        family: u8,
        class: u8,
        key: &[u8; 32],
    ) -> Result<Self> {
        let cipher = XChaCha20Poly1305::new_from_slice(key)
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-invalid-key"))?;
        Ok(Self {
            cipher,
            context,
            stream_id,
            artifact_id,
            family,
            class,
            total: None,
            count: 0,
            chunks: Vec::new(),
            record: Vec::new(),
            body: 0,
            chunk_len: 0,
            nonce: [0; 24],
        })
    }

    /// Declares the plaintext length, once, before any plaintext.
    pub(crate) fn begin(&mut self, total: usize) -> Result<()> {
        ensure!(
            self.total.is_none(),
            "error encrypted-local-shared-package-artifact-length"
        );
        self.count = total.div_ceil(CHUNK_PLAINTEXT_BYTES).max(1);
        u32::try_from(self.count)?;
        self.chunks.reserve_exact(self.count);
        self.total = Some(u64::try_from(total)?);
        Ok(())
    }

    pub(crate) fn write(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            if self.record.is_empty() {
                self.start_chunk()?;
            }
            let room = self.chunk_len - (self.record.len() - self.body);
            let taken = room.min(bytes.len());
            self.record.extend_from_slice(&bytes[..taken]);
            bytes = &bytes[taken..];
            if self.record.len() - self.body == self.chunk_len {
                self.seal_chunk()?;
            }
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<EncryptedArtifact<'static>> {
        let total = self
            .total
            .context("error encrypted-local-shared-package-artifact-length")?;
        if total == 0 {
            self.start_chunk()?;
            self.seal_chunk()?;
        }
        ensure!(
            self.record.is_empty() && self.chunks.len() == self.count,
            "error encrypted-local-shared-package-artifact-length"
        );
        let chunks = std::mem::take(&mut self.chunks);
        Ok(EncryptedArtifact {
            total_plaintext_bytes: total,
            aggregate_commitment: aggregate_commitment(&chunks),
            chunks,
        })
    }

    fn start_chunk(&mut self) -> Result<()> {
        let total = self
            .total
            .context("error encrypted-local-shared-package-artifact-length")?;
        let index = self.chunks.len();
        ensure!(
            index < self.count,
            "error encrypted-local-shared-package-artifact-length"
        );
        self.chunk_len = expected_chunk_plaintext_len(index, self.count, usize::try_from(total)?)?;
        getrandom::fill(&mut self.nonce).context("error encrypted-local-shared-package-rng")?;
        let header = chunk_header(
            self.context,
            self.stream_id,
            self.artifact_id,
            self.family,
            self.class,
            u32::try_from(index)?,
            u32::try_from(self.count)?,
            total,
            self.nonce,
        )?;
        self.record = Vec::with_capacity(CHUNK_RECORD_OVERHEAD + self.chunk_len);
        codec::bytes(&mut self.record, &header);
        self.record
            .extend_from_slice(&u32::try_from(self.chunk_len + TAG_BYTES)?.to_be_bytes());
        self.body = self.record.len();
        Ok(())
    }

    /// Encrypts the open chunk in place and appends its tag.
    fn seal_chunk(&mut self) -> Result<()> {
        let nonce = XNonce::try_from(self.nonce.as_slice())
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-nonce-size"))?;
        let (head, body) = self.record.split_at_mut(self.body);
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce, &head[4..4 + CHUNK_HEADER_BYTES], body.into())
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-encryption"))?;
        let mut record = std::mem::take(&mut self.record);
        record.extend_from_slice(&tag);
        self.chunks.push(EncryptedChunk {
            record_commitment: codec::hash(&record),
            record: record.into(),
        });
        Ok(())
    }
}

impl Drop for ArtifactWriter {
    fn drop(&mut self) {
        // An unsealed chunk still holds plaintext.
        self.record.zeroize();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn decrypt_artifact(
    artifact: &EncryptedArtifact<'_>,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    artifact_id: [u8; 32],
    family: u8,
    class: u8,
    key: &[u8; 32],
    maximum_plaintext_bytes: usize,
) -> Result<Vec<u8>> {
    let mut reader = ArtifactReader::new(
        artifact,
        context,
        stream_id,
        artifact_id,
        family,
        class,
        key,
        maximum_plaintext_bytes,
    )?;
    let mut plaintext = Vec::with_capacity(usize::try_from(artifact.total_plaintext_bytes)?);
    loop {
        let piece = reader.take(usize::MAX)?;
        if piece.is_empty() {
            break;
        }
        plaintext.extend_from_slice(piece);
    }
    reader.finish()?;
    Ok(plaintext)
}

/// Authenticates and decrypts an artifact one chunk at a time, so at most one
/// chunk of plaintext exists at once. Every byte handed out has passed its
/// chunk's authentication, but only [`Self::finish`] establishes that the
/// artifact was complete.
pub(crate) struct ArtifactReader<'a> {
    artifact: &'a EncryptedArtifact<'a>,
    cipher: XChaCha20Poly1305,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    artifact_id: [u8; 32],
    family: u8,
    class: u8,
    /// Chunks decrypted so far.
    index: usize,
    chunk: Zeroizing<Vec<u8>>,
    position: usize,
}

impl<'a> ArtifactReader<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        artifact: &'a EncryptedArtifact<'a>,
        context: LocalSharedStatePackageContext,
        stream_id: [u8; 32],
        artifact_id: [u8; 32],
        family: u8,
        class: u8,
        key: &[u8; 32],
        maximum_plaintext_bytes: usize,
    ) -> Result<Self> {
        ensure!(
            !artifact.chunks.is_empty()
                && artifact.chunks.len() <= MAX_STATE_CHUNKS
                && artifact.total_plaintext_bytes <= u64::try_from(maximum_plaintext_bytes)?,
            "error encrypted-local-shared-package-artifact-bounds"
        );
        ensure!(
            aggregate_commitment(&artifact.chunks) == artifact.aggregate_commitment
                && artifact
                    .chunks
                    .iter()
                    .all(|chunk| codec::hash(&chunk.record) == chunk.record_commitment),
            "error encrypted-local-shared-package-aggregate-mismatch"
        );
        let cipher = XChaCha20Poly1305::new_from_slice(key)
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-invalid-key"))?;
        Ok(Self {
            artifact,
            cipher,
            context,
            stream_id,
            artifact_id,
            family,
            class,
            index: 0,
            chunk: Zeroizing::new(Vec::new()),
            position: 0,
        })
    }

    /// The plaintext bytes not yet taken.
    pub(crate) fn remaining(&self) -> u64 {
        let taken = self.index.saturating_sub(1) * CHUNK_PLAINTEXT_BYTES + self.position;
        self.artifact.total_plaintext_bytes - taken as u64
    }

    /// Up to `n` further plaintext bytes, never crossing a chunk boundary.
    /// Empty only at the end of the artifact.
    pub(crate) fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.position == self.chunk.len() {
            if self.index == self.artifact.chunks.len() {
                return Ok(&[]);
            }
            self.decrypt_next()?;
        }
        let end = self.position + n.min(self.chunk.len() - self.position);
        let piece = &self.chunk[self.position..end];
        self.position = end;
        Ok(piece)
    }

    fn decrypt_next(&mut self) -> Result<()> {
        let index = self.index;
        let count = self.artifact.chunks.len();
        let (header, ciphertext) = split_record(&self.artifact.chunks[index].record)?;
        let nonce = validate_chunk_header(
            header,
            self.context,
            self.stream_id,
            self.artifact_id,
            self.family,
            self.class,
            u32::try_from(index)?,
            u32::try_from(count)?,
            self.artifact.total_plaintext_bytes,
        )?;
        let nonce_value = XNonce::try_from(nonce.as_slice())
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-nonce-size"))?;
        let (ciphertext, tag) = ciphertext.split_at(ciphertext.len() - TAG_BYTES);
        self.chunk.clear();
        self.chunk.extend_from_slice(ciphertext);
        self.cipher
            .decrypt_inout_detached(
                &nonce_value,
                header,
                (&mut self.chunk[..]).into(),
                tag.try_into()?,
            )
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-authentication"))?;
        let expected = expected_chunk_plaintext_len(
            index,
            count,
            usize::try_from(self.artifact.total_plaintext_bytes)?,
        )?;
        ensure!(
            self.chunk.len() == expected,
            "error encrypted-local-shared-package-chunk-size-mismatch"
        );
        self.index += 1;
        self.position = 0;
        Ok(())
    }

    /// Checks that every chunk was decrypted and every byte taken.
    pub(crate) fn finish(self) -> Result<()> {
        ensure!(
            self.index == self.artifact.chunks.len() && self.position == self.chunk.len(),
            "error encrypted-local-shared-package-total-size-mismatch"
        );
        Ok(())
    }
}

fn expected_chunk_plaintext_len(index: usize, count: usize, total: usize) -> Result<usize> {
    ensure!(index < count && count > 0, "invalid chunk index");
    if index + 1 < count {
        return Ok(CHUNK_PLAINTEXT_BYTES);
    }
    let preceding = index
        .checked_mul(CHUNK_PLAINTEXT_BYTES)
        .context("chunk length overflow")?;
    total.checked_sub(preceding).context("invalid chunk total")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn chunk_header(
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
fn validate_chunk_header(
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

fn split_record(record: &[u8]) -> Result<(&[u8], &[u8])> {
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

fn aggregate_commitment(chunks: &[EncryptedChunk<'_>]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for chunk in chunks {
        digest.update(&chunk.record);
    }
    digest.finalize().into()
}

async fn load_selected_image_plaintexts(
    blob_dir: &Path,
    selected: &[(String, String)],
    snapshot: &AvenExport,
) -> Result<(Vec<SelectedImagePlaintext>, Vec<String>)> {
    ensure!(
        selected.len() <= MAX_PACKAGE_IMAGE_COUNT,
        "error encrypted-local-shared-package-too-many-images"
    );
    let mut total = 0_usize;
    let mut output = Vec::with_capacity(selected.len());
    let mut missing = Vec::new();
    for (source_sha256, classification) in selected {
        ensure!(
            matches!(
                classification.as_str(),
                "current_selected" | "extra_selected"
            ),
            "error encrypted-local-shared-package-image-classification"
        );
        let inventory = snapshot
            .tables
            .blob_inventory
            .iter()
            .find(|row| row.sha256 == source_sha256.as_str())
            .context("error encrypted-local-shared-package-image-inventory-missing")?;
        let path = crate::attachments::storage::object_path(blob_dir, source_sha256)?;
        let read =
            crate::attachments::blocking::run(move || Ok::<_, anyhow::Error>(std::fs::read(path)?))
                .await;
        let bytes = match read {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                missing.push(source_sha256.clone());
                continue;
            }
            Err(error) => {
                let attachment = snapshot
                    .tables
                    .task_attachments
                    .iter()
                    .find(|attachment| attachment.sha256 == source_sha256.as_str());
                let task = attachment.and_then(|attachment| {
                    snapshot.tables.tasks.iter().find(|task| {
                        task.id == attachment.task_id
                            && task.workspace_id == attachment.workspace_id
                    })
                });
                return Err(error).with_context(|| {
                    format!(
                        "error encrypted-local-shared-package-image-read task={:?} attachment={:?}",
                        task.map(|task| task.title.as_str())
                            .unwrap_or("unknown task"),
                        attachment
                            .and_then(|attachment| attachment.filename.as_deref())
                            .unwrap_or("unknown attachment")
                    )
                });
            }
        };
        ensure!(
            !bytes.is_empty()
                && bytes.len() <= MAX_IMAGE_PLAINTEXT_BYTES
                && i64::try_from(bytes.len())? == inventory.byte_size,
            "error encrypted-local-shared-package-selected-image-size-mismatch"
        );
        ensure!(
            crate::attachments::storage::sha256_hex(&bytes) == source_sha256.as_str(),
            "error encrypted-local-shared-package-selected-image-hash-mismatch"
        );
        let media_type = inventory.media_type.clone();
        let decode_bytes = bytes.to_vec();
        let facts = crate::attachments::blocking::run(move || {
            crate::attachments::decode::validate_image_blocking(decode_bytes, Some(&media_type))
        })
        .await
        .context("error encrypted-local-shared-package-selected-image-invalid")?
        .facts;
        for attachment in snapshot
            .tables
            .task_attachments
            .iter()
            .filter(|attachment| attachment.sha256 == source_sha256.as_str())
        {
            ensure!(
                attachment.media_type == inventory.media_type
                    && attachment.byte_size == inventory.byte_size
                    && attachment.width == Some(facts.width)
                    && attachment.height == Some(facts.height),
                "error encrypted-local-shared-package-selected-image-metadata-mismatch"
            );
        }
        total = total
            .checked_add(bytes.len())
            .context("image total overflow")?;
        ensure!(
            total <= MAX_PACKAGE_IMAGE_PLAINTEXT_BYTES,
            "error encrypted-local-shared-package-images-too-large"
        );
        output.push(SelectedImagePlaintext {
            source_sha256: source_sha256.clone(),
            bytes,
        });
    }
    Ok((output, missing))
}

async fn mark_capture_images_unavailable(
    database: &Database,
    candidate_id: &str,
    hashes: &[String],
) -> Result<()> {
    let mut conn = database.acquire_writer().await?;
    let mut tx = db::begin_immediate(&mut conn).await?;
    super::adoption::ensure_no_intent(&mut tx).await?;
    ensure!(
        !freeze::frozen_exists(&mut tx, candidate_id).await?,
        "error encrypted-local-shared-package-frozen"
    );
    let snapshot: Vec<u8> = sqlx::query_scalar(
        "SELECT snapshot FROM local_shared_capture_documents WHERE candidate_id = ?",
    )
    .bind(candidate_id)
    .fetch_one(&mut *tx)
    .await?;
    let mut persisted: super::PersistedLocalCapture =
        serde_json::from_str(&super::unpack_document(&snapshot)?)?;
    for hash in hashes {
        let image = persisted
            .images
            .iter_mut()
            .find(|image| image.sha256 == *hash)
            .context("error local-shared-capture-image-set-mismatch")?;
        image.classification = "unavailable".to_string();
        sqlx::query(
            "UPDATE local_shared_capture_images SET classification = 'unavailable'
             WHERE candidate_id = ? AND sha256 = ?",
        )
        .bind(candidate_id)
        .bind(hash)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM local_shared_capture_pins WHERE candidate_id = ? AND sha256 = ?")
            .bind(candidate_id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE local_shared_capture_documents SET snapshot = ? WHERE candidate_id = ?")
        .bind(super::pack_document(&persisted)?)
        .bind(candidate_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

fn random_id() -> Result<[u8; 32]> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).context("error encrypted-local-shared-package-rng")?;
    Ok(bytes)
}

fn decode_context_id(value: &str, name: &str) -> Result<[u8; 32]> {
    let decoded = hex::decode(value)
        .with_context(|| format!("error encrypted-local-shared-package-{name}-id-invalid"))?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-{name}-id-invalid"))
}

/// Freezes `package`: its bytes, the object ID of each selected image, and
/// the commitments to the capture it was built from, all under `conn`.
#[cfg(test)]
async fn persist_package(
    conn: &mut sqlx::SqliteConnection,
    package: &EncryptedLocalSharedStatePackage,
    capture: &NeverDispatchedLocalSharedCapture,
    attachments: &publication::AttachmentIndex,
) -> Result<()> {
    let upload = &package.upload;
    sqlx::query(
        "INSERT INTO local_shared_capture_publication(
             candidate_id, descriptor, data_catalog, prefix_catalog, image_catalog
         ) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&package.candidate_id)
    .bind(&upload.descriptor)
    .bind(&upload.catalogs[0])
    .bind(&upload.catalogs[1])
    .bind(&upload.catalogs[2])
    .execute(&mut *conn)
    .await?;
    let components = [
        (STATE_COMPONENT, &[][..], &upload.state),
        (MANIFEST_COMPONENT, &[][..], &upload.manifest),
    ]
    .into_iter()
    .chain(
        upload
            .images
            .iter()
            .map(|image| (IMAGE_COMPONENT, &image.object_id[..], &image.records)),
    );
    for (component, object_id, records) in components {
        for (index, record) in records.iter().enumerate() {
            sqlx::query(
                "INSERT INTO local_shared_capture_package_records(
                     candidate_id, component, object_id, chunk_index, record
                 ) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&package.candidate_id)
            .bind(component)
            .bind(object_id)
            .bind(i64::try_from(index)?)
            .bind(record)
            .execute(&mut *conn)
            .await?;
        }
    }
    let updated = sqlx::query(
        "UPDATE local_shared_capture_journal SET frozen_descriptor_commitment = ?
         WHERE candidate_id = ? AND frozen_descriptor_commitment IS NULL",
    )
    .bind(codec::hash(&upload.descriptor).as_slice())
    .bind(&package.candidate_id)
    .execute(&mut *conn)
    .await?;
    ensure!(
        updated.rows_affected() == 1,
        "error encrypted-local-shared-package-already-frozen"
    );
    super::validated::record_freeze(conn, capture, attachments).await
}

/// Descriptor and the data, prefix and image catalogs.
#[cfg(test)]
type PublicationRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

/// Whether the stored rows are exactly `package` under its freeze commitment,
/// so [`load_package`] would return it. Records are read one at a time rather
/// than as a second copy of the package.
#[cfg(test)]
async fn stored_package_matches(
    conn: &mut sqlx::SqliteConnection,
    package: &EncryptedLocalSharedStatePackage,
) -> Result<bool> {
    let upload = &package.upload;
    let frozen: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT frozen_descriptor_commitment FROM local_shared_capture_journal
         WHERE candidate_id = ?",
    )
    .bind(&package.candidate_id)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    if frozen.as_deref() != Some(codec::hash(&upload.descriptor).as_slice()) {
        return Ok(false);
    }
    let publication: Option<PublicationRow> = sqlx::query_as(
        "SELECT descriptor, data_catalog, prefix_catalog, image_catalog
         FROM local_shared_capture_publication WHERE candidate_id = ?",
    )
    .bind(&package.candidate_id)
    .fetch_optional(&mut *conn)
    .await?;
    if publication.is_none_or(|(descriptor, data, prefix, images)| {
        descriptor != upload.descriptor || [data, prefix, images] != upload.catalogs
    }) {
        return Ok(false);
    }
    let components = [
        (STATE_COMPONENT, &[][..], &upload.state),
        (MANIFEST_COMPONENT, &[][..], &upload.manifest),
    ]
    .into_iter()
    .chain(
        upload
            .images
            .iter()
            .map(|image| (IMAGE_COMPONENT, &image.object_id[..], &image.records)),
    );
    let mut expected = 0_i64;
    for (component, object_id, records) in components {
        for (index, record) in records.iter().enumerate() {
            let stored: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT record FROM local_shared_capture_package_records
                 WHERE candidate_id = ? AND component = ? AND object_id = ?
                   AND chunk_index = ?",
            )
            .bind(&package.candidate_id)
            .bind(component)
            .bind(object_id)
            .bind(i64::try_from(index)?)
            .fetch_optional(&mut *conn)
            .await?;
            if stored.as_ref() != Some(record) {
                return Ok(false);
            }
            expected += 1;
        }
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM local_shared_capture_package_records WHERE candidate_id = ?",
    )
    .bind(&package.candidate_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(count == expected)
}

/// Loads the frozen package and verifies every record against the committed
/// descriptor and catalogs. Missing or corrupt bytes fail without replacement.
#[cfg(test)]
pub(super) async fn load_package(
    conn: &mut sqlx::SqliteConnection,
    candidate_id: &str,
) -> Result<Option<EncryptedLocalSharedStatePackage>> {
    let frozen: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT frozen_descriptor_commitment FROM local_shared_capture_journal
         WHERE candidate_id = ?",
    )
    .bind(candidate_id)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let publication: Option<PublicationRow> = sqlx::query_as(
        "SELECT descriptor, data_catalog, prefix_catalog, image_catalog
         FROM local_shared_capture_publication WHERE candidate_id = ?",
    )
    .bind(candidate_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((descriptor, data_catalog, prefix_catalog, image_catalog)) = publication else {
        ensure!(
            frozen.is_none(),
            "error encrypted-local-shared-package-frozen-bytes-missing hint=cancel-never-dispatched-capture-and-recapture"
        );
        return Ok(None);
    };
    ensure!(
        frozen.as_deref() == Some(codec::hash(&descriptor).as_slice()),
        "error encrypted-local-shared-package-frozen-descriptor-mismatch"
    );
    let rows: Vec<(String, Vec<u8>, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT component, object_id, chunk_index, record
         FROM local_shared_capture_package_records
         WHERE candidate_id = ? ORDER BY component, object_id, chunk_index",
    )
    .bind(candidate_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut upload = publication::Package {
        descriptor,
        catalogs: [data_catalog, prefix_catalog, image_catalog],
        state: Vec::new(),
        manifest: Vec::new(),
        images: Vec::new(),
    };
    for (component, object_id, index, record) in rows {
        let records = match component.as_str() {
            STATE_COMPONENT => &mut upload.state,
            MANIFEST_COMPONENT => &mut upload.manifest,
            IMAGE_COMPONENT => {
                let object_id = <[u8; 32]>::try_from(object_id.as_slice()).map_err(|_| {
                    anyhow::anyhow!("error encrypted-local-shared-package-invalid-image-object")
                })?;
                if upload
                    .images
                    .last()
                    .is_none_or(|image| image.object_id != object_id)
                {
                    upload.images.push(publication::ImageRecords {
                        object_id,
                        records: Vec::new(),
                    });
                }
                &mut upload
                    .images
                    .last_mut()
                    .expect("image group was just ensured")
                    .records
            }
            _ => anyhow::bail!("error encrypted-local-shared-package-invalid-component"),
        };
        ensure!(
            usize::try_from(index)? == records.len(),
            "error encrypted-local-shared-package-chunk-order"
        );
        records.push(record);
    }
    publication::validate_keyless(&upload)
        .context("error encrypted-local-shared-package-frozen-records-invalid")?;
    Ok(Some(EncryptedLocalSharedStatePackage {
        candidate_id: candidate_id.to_string(),
        upload,
    }))
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod durable_tests;
