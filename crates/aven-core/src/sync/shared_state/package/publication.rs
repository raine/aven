//! Bounded publication-package codec and keyless completeness checks.
//!
//! Persistence belongs to the never-dispatched database package owner. The capture
//! candidate ID is the bootstrap ID, not a freshly minted specimen identity.
//! This codec provides no authorization: membership predecessor bytes are context
//! only. `seed_claim::Publication` authenticates that context and the descriptor;
//! `bootstrap_staging` owns server publication.
//!
//! # Profile 1 byte contract
//!
//! Integers are unsigned big-endian. Byte strings use a U64 length. IDs in
//! descriptor/chunk context are raw 32 bytes; catalog domain IDs are nonempty
//! UTF-8 byte strings, at most 256 bytes. Flags are exactly 0 or 1. No compression.
//! All parsers reject trailing bytes. SHA-256 commits exact bytes, not decoded JSON.
//!
//! * Descriptor: `AVBP || U16(2) || U8(1 suite)`, vault, stream, generation,
//!   capture bootstrap, predecessor membership commitment, U64 prefix count,
//!   three catalog declarations in class order, then manifest artifact descriptor.
//!   At most `MAX_DESCRIPTOR_BYTES`. Other versions are refused as unsupported,
//!   never reinterpreted.
//! * Declaration: U8 class, U64 record count, U64 byte length, aggregate SHA-256,
//!   then one SHA-256 per transfer slice. Slices are contiguous 1 MiB byte ranges
//!   with a final remainder; their count follows from the length. A hash-valid
//!   slice proves nothing about its catalog until the complete bytes match the
//!   aggregate and decode canonically.
//! * Catalog: `AVBC || U16(1) || U8(class) || U64(record count)`, followed by
//!   length-prefixed records. Class 1 has one U8(1 state) + artifact descriptor;
//!   class 2 records are U64 rank + ID, ordered densely from 1. Class 3 groups
//!   object, parent, reference records, with U8 discriminators 1, 2, 3.
//! * Artifact: U64 plaintext total, aggregate SHA-256, U64 chunk count, followed
//!   by U64 index, U64 record length, record SHA-256, raw nonce[24] per chunk.
//!   The recipe reconstructs the existing 198-byte authenticated chunk header.
//! * Object: ID[32], U8 selection (1 current, 2 extra), artifact. Both selections
//!   require bytes. Sort by raw ID. Context is inherited from the descriptor.
//! * Parent: workspace ID, task ID, deleted flag, protected flag, version-present
//!   flag and optional version ID. Sort by (workspace, task) UTF-8 bytes.
//! * Reference: workspace ID, task ID, reference ID, deleted flag, mapped flag,
//!   optional object[32]. Sort by (workspace, reference). Unmapped references
//!   preserve unavailable metadata and create no byte or download obligation.
//! * Encrypted state: `AVBD || U16(1)`, then every section in `domain.rs` order.
//!   Each section has U16 type, U64 count and length-prefixed strict JSON rows,
//!   sorted by complete encoded payload bytes. Struct field order, explicit nulls,
//!   UTF-8, and serde_json escaping are canonical. Unknown/duplicate/missing
//!   fields, alternate encodings and duplicate rows are rejected. Rows can span
//!   transport chunks. Export versions/defaults and device inventory flags are
//!   not wire fields; internal conversions only adapt shared domain validators.
//! * Encrypted manifest: `AVBM || U16(1)`, descriptor through catalog declarations
//!   (excluding its own artifact), U16 section count, then U16 section, U64 row
//!   count, U64 encoded section length. This binds domain totals and catalogs
//!   without a circular ciphertext commitment.
//!
//! Hard codec resource caps: 256 MiB state, 1 MiB manifest, 16 MiB per catalog,
//! 1,000,000 total domain rows, 1,000,000 records per catalog, 1,024 selected
//! images, 25 MiB per image and 256 MiB image plaintext in aggregate. Ciphertext
//! includes exactly 222 bytes overhead per chunk. Prefix counts also leave room
//! for a positive signed-64-bit tail position. These are refusal limits, not
//! domain validity rules or production transport/reservation policy. This
//! in-memory codec has bounded multiple-copy overhead and is not a streaming
//! installer or a measured mobile resource profile.

pub(crate) use aven_protocol::artifact::catalog;
pub(crate) use aven_protocol::artifact::codec;
mod domain;
#[cfg(feature = "test-support")]
pub(crate) mod fuzz;

pub use codec::MAX_DESCRIPTOR_BYTES;

