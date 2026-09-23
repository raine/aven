pub mod publication;

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::{LOCAL_CAPTURE_STATE, NeverDispatchedLocalSharedCapture, SharedStateCapture};
use crate::data_safety::export_types::AvenExport;
use crate::db::{self, Database};

const PACKAGE_STORAGE_VERSION: i64 = 1;
const PACKAGE_SUITE: i64 = 1;
const CHUNK_PLAINTEXT_BYTES: usize = 1_048_576;
const CHUNK_HEADER_BYTES: usize = 198;
const CHUNK_RECORD_OVERHEAD: usize = 222;
const MAX_STATE_PLAINTEXT_BYTES: usize = 256 * 1024 * 1024;
const MAX_STATE_CHUNKS: usize = 256;
const MAX_MANIFEST_PLAINTEXT_BYTES: usize = 1024 * 1024;
const MAX_IMAGE_PLAINTEXT_BYTES: usize = crate::attachments::validation::MAX_BLOB_BYTES;
const MAX_IMAGE_CHUNKS: usize = 25;
const MAX_PACKAGE_IMAGE_COUNT: usize = 1024;
const MAX_PACKAGE_IMAGE_PLAINTEXT_BYTES: usize = 256 * 1024 * 1024;
const IMAGE_FAMILY: u8 = 1;
const IMAGE_CLASS: u8 = 0;
const STATE_CLASS: u8 = 1;
const MANIFEST_CLASS: u8 = 2;

type ChunkRow = (i64, i64, i64, Vec<u8>, Vec<u8>);

/// Public cryptographic context supplied by the future vault owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalSharedStatePackageContext {
    pub vault_id: [u8; 32],
    pub generation_id: [u8; 32],
}

/// A generation secret supplied by a secure-store boundary.
///
/// Core never persists this value. Debug output is redacted and owned bytes are
/// zeroized on drop. Callers remain responsible for protected durable storage.
pub struct LocalSharedStatePackageKey([u8; 32]);

impl LocalSharedStatePackageKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    fn expose(&self) -> &[u8; 32] {
        &self.0
    }

    /// Borrows key bytes only for persistence at a host protected-store boundary.
    ///
    /// Callers must not place these bytes in SQLite, settings, logs, exports, or
    /// ordinary backups.
    pub fn protected_storage_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for LocalSharedStatePackageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LocalSharedStatePackageKey([REDACTED])")
    }
}

impl Drop for LocalSharedStatePackageKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Exact encrypted bytes for one local bootstrap package.
///
/// This value is transferable between local databases, but has no dispatch or
/// publication behavior. Upload components use the experimental publication codec;
/// this is not a security-approved production wire contract.
#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedLocalSharedStatePackage {
    candidate_id: String,
    stream_id: [u8; 32],
    context: LocalSharedStatePackageContext,
    state: EncryptedArtifact,
    manifest: EncryptedArtifact,
    images: Vec<EncryptedLocalSharedStateImage>,
    image_mappings: Vec<PrivateImageMapping>,
    descriptor: Vec<u8>,
    catalogs: [Vec<u8>; 3],
}

impl fmt::Debug for EncryptedLocalSharedStatePackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedLocalSharedStatePackage")
            .field("candidate_id", &self.candidate_id)
            .field("stream_id", &self.stream_id)
            .field("context", &self.context)
            .field("state", &self.state)
            .field("manifest", &self.manifest)
            .field("image_count", &self.images.len())
            .finish()
    }
}

impl EncryptedLocalSharedStatePackage {
    pub(crate) fn descriptor(&self) -> &[u8] {
        &self.descriptor
    }

    /// Copies the exact frozen descriptor, catalogs and encrypted records.
    /// Returning these bytes does not make this local package dispatchable.
    pub fn upload_package(&self) -> publication::Package {
        let mut images = self
            .images
            .iter()
            .map(|image| publication::ImageRecords {
                object_id: image.object_id,
                records: image
                    .artifact
                    .chunks
                    .iter()
                    .map(|chunk| chunk.record.clone())
                    .collect(),
            })
            .collect::<Vec<_>>();
        images.sort_by_key(|image| image.object_id);
        publication::Package {
            descriptor: self.descriptor.clone(),
            catalogs: self.catalogs.clone(),
            state: self
                .state
                .chunks
                .iter()
                .map(|chunk| chunk.record.clone())
                .collect(),
            manifest: self
                .manifest
                .chunks
                .iter()
                .map(|chunk| chunk.record.clone())
                .collect(),
            images,
        }
    }

    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    pub fn stream_id(&self) -> &[u8; 32] {
        &self.stream_id
    }

