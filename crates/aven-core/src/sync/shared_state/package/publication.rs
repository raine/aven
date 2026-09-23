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
//! * Descriptor: `AVBP || U16(1) || U8(1 suite)`, vault, stream, generation,
//!   capture bootstrap, predecessor membership commitment, U64 prefix count,
//!   three catalog declarations in class order, then manifest artifact descriptor.
//! * Declaration: U8 class, U64 record count, U64 byte length, U64 transfer chunk
//!   count, SHA-256. Catalog transfer chunks are contiguous 1 MiB byte slices
//!   (final remainder); only the aggregate is committed. Individual catalog
//!   slices are untrusted until the entire bounded stream verifies.
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

mod catalog;
mod codec;
mod domain;

/// Version of the encrypted AVBD domain, independent of plaintext sync/export.
pub const DOMAIN_VERSION: u32 = 1;
pub mod download;
mod projection;
pub(crate) mod staging;

use super::super::{NeverDispatchedLocalSharedCapture, SharedStateCapture};
use super::{self as crypto, EncryptedLocalSharedStatePackage, LocalSharedStatePackageKey};
use catalog::{Artifact, Declaration, Image, Images};
use codec::*;
use zeroize::Zeroizing;

/// Privacy-safe failures. Resource refusal says nothing about domain validity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    ResourceLimit,
    Authentication,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid bootstrap representation",
            Self::ResourceLimit => "bootstrap codec resource limit exceeded",
            Self::Authentication => "bootstrap authentication failed",
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
        let mut out = b"AVBP\0\x01\x01".to_vec();
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
        bound(number(bytes.len())?, 1024)?;
        let mut r = Reader(bytes);
        valid(r.take(7)? == b"AVBP\0\x01\x01")?;
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
    images: &[crypto::EncryptedLocalSharedStateImage],
    image_mappings: &[crypto::PrivateImageMapping],
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<EncryptedLocalSharedStatePackage> {
    let stream_id =
        crypto::decode_context_id(capture.stream_id(), "stream").map_err(|_| Error::Invalid)?;
    let bootstrap = crypto::decode_context_id(capture.candidate_id(), "candidate")
        .map_err(|_| Error::Invalid)?;
    let frozen_mappings = image_mappings
        .iter()
        .map(|m| (m.source_sha256.as_str(), m.object_id))
        .collect::<std::collections::HashMap<_, _>>();
    let mappings = capture
        .images
        .iter()
        .map(|r| {
            let object = frozen_mappings.get(r.sha256.as_str()).copied();
            domain::Mapping {
                sha256: r.sha256.clone(),
                classification: r.classification.clone(),
                object,
            }
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
            let mapping = image_mappings
                .iter()
                .find(|m| m.object_id == image.object_id)
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
    let local = EncryptedLocalSharedStatePackage {
        candidate_id: capture.candidate_id().to_owned(),
        stream_id,
        context,
        state,
        manifest,
        images: images.to_vec(),
        image_mappings: image_mappings.to_vec(),
        descriptor: package.descriptor.clone(),
        catalogs: package.catalogs.clone(),
    };
    validate_against_capture(&package, capture, &local, key, membership_predecessor)?;
    Ok(local)
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

pub(super) fn membership(package: &EncryptedLocalSharedStatePackage) -> Result<[u8; 32]> {
    Ok(Descriptor::decode(&package.descriptor)?.membership)
}

/// Checks exact clear commitments, framing, canonical coverage and all selected
/// image-byte obligations without a key. Catalogs are aggregate-verified before
/// any records are trusted. No partial catalog can establish completeness.
pub fn validate_keyless(package: &Package) -> Result<Completeness> {
    let d = Descriptor::decode(&package.descriptor)?;
    for (i, bytes) in package.catalogs.iter().enumerate() {
        d.catalogs[i].verify(bytes, i as u8 + 1)?;
    }
    let state = decode_state_catalog(&package.catalogs[0])?;
    catalog::prefix_decode(&package.catalogs[1], d.prefix)?;
    let images = Images::decode(&package.catalogs[2])?;
    state.verify(&package.state, d.context(), d.stream, d.bootstrap, 2, 1)?;
    d.manifest
        .verify(&package.manifest, d.context(), d.stream, d.bootstrap, 2, 2)?;
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
    let d = Descriptor::decode(&package.descriptor)?;
    let state = decode_state_catalog(&package.catalogs[0])?;
    let state_key = crypto::derive_bootstrap_class_key(key, d.context(), d.stream, d.bootstrap, 1)
        .map_err(|_| Error::Authentication)?;
    let plaintext = Zeroizing::new(
        crypto::decrypt_artifact(
            &state.encrypted(&package.state),
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
            &d.manifest.encrypted(&package.manifest),
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
            == catalog::prefix_decode(&package.catalogs[1], d.prefix)?,
    )?;
    let images = Images::decode(&package.catalogs[2])?;
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

fn validate_local_metadata(local: &EncryptedLocalSharedStatePackage) -> Result<()> {
    let d = Descriptor::decode(&local.descriptor)?;
    valid(
        d.context() == local.context
            && d.stream == local.stream_id
            && d.bootstrap
                == crypto::decode_context_id(local.candidate_id(), "candidate")
                    .map_err(|_| Error::Invalid)?,
    )?;
    valid(Artifact::from_encrypted(&local.state)? == decode_state_catalog(&local.catalogs[0])?)?;
    valid(Artifact::from_encrypted(&local.manifest)? == d.manifest)?;
    let images = Images::decode(&local.catalogs[2])?;
    valid(images.objects.len() == local.images.len())?;
    for image in &local.images {
        let object = images
            .objects
            .iter()
            .find(|o| o.id == image.object_id)
            .ok_or(Error::Invalid)?;
        valid(Artifact::from_encrypted(&image.artifact)? == object.artifact)?;
    }
    Ok(())
}

pub(super) fn decrypt_local(
    local: &EncryptedLocalSharedStatePackage,
    key: &LocalSharedStatePackageKey,
) -> Result<SharedStateCapture> {
    validate_local_metadata(local)?;
    let (capture, mappings) = decrypt_domain(&local.upload_package(), key)?;
    let mut selected = mappings
        .into_iter()
        .filter_map(|mapping| {
            mapping.object.map(|object_id| crypto::PrivateImageMapping {
                source_sha256: mapping.sha256,
                classification: mapping.classification,
                object_id,
            })
        })
        .collect::<Vec<_>>();
    selected.sort_by(|a, b| a.source_sha256.cmp(&b.source_sha256));
    valid(selected == local.image_mappings)?;
    Ok(capture)
}

/// Client-side check against the immutable source, not live rows. Also checks
/// encrypted manifest/catalog agreement and private image hashes. This is not
/// join installation and does not establish any trusted membership anchor.
pub fn validate_against_capture(
    package: &Package,
    capture: &NeverDispatchedLocalSharedCapture,
    local: &EncryptedLocalSharedStatePackage,
    key: &LocalSharedStatePackageKey,
    membership_predecessor: [u8; 32],
) -> Result<()> {
    validate_local_metadata(local)?;
    let d = Descriptor::decode(&package.descriptor)?;
    valid(
        d.context() == local.context
            && d.stream == local.stream_id
            && d.membership == membership_predecessor,
    )?;
    valid(
        capture.candidate_id() == local.candidate_id()
            && capture.stream_id() == hex::encode(d.stream),
    )?;
    valid(
        d.bootstrap
            == crypto::decode_context_id(local.candidate_id(), "candidate")
                .map_err(|_| Error::Invalid)?,
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
    let frozen_mappings = local
        .image_mappings
        .iter()
        .map(|m| (m.source_sha256.as_str(), m.object_id))
        .collect::<std::collections::HashMap<_, _>>();
    for mapping in &mappings {
        valid(
            captured_images.get(mapping.sha256.as_str()).copied()
                == Some(mapping.classification.as_str()),
        )?;
        let expected = frozen_mappings.get(mapping.sha256.as_str()).copied();
        valid(mapping.object == expected)?;
    }
    valid(package.images.len() == local.images.len())?;
    for image in &package.images {
        let original = local
            .images
            .iter()
            .find(|i| i.object_id == image.object_id)
            .ok_or(Error::Invalid)?;
        valid(image.records == artifact_records(&original.artifact))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
