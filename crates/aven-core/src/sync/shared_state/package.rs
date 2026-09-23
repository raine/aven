use std::fmt;

use anyhow::{Context, Result, ensure};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::{LOCAL_CAPTURE_STATE, SharedStateCapture};
use crate::data_safety::export_types::AvenExport;
use crate::db::{self, Database};

const PACKAGE_FORMAT_VERSION: i64 = 1;
const PACKAGE_SUITE: i64 = 1;
const CHUNK_PLAINTEXT_BYTES: usize = 1_048_576;
const CHUNK_HEADER_BYTES: usize = 198;
const CHUNK_RECORD_OVERHEAD: usize = 222;
const MAX_STATE_PLAINTEXT_BYTES: usize = 256 * 1024 * 1024;
const MAX_STATE_CHUNKS: usize = 256;
const MAX_MANIFEST_PLAINTEXT_BYTES: usize = 1024 * 1024;
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
/// publication behavior. Its format is a versioned local profile, not the E2EE
/// wire contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedLocalSharedStatePackage {
    candidate_id: String,
    stream_id: [u8; 32],
    context: LocalSharedStatePackageContext,
    state: EncryptedArtifact,
    manifest: EncryptedArtifact,
}

impl EncryptedLocalSharedStatePackage {
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

#[derive(Serialize)]
struct PackagePlaintextRef<'a> {
    format: &'static str,
    version: i64,
    snapshot: &'a AvenExport,
}

#[derive(Deserialize)]
struct PackagePlaintext {
    format: String,
    version: i64,
    snapshot: AvenExport,
}

#[derive(Serialize, Deserialize)]
struct PackageManifest {
    format: String,
    version: i64,
    suite: i64,
    vault_id: String,
    stream_id: String,
    generation_id: String,
    candidate_id: String,
    prefix_count: u64,
    attachment_metadata_count: u64,
    attachment_bytes_included: bool,
    state: ManifestArtifact,
}

#[derive(Serialize, Deserialize)]
struct ManifestArtifact {
    total_plaintext_bytes: u64,
    chunk_count: u32,
    aggregate_commitment: String,
    chunks: Vec<ManifestChunk>,
}

#[derive(Serialize, Deserialize)]
struct ManifestChunk {
    record_length: u64,
    record_commitment: String,
}