    pub fn context(&self) -> LocalSharedStatePackageContext {
        self.context
    }

    pub fn state_chunk_count(&self) -> usize {
        self.state.chunks.len()
    }

    pub fn state_total_plaintext_bytes(&self) -> u64 {
        self.state.total_plaintext_bytes
    }

    pub fn manifest_chunk_count(&self) -> usize {
        self.manifest.chunks.len()
    }

    pub fn manifest_total_plaintext_bytes(&self) -> u64 {
        self.manifest.total_plaintext_bytes
    }

    pub fn state_aggregate_commitment(&self) -> &[u8; 32] {
        &self.state.aggregate_commitment
    }

    pub fn manifest_aggregate_commitment(&self) -> &[u8; 32] {
        &self.manifest.aggregate_commitment
    }

    pub fn images(&self) -> &[EncryptedLocalSharedStateImage] {
        &self.images
    }

    pub fn state_chunks(&self) -> impl ExactSizeIterator<Item = (&[u8; 32], &[u8])> {
        self.state
            .chunks
            .iter()
            .map(|chunk| (&chunk.record_commitment, chunk.record.as_slice()))
    }

    pub fn manifest_chunks(&self) -> impl ExactSizeIterator<Item = (&[u8; 32], &[u8])> {
        self.manifest
            .chunks
            .iter()
            .map(|chunk| (&chunk.record_commitment, chunk.record.as_slice()))
    }
}

/// One opaque encrypted image representation in a local capture package.
///
/// The descriptor exposes no plaintext hash or attachment metadata. Those
/// mappings remain inside encrypted domain records and local private persistence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedLocalSharedStateImage {
    object_id: [u8; 32],
    artifact: EncryptedArtifact,
}

impl EncryptedLocalSharedStateImage {
    pub fn object_id(&self) -> &[u8; 32] {
        &self.object_id
    }

    pub fn total_plaintext_bytes(&self) -> u64 {
        self.artifact.total_plaintext_bytes
    }

    pub fn aggregate_commitment(&self) -> &[u8; 32] {
        &self.artifact.aggregate_commitment
    }

    pub fn chunks(&self) -> impl ExactSizeIterator<Item = (&[u8; 32], &[u8])> {
        self.artifact
            .chunks
            .iter()
            .map(|chunk| (&chunk.record_commitment, chunk.record.as_slice()))
    }
}

/// Authenticated plaintext recovered from a local image package.
#[derive(PartialEq, Eq)]
pub struct DecryptedLocalSharedStateImage {
    source_sha256: String,
    classification: String,
    object_id: [u8; 32],
    bytes: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for DecryptedLocalSharedStateImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecryptedLocalSharedStateImage")
            .field("classification", &self.classification)
            .field("object_id", &self.object_id)
            .field("byte_count", &self.bytes.len())
            .finish()
    }
}

impl DecryptedLocalSharedStateImage {
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    pub fn classification(&self) -> &str {
        &self.classification
    }

