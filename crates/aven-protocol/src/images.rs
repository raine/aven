//! Image descriptor and exact retained-byte verification, independent of keys.
use crate::artifact::{catalog::Artifact, codec::Reader};
use crate::context::LocalSharedStatePackageContext;
pub use crate::wire::images::{DESCRIPTOR_LIMIT, IMAGE_BYTES};
use anyhow::{Result, ensure};
fn valid(ok: bool) -> Result<()> {
    ensure!(ok, "error encrypted-tail-invalid");
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub vault: [u8; 32],
    pub stream: [u8; 32],
    pub generation: [u8; 32],
    pub object: [u8; 32],
    pub artifact: Artifact,
}
impl Descriptor {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        valid(bytes.len() <= DESCRIPTOR_LIMIT)?;
        let mut r = Reader(bytes);
        valid(r.take(8)? == b"AVEN\x03\x00\x01\x01")?;
        let value = Self {
            vault: r.array()?,
            stream: r.array()?,
            generation: r.array()?,
            object: r.array()?,
            artifact: Artifact::read(&mut r, IMAGE_BYTES as u64, true)?,
        };
        r.end()?;
        Ok(value)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.artifact.shape(IMAGE_BYTES as u64, true)?;
        let mut bytes = b"AVEN\x03\x00\x01\x01".to_vec();
        for id in [&self.vault, &self.stream, &self.generation, &self.object] {
            bytes.extend(id);
        }
        self.artifact.write(&mut bytes)?;
        Ok(bytes)
    }
    pub fn context(&self) -> LocalSharedStatePackageContext {
        LocalSharedStatePackageContext {
            vault_id: self.vault,
            generation_id: self.generation,
        }
    }
    pub fn byte_size(&self) -> i64 {
        self.artifact.chunks.iter().map(|c| c.length as i64).sum()
    }
    pub fn verify_chunk(&self, index: usize, bytes: &[u8]) -> Result<()> {
        self.artifact
            .verify_chunk(bytes, index, self.context(), self.stream, self.object, 1, 0)?;
        Ok(())
    }
    pub fn verify(&self, records: &[Vec<u8>]) -> Result<()> {
        self.artifact
            .verify(records, self.context(), self.stream, self.object, 1, 0)?;
        Ok(())
    }
}