/// Version of the encrypted AVBD domain, independent of plaintext sync/export.
pub const DOMAIN_VERSION: u32 = 1;
pub mod download;
mod projection;
pub(crate) mod staging;

use super::super::{NeverDispatchedLocalSharedCapture, SharedStateCapture};
use super::{self as crypto, LocalSharedStatePackageKey};
use catalog::{Artifact, Declaration, Image, Images};
use codec::*;
use zeroize::Zeroizing;

pub use aven_protocol::artifact::Error;
use aven_protocol::artifact::Result;

/// Exact upload components. Catalog array order is data, prefix, images.
/// Records within artifacts are in canonical index order, images in object order.
/// Clear bytes contain only the permitted metadata; encrypted bytes are opaque.
#[derive(Clone, PartialEq, Eq)]
pub struct Package {
    pub descriptor: Vec<u8>,
    pub catalogs: [Vec<u8>; 3],
    pub state: Vec<Vec<u8>>,
    pub manifest: Vec<Vec<u8>>,
    pub images: Vec<ImageRecords>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ImageRecords {
    pub object_id: [u8; 32],
    pub records: Vec<Vec<u8>>,
}

/// Structural completeness only. It does not authenticate membership or prove
/// domain correctness, durable storage, quota admission, or permission to publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completeness {
    pub prefix_count: u64,
    pub image_count: u64,
    pub ciphertext_bytes: u64,
}

use aven_protocol::artifact::{Descriptor, decode_state_catalog, state_catalog};

fn manifest_plaintext(d: &Descriptor, stats: &domain::Stats) -> Vec<u8> {
    let mut out = b"AVBM\0\x01".to_vec();
    out.extend_from_slice(&d.binding());
    out.extend_from_slice(&(domain::SECTIONS as u16).to_be_bytes());
    for (index, (count, length)) in stats.iter().enumerate() {
        out.extend_from_slice(&(index as u16 + 1).to_be_bytes());
        u64_bytes(&mut out, *count);
        u64_bytes(&mut out, *length);
    }
    out
}

/// Encrypts the domain encoding as it is produced, hashing the plaintext so
/// the finished ciphertext can be checked against exactly these bytes.
struct StateOut {
    writer: crypto::ArtifactWriter,
    digest: sha2::Sha256,
    len: usize,
    failed: bool,
}

impl domain::Out for StateOut {
    fn begin(&mut self, total: usize) {
        self.failed |= self.writer.begin(total).is_err();
    }
    fn len(&self) -> usize {
        self.len
    }
    fn put(&mut self, bytes: &[u8]) {
        self.len += bytes.len();
        sha2::Digest::update(&mut self.digest, bytes);
        self.failed = self.failed || self.writer.write(bytes).is_err();
    }
}

/// One authenticated selected image without ownership of its ciphertext records.
pub(super) struct ImageSummary {
    pub(super) sha256: String,
    pub(super) object_id: [u8; 32],
    pub(super) artifact: Artifact,
}

impl ImageSummary {
    pub(super) fn authenticated(
        image: &crypto::EncryptedImage,
        context: crypto::LocalSharedStatePackageContext,
        stream: [u8; 32],
        key: &LocalSharedStatePackageKey,
    ) -> Result<Self> {
        let image_key = crypto::derive_image_key(key, context, image.object_id)
            .map_err(|_| Error::Authentication)?;
        let reader = crypto::ArtifactReader::new(
            &image.artifact,
            context,
            stream,
            image.object_id,
            1,
            0,
            &image_key,
            size(IMAGE_LIMIT)?,
        )
        .map_err(|_| Error::Authentication)?;
        valid(hex::encode(hash_plaintext(reader)?) == image.sha256)?;
        Ok(Self {
            sha256: image.sha256.clone(),
            object_id: image.object_id,
            artifact: Artifact::from_encrypted(&image.artifact)?,
        })
    }
}

