use super::super::{Authority, hash, valid};
use crate::sync::bootstrap_format::{catalog::Artifact, codec::Reader};
use crate::sync::shared_state::package::{self as crypto, LocalSharedStatePackageContext};
use anyhow::{Result, ensure};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use zeroize::Zeroizing;

pub const DESCRIPTOR_LIMIT: usize = 1984;
pub const IMAGE_BYTES: usize = 25 * 1048576;
pub const CHUNK_BYTES: usize = 1048576 + 222;
pub const TRANSFER_BYTES: usize = IMAGE_BYTES + 25 * 222;
pub const HTTP_LIMIT: usize = crate::sync::base64_bytes::encoded_len(CHUNK_BYTES) + 16384;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Descriptor {
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
    pub(super) fn authority(&self, a: &Authority) -> Result<()> {
        a.validate()?;
        a.key(self.generation)?;
        valid(self.vault == a.context.vault && self.stream == a.context.stream)
    }
    pub fn seal(a: &Authority, bytes: &[u8]) -> Result<(Self, Vec<Vec<u8>>)> {
        a.validate()?;
        ensure!(!a.rotation_pending(), "error membership-rotation-pending");
        valid(!bytes.is_empty() && bytes.len() <= IMAGE_BYTES)?;
        let mut object = [0; 32];
        getrandom::fill(&mut object)
            .map_err(|_| anyhow::anyhow!("error encrypted-image-entropy"))?;
        let context = LocalSharedStatePackageContext {
            vault_id: a.context.vault,
            generation_id: a.generation(),
        };
        let key = crypto::derive_image_key(a.key(a.generation())?, context, object)?;
        let encrypted =
            crypto::encrypt_artifact(bytes, context, a.context.stream, object, 1, 0, &key)?;
        let descriptor = Self {
            vault: context.vault_id,
            stream: a.context.stream,
            generation: context.generation_id,
            object,
            artifact: Artifact::from_encrypted(&encrypted)?,
        };
        let records = encrypted.chunks.into_iter().map(|c| c.record).collect();
        Ok((descriptor, records))
    }
    /// The owned source is hashed in full before any saved nonce is used.
    pub fn reconstruct(
        &self,
        a: &Authority,
        source: &[u8],
        expected_hash: &str,
    ) -> Result<Vec<Vec<u8>>> {
        self.authority(a)?;
        ensure!(
            source.len() as u64 == self.artifact.total
                && hex::encode(hash(source)) == expected_hash,
            "error encrypted-image-source-changed"
        );
        let key = crypto::derive_image_key(a.key(self.generation)?, self.context(), self.object)?;
        let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).expect("fixed key");
        let mut records = Vec::new();
        for (index, chunk) in self.artifact.chunks.iter().enumerate() {
            let start = index * 1048576;
            let end = source.len().min(start + 1048576);
            let header = crypto::chunk_header(
                self.context(),
                self.stream,
                self.object,
                1,
                0,
                index as u32,
                self.artifact.chunks.len() as u32,
                self.artifact.total,
                chunk.nonce,
            )?;
            let body = cipher
                .encrypt(
                    &XNonce::from(chunk.nonce),
                    Payload {
                        msg: &source[start..end],
                        aad: &header,
                    },
                )
                .map_err(|_| anyhow::anyhow!("error encrypted-image-encryption"))?;
            let mut record = Vec::new();
            for part in [&header, &body] {
                record.extend((part.len() as u32).to_be_bytes());
                record.extend(part);
            }
            records.push(record);
        }
        self.verify(&records)?;
        Ok(records)
    }
    pub fn open(&self, a: &Authority, records: &[Vec<u8>]) -> Result<Zeroizing<Vec<u8>>> {
        self.authority(a)?;
        self.verify(records)?;
        let key = crypto::derive_image_key(a.key(self.generation)?, self.context(), self.object)?;
        Ok(Zeroizing::new(crypto::decrypt_artifact(
            &self.artifact.encrypted(records),
            self.context(),
            self.stream,
            self.object,
            1,
            0,
            &key,
            IMAGE_BYTES,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_recipe_reconstruction_and_frozen_commitments() {
        let a = crate::sync::encrypted_tail::tests::authority();
        let source = vec![42; 1048579];
        let sha = hex::encode(hash(&source));
        let (d, records) = Descriptor::seal(&a, &source).unwrap();
        assert_eq!(Descriptor::decode(&d.encode().unwrap()).unwrap(), d);
        assert_eq!(d.reconstruct(&a, &source, &sha).unwrap(), records);
        assert_eq!(d.open(&a, &records).unwrap().as_slice(), source);
        let mut changed = source.clone();
        changed[0] ^= 1;
        assert!(d.reconstruct(&a, &changed, &sha).is_err());
        let mut changed = d.clone();
        changed.artifact.chunks[0].nonce[0] ^= 1;
        assert!(changed.reconstruct(&a, &source, &sha).is_err());
        let encoded = d.encode().unwrap();
        for n in 0..encoded.len() {
            assert!(Descriptor::decode(&encoded[..n]).is_err());
        }
    }
}