impl Database {
    /// Reports whether the durable capture already owns frozen encrypted bytes.
    pub async fn has_local_shared_state_package_never_dispatched(&self) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_shared_capture_packages LIMIT 1)",
        )
        .fetch_one(&mut *conn)
        .await?;
        Ok(exists != 0)
    }

    /// Encrypts the durable never-dispatched capture or returns its frozen package.
    ///
    /// The first successful call persists every ciphertext record atomically.
    /// Later calls with the same context and key return those exact bytes. A key
    /// or context mismatch fails without replacing the package.
    pub async fn package_local_shared_state_never_dispatched(
        &self,
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
    ) -> Result<EncryptedLocalSharedStatePackage> {
        let capture = self
            .resume_local_shared_state_never_dispatched()
            .await?
            .context("error local-shared-capture-missing")?;
        let candidate_id = capture.candidate_id().to_string();
        let stream_id = decode_context_id(capture.stream_id(), "stream")?;

        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        if let Some(package) = load_package(&mut tx, &candidate_id).await? {
            tx.commit().await?;
            validate_package_identity(&package, context, stream_id)?;
            decrypt_package(&package, key)?;
            return Ok(package);
        }
        tx.commit().await?;
        drop(conn);

        let package = encrypt_package(
            &candidate_id,
            stream_id,
            context,
            capture.shared_state(),
            key,
        )?;

        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        let active: Option<(String, String)> = sqlx::query_as(
            "SELECT candidate_id, state FROM local_shared_capture_journal WHERE singleton = 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        ensure!(
            active.as_ref() == Some(&(candidate_id.clone(), LOCAL_CAPTURE_STATE.to_string())),
            "error local-shared-capture-changed-during-packaging"
        );
        if let Some(existing) = load_package(&mut tx, &candidate_id).await? {
            tx.commit().await?;
            validate_package_identity(&existing, context, stream_id)?;
            decrypt_package(&existing, key)?;
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

fn encrypt_package(
    candidate_id: &str,
    stream_id: [u8; 32],
    context: LocalSharedStatePackageContext,
    capture: &SharedStateCapture,
    key: &LocalSharedStatePackageKey,
) -> Result<EncryptedLocalSharedStatePackage> {
    let candidate = decode_context_id(candidate_id, "candidate")?;
    let plaintext = serde_json::to_vec(&PackagePlaintextRef {
        format: "aven-encrypted-local-shared-state",
        version: PACKAGE_FORMAT_VERSION,
        snapshot: &capture.snapshot,
    })?;
    ensure!(
        plaintext.len() <= MAX_STATE_PLAINTEXT_BYTES,
        "error encrypted-local-shared-package-too-large"
    );
    let state_key = derive_class_key(key, context, stream_id, candidate, STATE_CLASS)?;
    let state = encrypt_artifact(
        &plaintext,
        context,
        stream_id,
        candidate,
        STATE_CLASS,
        &state_key,
    )?;
    ensure!(
        state.chunks.len() <= MAX_STATE_CHUNKS,
        "error encrypted-local-shared-package-too-many-chunks"
    );

    let manifest_plaintext = serde_json::to_vec(&PackageManifest {
        format: "aven-encrypted-local-shared-state-manifest".to_string(),
        version: PACKAGE_FORMAT_VERSION,
        suite: PACKAGE_SUITE,
        vault_id: hex::encode(context.vault_id),
        stream_id: hex::encode(stream_id),
        generation_id: hex::encode(context.generation_id),
        candidate_id: candidate_id.to_string(),
        prefix_count: u64::try_from(capture.snapshot.tables.changes.len())?,
        attachment_metadata_count: u64::try_from(capture.snapshot.tables.task_attachments.len())?,
        attachment_bytes_included: false,
        state: manifest_artifact(&state)?,
    })?;
    ensure!(
        manifest_plaintext.len() <= MAX_MANIFEST_PLAINTEXT_BYTES,
        "error encrypted-local-shared-package-manifest-too-large"
    );
    let manifest_key = derive_class_key(key, context, stream_id, candidate, MANIFEST_CLASS)?;
    let manifest = encrypt_artifact(
        &manifest_plaintext,
        context,
        stream_id,
        candidate,
        MANIFEST_CLASS,
        &manifest_key,
    )?;

    Ok(EncryptedLocalSharedStatePackage {
        candidate_id: candidate_id.to_string(),
        stream_id,
        context,
        state,
        manifest,
    })
}

fn decrypt_package(
    package: &EncryptedLocalSharedStatePackage,
    key: &LocalSharedStatePackageKey,
) -> Result<SharedStateCapture> {
    let candidate = decode_context_id(&package.candidate_id, "candidate")?;
    let manifest_key = derive_class_key(
        key,
        package.context,
        package.stream_id,
        candidate,
        MANIFEST_CLASS,
    )?;
    let manifest_bytes = decrypt_artifact(
        &package.manifest,
        package.context,
        package.stream_id,
        candidate,
        MANIFEST_CLASS,
        &manifest_key,
        MAX_MANIFEST_PLAINTEXT_BYTES,
    )?;
    let manifest: PackageManifest = serde_json::from_slice(&manifest_bytes)
        .context("error encrypted-local-shared-package-manifest-malformed")?;
    validate_manifest(package, &manifest)?;

    let state_key = derive_class_key(
        key,
        package.context,
        package.stream_id,
        candidate,
        STATE_CLASS,
    )?;
    let state_bytes = decrypt_artifact(
        &package.state,
        package.context,
        package.stream_id,
        candidate,
        STATE_CLASS,
        &state_key,
        MAX_STATE_PLAINTEXT_BYTES,
    )?;
    let plaintext: PackagePlaintext = serde_json::from_slice(&state_bytes)
        .context("error encrypted-local-shared-package-state-malformed")?;
    ensure!(
        plaintext.format == "aven-encrypted-local-shared-state"
            && plaintext.version == PACKAGE_FORMAT_VERSION,
        "error encrypted-local-shared-package-state-unsupported"
    );
    let capture = SharedStateCapture {
        snapshot: plaintext.snapshot,
    };
    capture.validate()?;
    ensure!(
        manifest.prefix_count == u64::try_from(capture.snapshot.tables.changes.len())?
            && manifest.attachment_metadata_count
                == u64::try_from(capture.snapshot.tables.task_attachments.len())?,
        "error encrypted-local-shared-package-manifest-domain-mismatch"
    );
    Ok(capture)
}

fn validate_package_identity(
    package: &EncryptedLocalSharedStatePackage,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
) -> Result<()> {
    ensure!(
        package.context == context && package.stream_id == stream_id,
        "error encrypted-local-shared-package-context-mismatch"
    );
    Ok(())
}

fn validate_manifest(
    package: &EncryptedLocalSharedStatePackage,
    manifest: &PackageManifest,
) -> Result<()> {
    ensure!(
        manifest.format == "aven-encrypted-local-shared-state-manifest"
            && manifest.version == PACKAGE_FORMAT_VERSION
            && manifest.suite == PACKAGE_SUITE,
        "error encrypted-local-shared-package-manifest-unsupported"
    );
    ensure!(
        manifest.vault_id == hex::encode(package.context.vault_id)
            && manifest.stream_id == hex::encode(package.stream_id)
            && manifest.generation_id == hex::encode(package.context.generation_id)
            && manifest.candidate_id == package.candidate_id
            && !manifest.attachment_bytes_included,
        "error encrypted-local-shared-package-manifest-context-mismatch"
    );
    let expected = manifest_artifact(&package.state)?;
    ensure!(
        manifest.state.total_plaintext_bytes == expected.total_plaintext_bytes
            && manifest.state.chunk_count == expected.chunk_count
            && manifest.state.aggregate_commitment == expected.aggregate_commitment
            && manifest.state.chunks.len() == expected.chunks.len()
            && manifest
                .state
                .chunks
                .iter()
                .zip(expected.chunks.iter())
                .all(|(left, right)| left.record_length == right.record_length
                    && left.record_commitment == right.record_commitment),
        "error encrypted-local-shared-package-manifest-state-mismatch"
    );
    Ok(())
}

fn manifest_artifact(artifact: &EncryptedArtifact) -> Result<ManifestArtifact> {
    Ok(ManifestArtifact {
        total_plaintext_bytes: artifact.total_plaintext_bytes,
        chunk_count: u32::try_from(artifact.chunks.len())?,
        aggregate_commitment: hex::encode(artifact.aggregate_commitment),
        chunks: artifact
            .chunks
            .iter()
            .map(|chunk| {
                Ok(ManifestChunk {
                    record_length: u64::try_from(chunk.record.len())?,
                    record_commitment: hex::encode(chunk.record_commitment),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    })
}

fn derive_class_key(
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

fn cce(label: &[u8], fields: &[&[u8]]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    push_bytes(&mut output, label)?;
    for field in fields {
        push_bytes(&mut output, field)?;
    }
    Ok(output)
}

fn encrypt_artifact(
    plaintext: &[u8],
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    candidate: [u8; 32],
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
            candidate,
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

fn decrypt_artifact(
    artifact: &EncryptedArtifact,
    context: LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    candidate: [u8; 32],
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
        aggregate_commitment(&artifact.chunks) == artifact.aggregate_commitment,
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
            candidate,
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
    candidate: [u8; 32],
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
    push_bytes(&mut header, &candidate)?;
    header.push(2);
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
    candidate: [u8; 32],
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
        context, stream_id, candidate, class, index, count, total, nonce,
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
    .bind(PACKAGE_FORMAT_VERSION)
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
    Ok(())
}

async fn load_package(
    conn: &mut sqlx::SqliteConnection,
    candidate_id: &str,
) -> Result<Option<EncryptedLocalSharedStatePackage>> {
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
        return Ok(None);
    };
    ensure!(
        format_version == PACKAGE_FORMAT_VERSION && suite == PACKAGE_SUITE,
        "error encrypted-local-shared-package-unsupported"
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
    Ok(Some(EncryptedLocalSharedStatePackage {
        candidate_id: candidate_id.to_string(),
        stream_id: decode_context_id(&stream_id, "stream")?,
        context,
        state,
        manifest,
    }))
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
        aggregate_commitment(&artifact.chunks) == artifact.aggregate_commitment,
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
