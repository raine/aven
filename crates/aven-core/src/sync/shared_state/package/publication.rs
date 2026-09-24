//! Bounded publication-package codec and keyless completeness checks.
//!
//! Persistence belongs to the never-dispatched database package owner. The capture
//! candidate ID is the bootstrap ID, not a freshly minted specimen identity.
//! This codec provides no authorization: membership predecessor bytes are context
//! only. `seed_claim::Publication` authenticates that context and the descriptor;
//! `bootstrap_staging` owns server publication. No production local dispatcher or
//! adoption is provided. Security review, cross-platform interoperability and
//! platform durability validation remain required.
//!
//! # Profile 1 byte contract (provisional, not released or security-approved)
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

pub(crate) mod catalog;
pub(crate) mod codec;
mod domain;

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

/// Privacy-safe failures. Resource refusal says nothing about domain validity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    ResourceLimit,
    Authentication,
    Unsupported,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid bootstrap representation",
            Self::ResourceLimit => "bootstrap codec resource limit exceeded",
            Self::Authentication => "bootstrap authentication failed",
            Self::Unsupported => "unsupported bootstrap format version",
        })
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;

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

#[derive(Clone, PartialEq, Eq)]
struct Descriptor {
    vault: [u8; 32],
    stream: [u8; 32],
    generation: [u8; 32],
    bootstrap: [u8; 32],
    // An already-known predecessor/genesis commitment, never PublishBootstrap.
    membership: [u8; 32],
    prefix: u64,
    catalogs: [Declaration; 3],
    manifest: Artifact,
}

impl Descriptor {
    fn context(&self) -> crypto::LocalSharedStatePackageContext {
        crypto::LocalSharedStatePackageContext {
            vault_id: self.vault,
            generation_id: self.generation,
        }
    }
    fn binding(&self) -> Vec<u8> {
        let mut out = b"AVBP\0\x02\x01".to_vec();
        for id in [
            self.vault,
            self.stream,
            self.generation,
            self.bootstrap,
            self.membership,
        ] {
            out.extend_from_slice(&id);
        }
        u64_bytes(&mut out, self.prefix);
        for (index, declaration) in self.catalogs.iter().enumerate() {
            out.push(index as u8 + 1);
            declaration.write(&mut out);
        }
        out
    }
    fn encode(&self) -> Result<Vec<u8>> {
        let mut out = self.binding();
        self.manifest.write(&mut out)?;
        Ok(out)
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        bound(number(bytes.len())?, MAX_DESCRIPTOR_BYTES as u64)?;
        let mut r = Reader(bytes);
        valid(r.take(4)? == b"AVBP")?;
        if r.take(2)? != [0, 2] {
            return Err(Error::Unsupported);
        }
        valid(r.byte()? == 1)?;
        let vault = r.array()?;
        let stream = r.array()?;
        let generation = r.array()?;
        let bootstrap = r.array()?;
        let membership = r.array()?;
        let prefix = r.u64()?;
        valid(prefix < i64::MAX as u64)?;
        bound(prefix, RECORD_LIMIT)?;
        valid(r.byte()? == 1)?;
        let data = Declaration::read(&mut r)?;
        valid(r.byte()? == 2)?;
        let ids = Declaration::read(&mut r)?;
        valid(r.byte()? == 3)?;
        let images = Declaration::read(&mut r)?;
        let manifest = Artifact::read(&mut r, CHUNK, false)?;
        r.end()?;
        Ok(Self {
            vault,
            stream,
            generation,
            bootstrap,
            membership,
            prefix,
            catalogs: [data, ids, images],
            manifest,
        })
    }
}

fn artifact_records(artifact: &crypto::EncryptedArtifact) -> Vec<Vec<u8>> {
    artifact
        .chunks
        .iter()
        .map(|chunk| chunk.record.clone())
        .collect()
}

fn state_catalog(state: &Artifact) -> Result<Vec<u8>> {
    let mut row = vec![1];
    state.write(&mut row)?;
    stream(1, &[row])
}

