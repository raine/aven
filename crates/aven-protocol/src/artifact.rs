//! Descriptor, catalog and ordered whole-artifact verification.
//! Structural verification does not authorize membership or publication.

pub mod catalog;
pub mod codec;
pub mod staging;

use crate::context::LocalSharedStatePackageContext;
use catalog::{Artifact, Declaration};
use codec::*;

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
pub type Result<T> = std::result::Result<T, Error>;

/// Encrypted records that are either freshly owned or borrowed from a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedArtifact<'a> {
    pub total_plaintext_bytes: u64,
    pub aggregate_commitment: [u8; 32],
    pub chunks: Vec<EncryptedChunk<'a>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedChunk<'a> {
    pub record_commitment: [u8; 32],
    pub record: std::borrow::Cow<'a, [u8]>,
}

impl EncryptedArtifact<'_> {
    /// Moves the records out without copying owned bytes.
    pub fn into_records(self) -> Vec<Vec<u8>> {
        self.chunks
            .into_iter()
            .map(|chunk| chunk.record.into_owned())
            .collect()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub vault: [u8; 32],
    pub stream: [u8; 32],
    pub generation: [u8; 32],
    pub bootstrap: [u8; 32],
    // An already-known predecessor/genesis commitment, never PublishBootstrap.
    pub membership: [u8; 32],
    pub prefix: u64,
    pub catalogs: [Declaration; 3],
    pub manifest: Artifact,
}

impl Descriptor {
    pub fn context(&self) -> LocalSharedStatePackageContext {
        LocalSharedStatePackageContext {
            vault_id: self.vault,
            generation_id: self.generation,
        }
    }
    pub fn binding(&self) -> Vec<u8> {
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
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = self.binding();
        self.manifest.write(&mut out)?;
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
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

pub fn state_catalog(state: &Artifact) -> Result<Vec<u8>> {
    let mut row = vec![1];
    state.write(&mut row)?;
    stream(1, &[row])
}

pub fn decode_state_catalog(bytes: &[u8]) -> Result<Artifact> {
    let records = read_stream(bytes, 1)?;
    valid(records.len() == 1)?;
    let mut r = Reader(records[0]);
    valid(r.byte()? == 1)?;
    let state = Artifact::read(&mut r, STATE_LIMIT, false)?;
    r.end()?;
    Ok(state)
}

#[cfg(test)]
mod tests;