    pub fn object_id(&self) -> &[u8; 32] {
        &self.object_id
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EncryptedArtifact {
    total_plaintext_bytes: u64,
    aggregate_commitment: [u8; 32],
    chunks: Vec<EncryptedChunk>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EncryptedChunk {
    record_commitment: [u8; 32],
    record: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq)]
struct PrivateImageMapping {
    source_sha256: String,
    classification: String,
    object_id: [u8; 32],
}

struct SelectedImagePlaintext {
    source_sha256: String,
    classification: String,
    bytes: Zeroizing<Vec<u8>>,
}

impl Database {
    /// Reports frozen package ownership, including missing-byte corruption.
    /// A true result requires the host to load existing protected authority,
    /// never generate a replacement key.
    pub async fn has_local_shared_state_package_never_dispatched(&self) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_shared_capture_packages LIMIT 1)
                 OR EXISTS(SELECT 1 FROM local_shared_capture_journal
                           WHERE frozen_descriptor_commitment IS NOT NULL)",
        )
        .fetch_one(&mut *conn)
        .await?;
        Ok(exists != 0)
    }

    /// Encrypts the durable never-dispatched capture or returns its frozen package.
    ///
    /// The first successful call freezes descriptor, catalogs, ciphertext and
    /// image mappings in one SQLite transaction under the capture candidate ID.
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
        let capture = self
            .resume_local_shared_state_never_dispatched()
            .await?
            .context("error local-shared-capture-missing")?;
        let candidate_id = capture.candidate_id().to_string();
        let stream_id = decode_context_id(capture.stream_id(), "stream")?;

        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        super::adoption::ensure_no_intent(&mut tx).await?;
        let selected_inventory: Vec<(String, String)> = sqlx::query_as(
            "SELECT sha256, classification FROM local_shared_capture_images
             WHERE candidate_id = ? AND classification != 'unavailable'
             ORDER BY sha256",
        )
        .bind(&candidate_id)
        .fetch_all(&mut *tx)
        .await?;
        if let Some(package) = load_package(&mut tx, &candidate_id).await? {
            tx.commit().await?;
            validate_package_identity(&package, context, stream_id, membership_predecessor)?;
            validate_package_image_coverage(&package, &selected_inventory)?;
            publication::validate_against_capture(
                &package.upload_package(),
                &capture,
                &package,
                key,
                membership_predecessor,
            )?;
            return Ok(package);
        }
        tx.commit().await?;
        drop(conn);

        let selected = load_selected_image_plaintexts(
            blob_dir,
            &selected_inventory,
            &capture.shared_state().snapshot,
        )
        .await?;
        let package = encrypt_package(&capture, context, &selected, key, membership_predecessor)?;

        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        super::adoption::ensure_no_intent(&mut tx).await?;
        let active: Option<(String, String)> = sqlx::query_as(
            "SELECT candidate_id, state FROM local_shared_capture_journal WHERE singleton = 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        ensure!(
            active.as_ref() == Some(&(candidate_id.clone(), LOCAL_CAPTURE_STATE.to_string())),
            "error local-shared-capture-changed-during-packaging"
        );
        super::validate_persisted_local_capture(
            &mut tx,
            &candidate_id,
            &capture.images,
            &capture.capture.snapshot,
        )
        .await?;
        if let Some(existing) = load_package(&mut tx, &candidate_id).await? {
            tx.commit().await?;
            validate_package_identity(&existing, context, stream_id, membership_predecessor)?;
            validate_package_image_coverage(&existing, &selected_inventory)?;
            publication::validate_against_capture(
                &existing.upload_package(),
                &capture,
                &existing,
                key,
                membership_predecessor,
            )?;
            return Ok(existing);
        }
        persist_package(&mut tx, &package).await?;
        let stored = load_package(&mut tx, &candidate_id)
            .await?
            .context("error encrypted-local-shared-package-write-incomplete")?;
        ensure!(
            stored == package,
            "error encrypted-local-shared-package-write-mismatch"
        );
        tx.commit().await?;
        Ok(package)
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
        let capture = decrypt_package(package, key)?;
        self.install_shared_state(&capture).await
    }
}

/// Authenticates and decrypts all image objects in a local capture package.
///
/// This local API returns private plaintext mappings. It is not a network
/// descriptor or publication format.
pub fn decrypt_local_shared_state_package_images(
    package: &EncryptedLocalSharedStatePackage,
    key: &LocalSharedStatePackageKey,
) -> Result<Vec<DecryptedLocalSharedStateImage>> {
    decrypt_package(package, key)?;
    package
        .image_mappings
        .iter()
        .map(|mapping| {
            let image = package
                .images
                .iter()
                .find(|image| image.object_id == mapping.object_id)
                .context("error encrypted-local-shared-package-image-descriptor-mismatch")?;
            let image_key = derive_image_key(key, package.context, image.object_id)?;
            let bytes = decrypt_artifact(
                &image.artifact,
                package.context,
                package.stream_id,
                image.object_id,
                IMAGE_FAMILY,
                IMAGE_CLASS,
                &image_key,
                MAX_IMAGE_PLAINTEXT_BYTES,
            )?;
            Ok(DecryptedLocalSharedStateImage {
                source_sha256: mapping.source_sha256.clone(),
                classification: mapping.classification.clone(),
                object_id: image.object_id,
                bytes: Zeroizing::new(bytes),
            })
        })
        .collect()
}