/// Constructs tentative metadata only for the durable owner's first freeze.
/// Image ciphertext has already been authenticated while each image was resident.
pub(super) fn build_metadata(
    capture: &NeverDispatchedLocalSharedCapture,
    context: crypto::LocalSharedStatePackageContext,
    images: &[ImageSummary],
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<(Package, AttachmentIndex)> {
    let stream_id =
        crypto::decode_context_id(capture.stream_id(), "stream").map_err(|_| Error::Invalid)?;
    let bootstrap = crypto::decode_context_id(capture.candidate_id(), "candidate")
        .map_err(|_| Error::Invalid)?;
    let objects = images
        .iter()
        .map(|image| (image.sha256.as_str(), image.object_id))
        .collect::<std::collections::HashMap<_, _>>();
    let mappings = capture
        .images
        .iter()
        .map(|r| domain::Mapping {
            sha256: r.sha256.clone(),
            classification: r.classification.clone(),
            object: objects.get(r.sha256.as_str()).copied(),
        })
        .collect::<Vec<_>>();
    let state_key = crypto::derive_bootstrap_class_key(key, context, stream_id, bootstrap, 1)
        .map_err(|_| Error::Authentication)?;
    let mut out = StateOut {
        writer: crypto::ArtifactWriter::new(context, stream_id, bootstrap, 2, 1, &state_key)
            .map_err(|_| Error::Authentication)?,
        digest: sha2::Digest::new(),
        len: 0,
        failed: false,
    };
    let stats = domain::encode_into(&mut out, &capture.capture.snapshot.tables, &mappings)?;
    valid(!out.failed)?;
    let encoded = <[u8; 32]>::from(sha2::Digest::finalize(out.digest));
    let state = out.writer.finish().map_err(|_| Error::Authentication)?;
    let objects = images
        .iter()
        .map(|image| {
            let mapping = mappings
                .iter()
                .find(|m| m.object == Some(image.object_id))
                .ok_or(Error::Invalid)?;
            Ok(Image {
                id: image.object_id,
                selection: if mapping.classification == "current_selected" {
                    1
                } else {
                    2
                },
                artifact: image.artifact.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let image_catalog = projection::images(
        &capture.capture.snapshot.tables,
        &mappings,
        Images {
            objects,
            parents: Vec::new(),
            references: Vec::new(),
        },
    )?;
    let prefix = projection::prefix(&capture.capture.snapshot.tables)?;
    let catalogs = [
        state_catalog(&Artifact::from_encrypted(&state)?)?,
        catalog::prefix_encode(&prefix)?,
        image_catalog.encode()?,
    ];
    let mut d = Descriptor {
        vault: context.vault_id,
        stream: stream_id,
        generation: context.generation_id,
        bootstrap,
        membership: membership_predecessor,
        prefix: number(prefix.len())?,
        catalogs: [
            Declaration::new(&catalogs[0], 1)?,
            Declaration::new(&catalogs[1], 2)?,
            Declaration::new(&catalogs[2], 3)?,
        ],
        manifest: Artifact {
            total: 0,
            aggregate: [0; 32],
            chunks: Vec::new(),
        },
    };
    let manifest_key = crypto::derive_bootstrap_class_key(key, context, stream_id, bootstrap, 2)
        .map_err(|_| Error::Authentication)?;
    let manifest = crypto::encrypt_artifact(
        &manifest_plaintext(&d, &stats),
        context,
        stream_id,
        bootstrap,
        2,
        2,
        &manifest_key,
    )
    .map_err(|_| Error::Authentication)?;
    d.manifest = Artifact::from_encrypted(&manifest)?;
    let package = Package {
        descriptor: d.encode()?,
        catalogs,
        state: state.into_records(),
        manifest: manifest.into_records(),
        images: Vec::new(),
    };
    let index = check_capture(
        &package,
        capture,
        key,
        membership_predecessor,
        StateCheck::Encoded {
            digest: encoded,
            mappings: &mappings,
            stats: &stats,
        },
        false,
    )?;
    Ok((package, index))
}

/// Full-package adapter used by codec and persistence tests.
#[cfg(test)]
pub(super) fn build(
    capture: &NeverDispatchedLocalSharedCapture,
    context: crypto::LocalSharedStatePackageContext,
    images: Vec<crypto::EncryptedImage>,
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<(Package, AttachmentIndex)> {
    let stream =
        crypto::decode_context_id(capture.stream_id(), "stream").map_err(|_| Error::Invalid)?;
    let summaries = images
        .iter()
        .map(|image| ImageSummary::authenticated(image, context, stream, key))
        .collect::<Result<Vec<_>>>()?;
    let (mut package, _) =
        build_metadata(capture, context, &summaries, key, membership_predecessor)?;
    package.images = images
        .into_iter()
        .map(|image| ImageRecords {
            object_id: image.object_id,
            records: image.artifact.into_records(),
        })
        .collect();
    package.images.sort_by_key(|image| image.object_id);
    let index = authenticate_capture(&package, capture, key, membership_predecessor)?;
    Ok((package, index))
}

/// Authenticates all domain, manifest and image bytes under the supplied key and
/// expected context. This does not establish membership authorization.
pub fn authenticate(
    package: &Package,
    key: &LocalSharedStatePackageKey,
    context: crypto::LocalSharedStatePackageContext,
    stream_id: [u8; 32],
    bootstrap_id: [u8; 32],
    membership_predecessor: [u8; 32],
) -> Result<()> {
    let d = Descriptor::decode(&package.descriptor)?;
    valid(
        d.context() == context
            && d.stream == stream_id
            && d.bootstrap == bootstrap_id
            && d.membership == membership_predecessor,
    )?;
    decrypt_domain(package, key)?;
    Ok(())
}

pub(super) fn context_and_membership(
    descriptor: &[u8],
) -> Result<(crypto::LocalSharedStatePackageContext, [u8; 32])> {
    let d = Descriptor::decode(descriptor)?;
    Ok((d.context(), d.membership))
}

/// Checks exact clear commitments, framing, canonical coverage and all selected
/// image-byte obligations without a key. Catalogs are aggregate-verified before
/// any records are trusted. No partial catalog can establish completeness.
pub fn validate_keyless(package: &Package) -> Result<Completeness> {
    keyless(package).map(|(completeness, _, _)| completeness)
}

/// [`validate_keyless`], also returning the verified descriptor and image
/// catalog.
fn keyless(package: &Package) -> Result<(Completeness, Descriptor, Images)> {
    let metadata = download::MetadataView::from(package);
    let (d, images) = validate_metadata(&metadata)?;
    valid(images.objects.len() == package.images.len())?;
    for (object, records) in images.objects.iter().zip(&package.images) {
        valid(object.id == records.object_id)?;
        object
            .artifact
            .verify(&records.records, d.context(), d.stream, object.id, 1, 0)?;
    }
    let ciphertext_bytes = std::iter::once(&package.state)
        .chain(std::iter::once(&package.manifest))
        .chain(package.images.iter().map(|i| &i.records))
        .flatten()
        .try_fold(0, |total, record| add(total, number(record.len())?))?;
    let completeness = Completeness {
        prefix_count: d.prefix,
        image_count: number(images.objects.len())?,
        ciphertext_bytes,
    };
    Ok((completeness, d, images))
}

fn validate_metadata(metadata: &download::MetadataView<'_>) -> Result<(Descriptor, Images)> {
    let d = Descriptor::decode(metadata.descriptor)?;
    for (i, bytes) in metadata.catalogs.iter().enumerate() {
        d.catalogs[i].verify(bytes, i as u8 + 1)?;
    }
    let state = decode_state_catalog(&metadata.catalogs[0])?;
    catalog::prefix_decode(&metadata.catalogs[1], d.prefix)?;
    let images = Images::decode(&metadata.catalogs[2])?;
    state.verify(metadata.state, d.context(), d.stream, d.bootstrap, 2, 1)?;
    d.manifest
        .verify(metadata.manifest, d.context(), d.stream, d.bootstrap, 2, 2)?;
    Ok((d, images))
}

fn decrypt_domain(
    package: &Package,
    key: &LocalSharedStatePackageKey,
) -> Result<(SharedStateCapture, Vec<domain::Mapping>)> {
    decrypt_domain_images(package, key, |_, _| {})
}

fn decrypt_domain_images(
    package: &Package,
    key: &LocalSharedStatePackageKey,
    mut accept_image: impl FnMut(&str, Zeroizing<Vec<u8>>),
) -> Result<(SharedStateCapture, Vec<domain::Mapping>)> {
    validate_keyless(package)?;
    let metadata = download::MetadataView::from(package);
    let (capture, mappings) = decrypt_metadata(&metadata, key)?;
    let (d, images) = validate_metadata(&metadata)?;
    for (object, records) in images.objects.iter().zip(&package.images) {
        let mapping = mappings
            .iter()
            .find(|m| m.object == Some(object.id))
            .ok_or(Error::Invalid)?;
        let image_key = crypto::derive_image_key(key, d.context(), object.id)
            .map_err(|_| Error::Authentication)?;
        let bytes = Zeroizing::new(
            crypto::decrypt_artifact(
                &object.artifact.encrypted(&records.records),
                d.context(),
                d.stream,
                object.id,
                1,
                0,
                &image_key,
                size(IMAGE_LIMIT)?,
            )
            .map_err(|_| Error::Authentication)?,
        );
        valid(hex::encode(crate::sync::codec::hash(&bytes)) == mapping.sha256)?;
        accept_image(&mapping.sha256, bytes);
    }
    Ok((capture, mappings))
}

fn decrypt_metadata(
    metadata: &download::MetadataView<'_>,
    key: &LocalSharedStatePackageKey,
) -> Result<(SharedStateCapture, Vec<domain::Mapping>)> {
    validate_metadata(metadata)?;
    #[cfg(test)]
    crate::sync::shared_state::counters::keyed_pass();
    let d = Descriptor::decode(metadata.descriptor)?;
    let state = decode_state_catalog(&metadata.catalogs[0])?;
    let state_key = crypto::derive_bootstrap_class_key(key, d.context(), d.stream, d.bootstrap, 1)
        .map_err(|_| Error::Authentication)?;
    let plaintext = Zeroizing::new(
        crypto::decrypt_artifact(
            &state.encrypted(metadata.state),
            d.context(),
            d.stream,
            d.bootstrap,
            2,
            1,
            &state_key,
            size(STATE_LIMIT)?,
        )
        .map_err(|_| Error::Authentication)?,
    );
    let (tables, mappings, stats) = domain::decode(&plaintext)?;
    let manifest_key =
        crypto::derive_bootstrap_class_key(key, d.context(), d.stream, d.bootstrap, 2)
            .map_err(|_| Error::Authentication)?;
    let manifest = Zeroizing::new(
        crypto::decrypt_artifact(
            &d.manifest.encrypted(metadata.manifest),
            d.context(),
            d.stream,
            d.bootstrap,
            2,
            2,
            &manifest_key,
            size(CHUNK)?,
        )
        .map_err(|_| Error::Authentication)?,
    );
    valid(*manifest == manifest_plaintext(&d, &stats))?;
    let capture = capture(tables);
    capture.validate().map_err(|_| Error::Invalid)?;
    valid(
        projection::prefix(&capture.snapshot.tables)?
            == catalog::prefix_decode(&metadata.catalogs[1], d.prefix)?,
    )?;
    let images = Images::decode(&metadata.catalogs[2])?;
    let expected = projection::images(
        &capture.snapshot.tables,
        &mappings,
        Images {
            objects: images.objects.clone(),
            parents: Vec::new(),
            references: Vec::new(),
        },
    )?;
    valid(images == expected)?;
    Ok((capture, mappings))
}

/// Decoded domain tables as a capture for the shared domain validators.
fn capture(tables: crate::data_safety::export_types::ExportTables) -> SharedStateCapture {
    // Select the internal validation adapter version, not a wire version.
    let version = if tables.shared_history_provenance.is_empty() {
        3
    } else {
        4
    };
    SharedStateCapture {
        snapshot: crate::data_safety::export_types::AvenExport {
            format: "aven-export".into(),
            version,
            exported_at: String::new(),
            schema_version: 0,
            blobs_included: false,
            tables,
        },
    }
}

/// Authenticates and decrypts the complete package into installable state.
pub(super) fn decrypt_capture(
    package: &Package,
    key: &LocalSharedStatePackageKey,
) -> Result<SharedStateCapture> {
    Ok(decrypt_domain(package, key)?.0)
}

/// Returns every selected image's private hash and authenticated plaintext.
#[cfg(test)]
pub(super) fn decrypt_images(
    package: &Package,
    key: &LocalSharedStatePackageKey,
) -> Result<Vec<(String, Zeroizing<Vec<u8>>)>> {
    let mut images = Vec::new();
    decrypt_domain_images(package, key, |sha256, bytes| {
        images.push((sha256.to_string(), bytes));
    })?;
    Ok(images)
}

/// Client-side check against the immutable source, not live rows. Also checks
/// encrypted manifest/catalog agreement and private image hashes. This is not
/// join installation and does not establish any trusted membership anchor.
pub fn validate_against_capture(
    package: &Package,
    capture: &NeverDispatchedLocalSharedCapture,
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<()> {
    authenticate_capture(package, capture, key, membership_predecessor).map(drop)
}

/// The one keyed pass over a frozen package. The decrypted state must equal
/// the canonical encoding of the capture under the package's own private
/// mappings, so the state is never decoded into a second copy of the tables.
/// The state and images are decrypted one chunk at a time.
pub(crate) fn authenticate_capture(
    package: &Package,
    capture: &NeverDispatchedLocalSharedCapture,
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<AttachmentIndex> {
    check_capture(
        package,
        capture,
        key,
        membership_predecessor,
        StateCheck::Capture,
        true,
    )
}

/// How [`check_capture`] establishes that the decrypted state is the
/// capture's canonical encoding.
enum StateCheck<'a> {
    /// Re-encode the capture under the mappings read from the state and
    /// compare it with the state as both are produced.
    Capture,
    /// The state was encrypted from one encoding pass over this capture, which
    /// wrote these mappings, gave these section stats and hashed to `digest`.
    /// The state must decrypt to bytes with that hash.
    Encoded {
        digest: [u8; 32],
        mappings: &'a [domain::Mapping],
        stats: &'a domain::Stats,
    },
}

/// Hands out a state reader's plaintext to the domain decoder.
struct Plain<'r, 'a>(&'r mut crypto::ArtifactReader<'a>);

impl domain::In for Plain<'_, '_> {
    fn remaining(&self) -> u64 {
        self.0.remaining()
    }
    fn read(&mut self, into: &mut [u8]) -> Result<()> {
        let mut filled = 0;
        while filled < into.len() {
            let piece = self
                .0
                .take(into.len() - filled)
                .map_err(|_| Error::Authentication)?;
            valid(!piece.is_empty())?;
            into[filled..filled + piece.len()].copy_from_slice(piece);
            filled += piece.len();
        }
        Ok(())
    }
    fn skip(&mut self, n: u64) -> Result<()> {
        let mut left = size(n)?;
        while left > 0 {
            let piece = self.0.take(left).map_err(|_| Error::Authentication)?;
            valid(!piece.is_empty())?;
            left -= piece.len();
        }
        Ok(())
    }
}

/// Compares the encoding with a state reader's plaintext as both are
/// produced, so neither is ever held whole.
struct PlainMatcher<'r, 'a> {
    reader: &'r mut crypto::ArtifactReader<'a>,
    written: usize,
    equal: bool,
    failed: bool,
}

impl domain::Out for PlainMatcher<'_, '_> {
    fn begin(&mut self, total: usize) {
        self.equal &= total as u64 == self.reader.remaining();
    }
    fn len(&self) -> usize {
        self.written
    }
    fn put(&mut self, mut bytes: &[u8]) {
        self.written += bytes.len();
        while self.equal && !bytes.is_empty() {
            match self.reader.take(bytes.len()) {
                Ok(piece) if !piece.is_empty() => {
                    self.equal = piece == &bytes[..piece.len()];
                    bytes = &bytes[piece.len()..];
                }
                Ok(_) => self.equal = false,
                Err(_) => {
                    self.failed = true;
                    self.equal = false;
                }
            }
        }
    }
}

/// Encodes the capture tables under `mappings`, comparing the encoding with
/// the whole of the reader's plaintext as both are produced.
fn match_state(
    mut reader: crypto::ArtifactReader<'_>,
    tables: &crate::data_safety::export_types::ExportTables,
    mappings: &[domain::Mapping],
) -> Result<domain::Stats> {
    let mut matcher = PlainMatcher {
        reader: &mut reader,
        written: 0,
        equal: true,
        failed: false,
    };
    let stats = domain::encode_into(&mut matcher, tables, mappings)?;
    if matcher.failed {
        return Err(Error::Authentication);
    }
    valid(matcher.equal && reader.remaining() == 0)?;
    reader.finish().map_err(|_| Error::Invalid)?;
    Ok(stats)
}

/// The SHA-256 of an artifact's complete plaintext.
fn hash_plaintext(mut reader: crypto::ArtifactReader<'_>) -> Result<[u8; 32]> {
    let mut hash = <sha2::Sha256 as sha2::Digest>::new();
    loop {
        let piece = reader.take(usize::MAX).map_err(|_| Error::Authentication)?;
        if piece.is_empty() {
            break;
        }
        sha2::Digest::update(&mut hash, piece);
    }
    reader.finish().map_err(|_| Error::Invalid)?;
    Ok(sha2::Digest::finalize(hash).into())
}

fn state_reader<'a>(
    d: &Descriptor,
    state: &'a crypto::EncryptedArtifact<'a>,
    key: &LocalSharedStatePackageKey,
) -> Result<crypto::ArtifactReader<'a>> {
    let state_key = crypto::derive_bootstrap_class_key(key, d.context(), d.stream, d.bootstrap, 1)
        .map_err(|_| Error::Authentication)?;
    crypto::ArtifactReader::new(
        state,
        d.context(),
        d.stream,
        d.bootstrap,
        2,
        1,
        &state_key,
        size(STATE_LIMIT)?,
    )
    .map_err(|_| Error::Authentication)
}

/// Authenticates every byte of `package` against the independent capture:
/// the state per `check`, then the manifest, the prefix and image catalogs,
/// the private mappings, and each image's plaintext hash.
fn check_capture(
    package: &Package,
    capture: &NeverDispatchedLocalSharedCapture,
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
    check: StateCheck<'_>,
    authenticate_images: bool,
) -> Result<AttachmentIndex> {
    #[cfg(test)]
    crate::sync::shared_state::counters::keyed_pass();
    let metadata = download::MetadataView::from(package);
    let (d, images) = validate_metadata(&metadata)?;
    valid(
        d.membership == membership_predecessor
            && capture.stream_id() == hex::encode(d.stream)
            && capture.candidate_id() == hex::encode(d.bootstrap),
    )?;
    let state = decode_state_catalog(&metadata.catalogs[0])?;
    let state = state.encrypted(metadata.state);
    let tables = &capture.capture.snapshot.tables;
    let decoded;
    let computed;
    let (mappings, stats) = match check {
        StateCheck::Capture => {
            decoded = domain::decode_mappings(&mut Plain(&mut state_reader(&d, &state, key)?))?;
            computed = match_state(state_reader(&d, &state, key)?, tables, &decoded)?;
            (&decoded[..], &computed)
        }
        StateCheck::Encoded {
            digest,
            mappings,
            stats,
        } => {
            valid(hash_plaintext(state_reader(&d, &state, key)?)? == digest)?;
            (mappings, stats)
        }
    };
    let manifest_key =
        crypto::derive_bootstrap_class_key(key, d.context(), d.stream, d.bootstrap, 2)
            .map_err(|_| Error::Authentication)?;
    let manifest = Zeroizing::new(
        crypto::decrypt_artifact(
            &d.manifest.encrypted(metadata.manifest),
            d.context(),
            d.stream,
            d.bootstrap,
            2,
            2,
            &manifest_key,
            size(CHUNK)?,
        )
        .map_err(|_| Error::Authentication)?,
    );
    valid(*manifest == manifest_plaintext(&d, stats))?;
    valid(projection::prefix(tables)? == catalog::prefix_decode(&metadata.catalogs[1], d.prefix)?)?;
    let expected_images = projection::images(
        tables,
        mappings,
        Images {
            objects: images.objects.clone(),
            parents: Vec::new(),
            references: Vec::new(),
        },
    )?;
    valid(images == expected_images)?;
    valid(mappings.len() == capture.images.len())?;
    let captured_images = capture
        .images
        .iter()
        .map(|r| (r.sha256.as_str(), r.classification.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    for mapping in mappings {
        valid(
            captured_images.get(mapping.sha256.as_str()).copied()
                == Some(mapping.classification.as_str()),
        )?;
    }
    if authenticate_images {
        valid(images.objects.len() == package.images.len())?;
        for (object, records) in images.objects.iter().zip(&package.images) {
            valid(object.id == records.object_id)?;
            let mapping = mappings
                .iter()
                .find(|m| m.object == Some(object.id))
                .ok_or(Error::Invalid)?;
            authenticate_image(
                d.context(),
                d.stream,
                object,
                &mapping.sha256,
                &records.records,
                key,
            )?;
        }
    }
    index_from_mappings(&metadata, mappings).map_err(|_| Error::Invalid)
}

/// Authenticates metadata against the capture. Image records must be checked
/// separately with [`authenticate_indexed_image`] in the same snapshot.
pub(super) fn authenticate_metadata_capture(
    package: &Package,
    capture: &NeverDispatchedLocalSharedCapture,
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<AttachmentIndex> {
    check_capture(
        package,
        capture,
        key,
        membership_predecessor,
        StateCheck::Capture,
        false,
    )
}

fn authenticate_image(
    context: crypto::LocalSharedStatePackageContext,
    stream: [u8; 32],
    object: &Image,
    sha256: &str,
    records: &[Vec<u8>],
    key: &LocalSharedStatePackageKey,
) -> Result<()> {
    let image_key =
        crypto::derive_image_key(key, context, object.id).map_err(|_| Error::Authentication)?;
    let artifact = object.artifact.encrypted(records);
    let reader = crypto::ArtifactReader::new(
        &artifact,
        context,
        stream,
        object.id,
        1,
        0,
        &image_key,
        size(IMAGE_LIMIT)?,
    )
    .map_err(|_| Error::Authentication)?;
    valid(hex::encode(hash_plaintext(reader)?) == sha256)
}

pub(super) fn authenticate_indexed_image(
    index: &AttachmentIndex,
    object_id: [u8; 32],
    sha256: &str,
    records: &[Vec<u8>],
    key: &LocalSharedStatePackageKey,
) -> Result<()> {
    let (object, expected) = index
        .objects
        .iter()
        .find(|(object, _)| object.id == object_id)
        .ok_or(Error::Invalid)?;
    valid(expected == sha256)?;
    authenticate_image(index.context, index.stream, object, sha256, records, key)
}

/// The decoding validator [`authenticate_capture`] replaced, kept so tests can
/// show both agree.
#[cfg(test)]
pub(crate) fn validate_against_capture_decoding(
    package: &Package,
    capture: &NeverDispatchedLocalSharedCapture,
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<()> {
    let d = Descriptor::decode(&package.descriptor)?;
    valid(
        d.membership == membership_predecessor
            && capture.stream_id() == hex::encode(d.stream)
            && capture.candidate_id() == hex::encode(d.bootstrap),
    )?;
    let (decoded, mappings) = decrypt_domain(package, key)?;
    valid(
        domain::encode(&decoded.snapshot.tables, &mappings)?.0
            == domain::encode(&capture.capture.snapshot.tables, &mappings)?.0,
    )?;
    valid(mappings.len() == capture.images.len())?;
    let captured_images = capture
        .images
        .iter()
        .map(|r| (r.sha256.as_str(), r.classification.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    for mapping in &mappings {
        valid(
            captured_images.get(mapping.sha256.as_str()).copied()
                == Some(mapping.classification.as_str()),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

/// Rewrites one catalog commitment to match edited catalog bytes, so tests can
/// exercise hash-valid catalogs that are otherwise wrong.
#[cfg(test)]
pub(crate) fn recommit_catalog(package: &mut Package, index: usize) {
    let mut d = Descriptor::decode(&package.descriptor).unwrap();
    d.catalogs[index] = Declaration::new(&package.catalogs[index], index as u8 + 1).unwrap();
    package.descriptor = d.encode().unwrap();
}

/// Replaces the prefix catalog with `count` synthetic rows and recommits the
/// descriptor. Keyless checks pass; keyed validation against the snapshot
/// does not.
#[cfg(test)]
pub(crate) fn replace_prefix_catalog(package: &mut Package, count: u64) {
    let rows = (1..=count)
        .map(|rank| (rank, format!("{rank:026}")))
        .collect::<Vec<_>>();
    package.catalogs[1] = catalog::prefix_encode(&rows).unwrap();
    let mut d = Descriptor::decode(&package.descriptor).unwrap();
    d.prefix = count;
    package.descriptor = d.encode().unwrap();
    recommit_catalog(package, 1);
}

#[derive(Clone)]
pub(crate) struct AttachmentIndex {
    pub context: crypto::LocalSharedStatePackageContext,
    pub stream: [u8; 32],
    pub objects: Vec<(catalog::Image, String)>,
    pub references: Vec<catalog::Reference>,
}

/// The caller receives mappings only after complete public/private authentication.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn attachment_index(
    package: &Package,
    key: &LocalSharedStatePackageKey,
) -> anyhow::Result<AttachmentIndex> {
    let (_, mappings) = decrypt_domain_images(package, key, |_, _| {})?;
    index_from_mappings(&download::MetadataView::from(package), &mappings)
}

/// Rebuilds the index of an authenticated freeze from its public catalogs and
/// its recorded `(object ID, sha256)` pairs, without decrypting. Every catalog
/// object must be recorded exactly once, and nothing else.
pub(crate) fn index_from_objects(
    descriptor: &[u8],
    catalogs: &[Vec<u8>; 3],
    recorded: &[([u8; 32], String)],
) -> anyhow::Result<AttachmentIndex> {
    use anyhow::Context as _;
    let declaration = staging::DeclarationView::decode(descriptor)?;
    for (class, bytes) in catalogs.iter().enumerate() {
        declaration.catalog(class, bytes)?;
    }
    let descriptor = Descriptor::decode(descriptor)?;
    let images = Images::decode(&catalogs[2])?;
    anyhow::ensure!(
        recorded.len() == images.objects.len(),
        "error seed-capture-changed"
    );
    let objects = images
        .objects
        .into_iter()
        .map(|image| {
            let sha256 = recorded
                .iter()
                .find(|(id, _)| *id == image.id)
                .map(|(_, sha256)| sha256.clone())
                .context("error seed-capture-changed")?;
            Ok((image, sha256))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(AttachmentIndex {
        context: descriptor.context(),
        stream: descriptor.stream,
        objects,
        references: images.references,
    })
}

fn index_from_mappings(
    metadata: &download::MetadataView<'_>,
    mappings: &[domain::Mapping],
) -> anyhow::Result<AttachmentIndex> {
    let descriptor = Descriptor::decode(metadata.descriptor)?;
    let images = Images::decode(&metadata.catalogs[2])?;
    let objects = images
        .objects
        .into_iter()
        .map(|image| {
            let mapping = mappings
                .iter()
                .find(|m| m.object == Some(image.id))
                .ok_or(Error::Invalid)?;
            Ok((image, mapping.sha256.clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(AttachmentIndex {
        context: descriptor.context(),
        stream: descriptor.stream,
        objects,
        references: images.references,
    })
}