fn decode_state_catalog(bytes: &[u8]) -> Result<Artifact> {
    let records = read_stream(bytes, 1)?;
    valid(records.len() == 1)?;
    let mut r = Reader(records[0]);
    valid(r.byte()? == 1)?;
    let state = Artifact::read(&mut r, STATE_LIMIT, false)?;
    r.end()?;
    Ok(state)
}

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

/// Constructs tentative bytes only for the durable owner's first freeze.
/// Failed preparation may be rebuilt; committed bytes must only be loaded.
pub(super) fn build(
    capture: &NeverDispatchedLocalSharedCapture,
    context: crypto::LocalSharedStatePackageContext,
    images: &[crypto::EncryptedImage],
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<Package> {
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
    let (plaintext, stats) = domain::encode(&capture.capture.snapshot.tables, &mappings)?;
    let plaintext = Zeroizing::new(plaintext);
    let state_key = crypto::derive_bootstrap_class_key(key, context, stream_id, bootstrap, 1)
        .map_err(|_| Error::Authentication)?;
    let state =
        crypto::encrypt_artifact(&plaintext, context, stream_id, bootstrap, 2, 1, &state_key)
            .map_err(|_| Error::Authentication)?;
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
                artifact: Artifact::from_encrypted(&image.artifact)?,
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
    let mut image_records = images
        .iter()
        .map(|i| ImageRecords {
            object_id: i.object_id,
            records: artifact_records(&i.artifact),
        })
        .collect::<Vec<_>>();
    image_records.sort_by_key(|i| i.object_id);
    let package = Package {
        descriptor: d.encode()?,
        catalogs,
        state: artifact_records(&state),
        manifest: artifact_records(&manifest),
        images: image_records,
    };
    validate_against_capture(&package, capture, key, membership_predecessor)?;
    Ok(package)
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
    package: &Package,
) -> Result<(crypto::LocalSharedStatePackageContext, [u8; 32])> {
    let d = Descriptor::decode(&package.descriptor)?;
    Ok((d.context(), d.membership))
}

/// Checks exact clear commitments, framing, canonical coverage and all selected
/// image-byte obligations without a key. Catalogs are aggregate-verified before
/// any records are trusted. No partial catalog can establish completeness.
pub fn validate_keyless(package: &Package) -> Result<Completeness> {
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
    Ok(Completeness {
        prefix_count: d.prefix,
        image_count: number(images.objects.len())?,
        ciphertext_bytes,
    })
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
        valid(hex::encode(crypto::sha256(&bytes)) == mapping.sha256)?;
        accept_image(&mapping.sha256, bytes);
    }
    Ok((capture, mappings))
}

fn decrypt_metadata(
    metadata: &download::MetadataView<'_>,
    key: &LocalSharedStatePackageKey,
) -> Result<(SharedStateCapture, Vec<domain::Mapping>)> {
    validate_metadata(metadata)?;
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
    // Select the internal validation adapter version, not a wire version.
    let validation_version = if tables.shared_history_provenance.is_empty() {
        3
    } else {
        4
    };
    let capture = SharedStateCapture {
        snapshot: crate::data_safety::export_types::AvenExport {
            format: "aven-export".into(),
            version: validation_version,
            exported_at: String::new(),
            schema_version: 0,
            blobs_included: false,
            tables,
        },
    };
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

pub(crate) struct AttachmentIndex {
    pub context: crypto::LocalSharedStatePackageContext,
    pub stream: [u8; 32],
    pub objects: Vec<(catalog::Image, String)>,
    pub references: Vec<catalog::Reference>,
}

/// The caller receives mappings only after complete public/private authentication.
pub(crate) fn attachment_index(
    package: &Package,
    key: &LocalSharedStatePackageKey,
) -> anyhow::Result<AttachmentIndex> {
    let (_, mappings) = decrypt_domain_images(package, key, |_, _| {})?;
    index_from_mappings(&download::MetadataView::from(package), &mappings)
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