fn encrypt_package(
    capture: &NeverDispatchedLocalSharedCapture,
    context: LocalSharedStatePackageContext,
    selected_images: &[SelectedImagePlaintext],
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<EncryptedLocalSharedStatePackage> {
    let stream_id = decode_context_id(capture.stream_id(), "stream")?;
    let mut images = Vec::with_capacity(selected_images.len());
    let mut mappings = Vec::with_capacity(selected_images.len());
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
        images.push(EncryptedLocalSharedStateImage {
            object_id,
            artifact,
        });
        mappings.push(PrivateImageMapping {
            source_sha256: image.source_sha256.clone(),
            classification: image.classification.clone(),
            object_id,
        });
    }
    Ok(publication::build(
        capture,
        context,
        &images,
        &mappings,
        key,
        membership_predecessor,
    )?)
}

fn decrypt_package(
    package: &EncryptedLocalSharedStatePackage,
    key: &LocalSharedStatePackageKey,
) -> Result<SharedStateCapture> {
    Ok(publication::decrypt_local(package, key)?)
}

fn validate_package_image_coverage(
    package: &EncryptedLocalSharedStatePackage,
    selected_inventory: &[(String, String)],
) -> Result<()> {
    let stored = package
        .image_mappings
        .iter()
        .map(|mapping| {
            (
                mapping.source_sha256.as_str(),
                mapping.classification.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let expected = selected_inventory
        .iter()
        .map(|(sha256, classification)| (sha256.as_str(), classification.as_str()))
        .collect::<Vec<_>>();
    ensure!(
        stored == expected,
        "error encrypted-local-shared-package-image-coverage-mismatch hint=cancel-never-dispatched-capture-and-recapture"
    );
    Ok(())
}

fn validate_package_identity(
    package: &EncryptedLocalSharedStatePackage,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    membership_predecessor: [u8; 32],
) -> Result<()> {
    ensure!(
        package.context == context
            && package.stream_id == stream_id
            && publication::membership(package)? == membership_predecessor,
        "error encrypted-local-shared-package-context-mismatch"
    );
    Ok(())
}

fn derive_bootstrap_class_key(
    key: &LocalSharedStatePackageKey,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    candidate: [u8; 32],
    class: u8,
) -> Result<Zeroizing<[u8; 32]>> {
    let hkdf = Hkdf::<Sha256>::new(Some(b"aven-e2ee/v1/generation"), key.expose().as_slice());
    let info = cce(
        b"aven-e2ee/v1/key/bootstrap-artifact",
        &[
            &context.vault_id,
            &context.generation_id,
            &stream_id,
            &candidate,
            &[class],
        ],
    )?;
    let mut output = [0_u8; 32];
    hkdf.expand(&info, &mut output)
        .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-key-derivation"))?;
    Ok(Zeroizing::new(output))
}

fn derive_image_key(
    key: &LocalSharedStatePackageKey,
    context: LocalSharedStatePackageContext,
    object_id: [u8; 32],
) -> Result<Zeroizing<[u8; 32]>> {
    let hkdf = Hkdf::<Sha256>::new(Some(b"aven-e2ee/v1/generation"), key.expose().as_slice());
    let info = cce(
        b"aven-e2ee/v1/key/image-object",
        &[&context.vault_id, &context.generation_id, &object_id],
    )?;
    let mut output = [0_u8; 32];
    hkdf.expand(&info, &mut output)
        .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-key-derivation"))?;
    Ok(Zeroizing::new(output))
}

fn cce(label: &[u8], fields: &[&[u8]]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    push_bytes(&mut output, label)?;
    for field in fields {
        push_bytes(&mut output, field)?;
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn encrypt_artifact(
    plaintext: &[u8],
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    artifact_id: [u8; 32],
    family: u8,
    class: u8,
    key: &[u8; 32],
) -> Result<EncryptedArtifact> {
    let chunk_count = plaintext.len().div_ceil(CHUNK_PLAINTEXT_BYTES).max(1);
    let chunk_count_u32 = u32::try_from(chunk_count)?;
    let total = u64::try_from(plaintext.len())?;
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-invalid-key"))?;
    let mut chunks = Vec::with_capacity(chunk_count);
    for index in 0..chunk_count {
        let start = index
            .checked_mul(CHUNK_PLAINTEXT_BYTES)
            .context("chunk offset overflow")?;
        let end = plaintext.len().min(
            start
                .checked_add(CHUNK_PLAINTEXT_BYTES)
                .context("chunk end overflow")?,
        );
        let chunk_plaintext = &plaintext[start..end];
        let mut nonce = [0_u8; 24];
        getrandom::fill(&mut nonce).context("error encrypted-local-shared-package-rng")?;
        let header = chunk_header(
            context,
            stream_id,
            artifact_id,
            family,
            class,
            u32::try_from(index)?,
            chunk_count_u32,
            total,
            nonce,
        )?;
        let nonce_value = XNonce::try_from(nonce.as_slice())
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-nonce-size"))?;
        let ciphertext = cipher
            .encrypt(
                &nonce_value,
                Payload {
                    msg: chunk_plaintext,
                    aad: &header,
                },
            )
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-encryption"))?;
        let mut record = Vec::with_capacity(CHUNK_RECORD_OVERHEAD + chunk_plaintext.len());
        push_bytes(&mut record, &header)?;
        push_bytes(&mut record, &ciphertext)?;
        chunks.push(EncryptedChunk {
            record_commitment: sha256(&record),
            record,
        });
    }
    let aggregate_commitment = aggregate_commitment(&chunks);
    Ok(EncryptedArtifact {
        total_plaintext_bytes: total,
        aggregate_commitment,
        chunks,
    })
}

#[allow(clippy::too_many_arguments)]
fn decrypt_artifact(
    artifact: &EncryptedArtifact,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    artifact_id: [u8; 32],
    family: u8,
    class: u8,
    key: &[u8; 32],
    maximum_plaintext_bytes: usize,
) -> Result<Vec<u8>> {
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
                .all(|chunk| sha256(&chunk.record) == chunk.record_commitment),
        "error encrypted-local-shared-package-aggregate-mismatch"
    );
    let chunk_count = u32::try_from(artifact.chunks.len())?;
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-invalid-key"))?;
    let mut plaintext = Vec::with_capacity(usize::try_from(artifact.total_plaintext_bytes)?);
    for (index, chunk) in artifact.chunks.iter().enumerate() {
        ensure!(
            sha256(&chunk.record) == chunk.record_commitment,
            "error encrypted-local-shared-package-record-commitment-mismatch"
        );
        let (header, ciphertext) = split_record(&chunk.record)?;
        let nonce = validate_chunk_header(
            header,
            context,
            stream_id,
            artifact_id,
            family,
            class,
            u32::try_from(index)?,
            chunk_count,
            artifact.total_plaintext_bytes,
        )?;
        let nonce_value = XNonce::try_from(nonce.as_slice())
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-nonce-size"))?;
        let decrypted = cipher
            .decrypt(
                &nonce_value,
                Payload {
                    msg: ciphertext,
                    aad: header,
                },
            )
            .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-authentication"))?;
        let expected = expected_chunk_plaintext_len(
            index,
            artifact.chunks.len(),
            usize::try_from(artifact.total_plaintext_bytes)?,
        )?;
        ensure!(
            decrypted.len() == expected,
            "error encrypted-local-shared-package-chunk-size-mismatch"
        );
        plaintext.extend_from_slice(&decrypted);
    }
    ensure!(
        plaintext.len() == usize::try_from(artifact.total_plaintext_bytes)?,
        "error encrypted-local-shared-package-total-size-mismatch"
    );
    Ok(plaintext)
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
fn chunk_header(
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
    push_bytes(&mut header, &context.vault_id)?;
    push_bytes(&mut header, &stream_id)?;
    push_bytes(&mut header, &context.generation_id)?;
    push_bytes(&mut header, &artifact_id)?;
    header.push(family);
    header.push(class);
    header.extend_from_slice(&index.to_be_bytes());
    header.extend_from_slice(&count.to_be_bytes());
    header.extend_from_slice(&total.to_be_bytes());
    push_bytes(&mut header, &nonce)?;
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

fn push_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    output.extend_from_slice(&u32::try_from(bytes.len())?.to_be_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn aggregate_commitment(chunks: &[EncryptedChunk]) -> [u8; 32] {
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
) -> Result<Vec<SelectedImagePlaintext>> {
    ensure!(
        selected.len() <= MAX_PACKAGE_IMAGE_COUNT,
        "error encrypted-local-shared-package-too-many-images"
    );
    let mut total = 0_usize;
    let mut output = Vec::with_capacity(selected.len());
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
        let bytes = crate::attachments::blocking::run(move || {
            Ok::<_, anyhow::Error>(Zeroizing::new(std::fs::read(path)?))
        })
        .await
        .context("error encrypted-local-shared-package-selected-image-missing")?;
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
            classification: classification.clone(),
            bytes,
        });
    }
    Ok(output)
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

async fn persist_package(
    conn: &mut sqlx::SqliteConnection,
    package: &EncryptedLocalSharedStatePackage,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO local_shared_capture_packages(
             candidate_id, format_version, suite, vault_id, generation_id,
             state_total_plaintext_bytes, state_chunk_count, state_aggregate_commitment,
             manifest_total_plaintext_bytes, manifest_chunk_count,
             manifest_aggregate_commitment, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&package.candidate_id)
    .bind(PACKAGE_STORAGE_VERSION)
    .bind(PACKAGE_SUITE)
    .bind(package.context.vault_id.as_slice())
    .bind(package.context.generation_id.as_slice())
    .bind(i64::try_from(package.state.total_plaintext_bytes)?)
    .bind(i64::try_from(package.state.chunks.len())?)
    .bind(package.state.aggregate_commitment.as_slice())
    .bind(i64::try_from(package.manifest.total_plaintext_bytes)?)
    .bind(i64::try_from(package.manifest.chunks.len())?)
    .bind(package.manifest.aggregate_commitment.as_slice())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO local_shared_capture_publication(
             candidate_id, descriptor, data_catalog, prefix_catalog, image_catalog
         ) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&package.candidate_id)
    .bind(&package.descriptor)
    .bind(&package.catalogs[0])
    .bind(&package.catalogs[1])
    .bind(&package.catalogs[2])
    .execute(&mut *conn)
    .await?;
    for (class, artifact) in [
        (STATE_CLASS, &package.state),
        (MANIFEST_CLASS, &package.manifest),
    ] {
        for (index, chunk) in artifact.chunks.iter().enumerate() {
            sqlx::query(
                "INSERT INTO local_shared_capture_package_chunks(
                     candidate_id, class, chunk_index, record_length,
                     record_commitment, record
                 ) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&package.candidate_id)
            .bind(i64::from(class))
            .bind(i64::try_from(index)?)
            .bind(i64::try_from(chunk.record.len())?)
            .bind(chunk.record_commitment.as_slice())
            .bind(&chunk.record)
            .execute(&mut *conn)
            .await?;
        }
    }
    ensure!(
        package.image_mappings.len() == package.images.len(),
        "error encrypted-local-shared-package-image-count-mismatch"
    );
    for (mapping, image) in package.image_mappings.iter().zip(&package.images) {
        ensure!(
            mapping.object_id == image.object_id,
            "error encrypted-local-shared-package-image-descriptor-mismatch"
        );
        sqlx::query(
            "INSERT INTO local_shared_capture_package_images(
                 candidate_id, source_sha256, classification, object_id,
                 total_plaintext_bytes, chunk_count, aggregate_commitment
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&package.candidate_id)
        .bind(&mapping.source_sha256)
        .bind(&mapping.classification)
        .bind(image.object_id.as_slice())
        .bind(i64::try_from(image.artifact.total_plaintext_bytes)?)
        .bind(i64::try_from(image.artifact.chunks.len())?)
        .bind(image.artifact.aggregate_commitment.as_slice())
        .execute(&mut *conn)
        .await?;
        for (index, chunk) in image.artifact.chunks.iter().enumerate() {
            sqlx::query(
                "INSERT INTO local_shared_capture_package_image_chunks(
                     candidate_id, object_id, chunk_index, record_length,
                     record_commitment, record
                 ) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&package.candidate_id)
            .bind(image.object_id.as_slice())
            .bind(i64::try_from(index)?)
            .bind(i64::try_from(chunk.record.len())?)
            .bind(chunk.record_commitment.as_slice())
            .bind(&chunk.record)
            .execute(&mut *conn)
            .await?;
        }
    }
    let updated = sqlx::query(
        "UPDATE local_shared_capture_journal SET frozen_descriptor_commitment = ?
         WHERE candidate_id = ? AND frozen_descriptor_commitment IS NULL",
    )
    .bind(sha256(&package.descriptor).as_slice())
    .bind(&package.candidate_id)
    .execute(&mut *conn)
    .await?;
    ensure!(
        updated.rows_affected() == 1,
        "error encrypted-local-shared-package-already-frozen"
    );
    Ok(())
}

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
    type PackageRow = (
        i64,
        i64,
        Vec<u8>,
        Vec<u8>,
        i64,
        i64,
        Vec<u8>,
        i64,
        i64,
        Vec<u8>,
    );
    let row: Option<PackageRow> = sqlx::query_as(
        "SELECT format_version, suite, vault_id, generation_id,
                state_total_plaintext_bytes, state_chunk_count,
                state_aggregate_commitment, manifest_total_plaintext_bytes,
                manifest_chunk_count, manifest_aggregate_commitment
         FROM local_shared_capture_packages WHERE candidate_id = ?",
    )
    .bind(candidate_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((
        format_version,
        suite,
        vault_id,
        generation_id,
        state_total,
        state_count,
        state_aggregate,
        manifest_total,
        manifest_count,
        manifest_aggregate,
    )) = row
    else {
        ensure!(
            frozen.is_none(),
            "error encrypted-local-shared-package-frozen-bytes-missing"
        );
        return Ok(None);
    };
    ensure!(
        format_version == PACKAGE_STORAGE_VERSION && suite == PACKAGE_SUITE,
        "error encrypted-local-shared-package-unsupported"
    );
    type PublicationRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);
    let publication: Option<PublicationRow> = sqlx::query_as(
        "SELECT descriptor, data_catalog, prefix_catalog, image_catalog
         FROM local_shared_capture_publication WHERE candidate_id = ?",
    )
    .bind(candidate_id)
    .fetch_optional(&mut *conn)
    .await?;
    let (descriptor, data_catalog, prefix_catalog, image_catalog) = publication.context(
        "error encrypted-local-shared-package-format-incompatible-or-incomplete hint=cancel-never-dispatched-capture-and-recapture",
    )?;
    ensure!(
        frozen.as_deref() == Some(sha256(&descriptor).as_slice()),
        "error encrypted-local-shared-package-frozen-descriptor-mismatch"
    );
    let context = LocalSharedStatePackageContext {
        vault_id: vec_to_array(vault_id, "vault id")?,
        generation_id: vec_to_array(generation_id, "generation id")?,
    };
    let stream_id: String = sqlx::query_scalar(
        "SELECT stream_id FROM local_shared_capture_journal WHERE candidate_id = ?",
    )
    .bind(candidate_id)
    .fetch_one(&mut *conn)
    .await?;
    let chunks: Vec<ChunkRow> = sqlx::query_as(
        "SELECT class, chunk_index, record_length, record_commitment, record
         FROM local_shared_capture_package_chunks
         WHERE candidate_id = ? ORDER BY class, chunk_index",
    )
    .bind(candidate_id)
    .fetch_all(&mut *conn)
    .await?;
    let state = load_artifact(
        &chunks,
        STATE_CLASS,
        state_total,
        state_count,
        state_aggregate,
        MAX_STATE_PLAINTEXT_BYTES,
        MAX_STATE_CHUNKS,
    )?;
    let manifest = load_artifact(
        &chunks,
        MANIFEST_CLASS,
        manifest_total,
        manifest_count,
        manifest_aggregate,
        MAX_MANIFEST_PLAINTEXT_BYTES,
        1,
    )?;
    ensure!(
        chunks.len() == state.chunks.len() + manifest.chunks.len(),
        "error encrypted-local-shared-package-chunk-class"
    );
    type ImageRow = (String, String, Vec<u8>, i64, i64, Vec<u8>);
    let image_rows: Vec<ImageRow> = sqlx::query_as(
        "SELECT source_sha256, classification, object_id,
                total_plaintext_bytes, chunk_count, aggregate_commitment
         FROM local_shared_capture_package_images
         WHERE candidate_id = ? ORDER BY source_sha256",
    )
    .bind(candidate_id)
    .fetch_all(&mut *conn)
    .await?;
    ensure!(
        image_rows.len() <= MAX_PACKAGE_IMAGE_COUNT,
        "error encrypted-local-shared-package-too-many-images"
    );
    let mut images = Vec::with_capacity(image_rows.len());
    let mut image_mappings = Vec::with_capacity(image_rows.len());
    let mut image_total = 0_usize;
    for (source_sha256, classification, object_id, total, count, aggregate) in image_rows {
        let object_id = vec_to_array(object_id, "image object id")?;
        let image_chunks: Vec<(i64, i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT chunk_index, record_length, record_commitment, record
             FROM local_shared_capture_package_image_chunks
             WHERE candidate_id = ? AND object_id = ? ORDER BY chunk_index",
        )
        .bind(candidate_id)
        .bind(object_id.as_slice())
        .fetch_all(&mut *conn)
        .await?;
        let artifact = load_image_artifact(image_chunks, total, count, aggregate)?;
        image_total = image_total
            .checked_add(usize::try_from(artifact.total_plaintext_bytes)?)
            .context("image total overflow")?;
        ensure!(
            image_total <= MAX_PACKAGE_IMAGE_PLAINTEXT_BYTES,
            "error encrypted-local-shared-package-images-too-large"
        );
        image_mappings.push(PrivateImageMapping {
            source_sha256,
            classification,
            object_id,
        });
        images.push(EncryptedLocalSharedStateImage {
            object_id,
            artifact,
        });
    }
    Ok(Some(EncryptedLocalSharedStatePackage {
        candidate_id: candidate_id.to_string(),
        stream_id: decode_context_id(&stream_id, "stream")?,
        context,
        state,
        manifest,
        images,
        image_mappings,
        descriptor,
        catalogs: [data_catalog, prefix_catalog, image_catalog],
    }))
}

fn load_image_artifact(
    rows: Vec<(i64, i64, Vec<u8>, Vec<u8>)>,
    total: i64,
    count: i64,
    aggregate: Vec<u8>,
) -> Result<EncryptedArtifact> {
    ensure!(
        total > 0
            && usize::try_from(total)? <= MAX_IMAGE_PLAINTEXT_BYTES
            && count > 0
            && usize::try_from(count)? <= MAX_IMAGE_CHUNKS
            && rows.len() == usize::try_from(count)?,
        "error encrypted-local-shared-package-image-bounds"
    );
    let mut chunks = Vec::with_capacity(rows.len());
    for (expected, (index, record_length, commitment, record)) in rows.into_iter().enumerate() {
        ensure!(
            index == i64::try_from(expected)? && record_length == i64::try_from(record.len())?,
            "error encrypted-local-shared-package-image-chunk-order"
        );
        chunks.push(EncryptedChunk {
            record_commitment: vec_to_array(commitment, "image record commitment")?,
            record,
        });
    }
    let artifact = EncryptedArtifact {
        total_plaintext_bytes: u64::try_from(total)?,
        aggregate_commitment: vec_to_array(aggregate, "image aggregate commitment")?,
        chunks,
    };
    ensure!(
        aggregate_commitment(&artifact.chunks) == artifact.aggregate_commitment
            && artifact
                .chunks
                .iter()
                .all(|chunk| sha256(&chunk.record) == chunk.record_commitment),
        "error encrypted-local-shared-package-image-aggregate-mismatch"
    );
    Ok(artifact)
}

fn load_artifact(
    rows: &[ChunkRow],
    class: u8,
    total: i64,
    count: i64,
    aggregate: Vec<u8>,
    maximum_plaintext_bytes: usize,
    maximum_chunks: usize,
) -> Result<EncryptedArtifact> {
    ensure!(
        total >= 0
            && usize::try_from(total)? <= maximum_plaintext_bytes
            && count > 0
            && usize::try_from(count)? <= maximum_chunks,
        "error encrypted-local-shared-package-bounds"
    );
    let selected = rows
        .iter()
        .filter(|row| row.0 == i64::from(class))
        .collect::<Vec<_>>();
    ensure!(
        selected.len() == usize::try_from(count)?,
        "error encrypted-local-shared-package-chunk-count"
    );
    let mut chunks = Vec::with_capacity(selected.len());
    for (expected, row) in selected.into_iter().enumerate() {
        ensure!(
            row.1 == i64::try_from(expected)? && row.2 == i64::try_from(row.4.len())?,
            "error encrypted-local-shared-package-chunk-order"
        );
        chunks.push(EncryptedChunk {
            record_commitment: vec_to_array(row.3.clone(), "record commitment")?,
            record: row.4.clone(),
        });
    }
    let artifact = EncryptedArtifact {
        total_plaintext_bytes: u64::try_from(total)?,
        aggregate_commitment: vec_to_array(aggregate, "aggregate commitment")?,
        chunks,
    };
    ensure!(
        aggregate_commitment(&artifact.chunks) == artifact.aggregate_commitment
            && artifact
                .chunks
                .iter()
                .all(|chunk| sha256(&chunk.record) == chunk.record_commitment),
        "error encrypted-local-shared-package-aggregate-mismatch"
    );
    Ok(artifact)
}

fn vec_to_array(bytes: Vec<u8>, name: &str) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("error encrypted-local-shared-package-invalid-{name}"))
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod durable_tests;
