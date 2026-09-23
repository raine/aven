use super::super::{
    self as crypto, EncryptedArtifact, EncryptedChunk, LocalSharedStatePackageContext,
};
use super::codec::*;
use super::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Artifact {
    pub total: u64,
    pub aggregate: [u8; 32],
    pub chunks: Vec<Chunk>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Chunk {
    pub length: u64,
    pub hash: [u8; 32],
    pub nonce: [u8; 24],
}

impl Artifact {
    pub fn from_encrypted(artifact: &EncryptedArtifact) -> Result<Self> {
        let chunks = artifact
            .chunks
            .iter()
            .map(|chunk| {
                let (header, _) =
                    crypto::split_record(&chunk.record).map_err(|_| Error::Invalid)?;
                Ok(Chunk {
                    length: number(chunk.record.len())?,
                    hash: chunk.record_commitment,
                    nonce: header[174..198].try_into().map_err(|_| Error::Invalid)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            total: artifact.total_plaintext_bytes,
            aggregate: artifact.aggregate_commitment,
            chunks,
        })
    }
    pub fn shape(&self, maximum: u64, image: bool) -> Result<()> {
        bound(self.total, maximum)?;
        valid(!image || self.total > 0)?;
        valid(number(self.chunks.len())? == count(self.total))?;
        for (i, chunk) in self.chunks.iter().enumerate() {
            let offset = number(i)?.checked_mul(CHUNK).ok_or(Error::Invalid)?;
            let expected = self
                .total
                .checked_sub(offset)
                .ok_or(Error::Invalid)?
                .min(CHUNK);
            valid(chunk.length == add(expected, 222)?)?;
        }
        Ok(())
    }
    pub fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        u64_bytes(out, self.total);
        out.extend_from_slice(&self.aggregate);
        u64_bytes(out, number(self.chunks.len())?);
        for (i, chunk) in self.chunks.iter().enumerate() {
            u64_bytes(out, number(i)?);
            u64_bytes(out, chunk.length);
            out.extend_from_slice(&chunk.hash);
            out.extend_from_slice(&chunk.nonce);
        }
        Ok(())
    }
    pub fn read(r: &mut Reader<'_>, maximum: u64, image: bool) -> Result<Self> {
        let total = r.u64()?;
        bound(total, maximum)?;
        let aggregate = r.array()?;
        let n = r.u64()?;
        valid(n == count(total))?;
        let mut chunks = Vec::new();
        for index in 0..n {
            valid(r.u64()? == index)?;
            chunks.push(Chunk {
                length: r.u64()?,
                hash: r.array()?,
                nonce: r.array()?,
            });
        }
        let value = Self {
            total,
            aggregate,
            chunks,
        };
        value.shape(maximum, image)?;
        Ok(value)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn verify(
        &self,
        records: &[Vec<u8>],
        context: LocalSharedStatePackageContext,
        stream: [u8; 32],
        id: [u8; 32],
        family: u8,
        class: u8,
    ) -> Result<()> {
        valid(records.len() == self.chunks.len())?;
        let mut digest = sha2::Sha256::new();
        use sha2::Digest;
        for (index, record) in records.iter().enumerate() {
            self.verify_chunk(record, index, context, stream, id, family, class)?;
            digest.update(record);
        }
        valid(<[u8; 32]>::from(digest.finalize()) == self.aggregate)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn verify_chunk(
        &self,
        record: &[u8],
        index: usize,
        context: LocalSharedStatePackageContext,
        stream: [u8; 32],
        id: [u8; 32],
        family: u8,
        class: u8,
    ) -> Result<()> {
        let chunk = self.chunks.get(index).ok_or(Error::Invalid)?;
        valid(number(record.len())? == chunk.length && crypto::sha256(record) == chunk.hash)?;
        let (header, _) = crypto::split_record(record).map_err(|_| Error::Invalid)?;
        let nonce = crypto::validate_chunk_header(
            header,
            context,
            stream,
            id,
            family,
            class,
            u32::try_from(index).map_err(|_| Error::Invalid)?,
            u32::try_from(self.chunks.len()).map_err(|_| Error::Invalid)?,
            self.total,
        )
        .map_err(|_| Error::Invalid)?;
        valid(nonce == chunk.nonce)
    }
    pub fn encrypted(&self, records: &[Vec<u8>]) -> EncryptedArtifact {
        EncryptedArtifact {
            total_plaintext_bytes: self.total,
            aggregate_commitment: self.aggregate,
            chunks: records
                .iter()
                .zip(&self.chunks)
                .map(|(record, chunk)| EncryptedChunk {
                    record_commitment: chunk.hash,
                    record: record.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Declaration {
    pub count: u64,
    pub length: u64,
    pub hash: [u8; 32],
}
impl Declaration {
    pub fn new(bytes: &[u8], class: u8) -> Result<Self> {
        Ok(Self {
            count: number(read_stream(bytes, class)?.len())?,
            length: number(bytes.len())?,
            hash: crypto::sha256(bytes),
        })
    }
    pub fn write(&self, out: &mut Vec<u8>) {
        u64_bytes(out, self.count);
        u64_bytes(out, self.length);
        u64_bytes(out, count(self.length));
        out.extend_from_slice(&self.hash);
    }
    pub fn read(r: &mut Reader<'_>) -> Result<Self> {
        let n = r.u64()?;
        let length = r.u64()?;
        bound(n, RECORD_LIMIT)?;
        bound(length, CATALOG_LIMIT)?;
        valid(length >= 15 && r.u64()? == count(length))?;
        Ok(Self {
            count: n,
            length,
            hash: r.array()?,
        })
    }
    pub fn verify(&self, bytes: &[u8], class: u8) -> Result<()> {
        valid(number(bytes.len())? == self.length && crypto::sha256(bytes) == self.hash)?;
        valid(Self::new(bytes, class)? == *self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Parent {
    pub workspace: String,
    pub task: String,
    pub deleted: bool,
    pub version: Option<String>,
    pub protected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Reference {
    pub workspace: String,
    pub task: String,
    pub reference: String,
    pub deleted: bool,
    pub object: Option<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Image {
    pub id: [u8; 32],
    // 1 current selected, 2 extra selected. Both require complete bytes.
    pub selection: u8,
    pub artifact: Artifact,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Images {
    pub objects: Vec<Image>,
    pub parents: Vec<Parent>,
    pub references: Vec<Reference>,
}

impl Images {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut rows = Vec::new();
        for object in &self.objects {
            let mut row = vec![1];
            row.extend_from_slice(&object.id);
            row.push(object.selection);
            object.artifact.write(&mut row)?;
            rows.push(row);
        }
        for parent in &self.parents {
            let mut row = vec![2];
            text(&mut row, &parent.workspace)?;
            text(&mut row, &parent.task)?;
            row.push(u8::from(parent.deleted));
            row.push(u8::from(parent.protected));
            row.push(u8::from(parent.version.is_some()));
            if let Some(version) = &parent.version {
                text(&mut row, version)?;
            }
            rows.push(row);
        }
        for reference in &self.references {
            let mut row = vec![3];
            text(&mut row, &reference.workspace)?;
            text(&mut row, &reference.task)?;
            text(&mut row, &reference.reference)?;
            row.push(u8::from(reference.deleted));
            row.push(u8::from(reference.object.is_some()));
            if let Some(object) = reference.object {
                row.extend_from_slice(&object);
            }
            rows.push(row);
        }
        stream(3, &rows)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut result = Self {
            objects: Vec::new(),
            parents: Vec::new(),
            references: Vec::new(),
        };
        let mut previous_kind = 0;
        let mut total = 0;
        for row in read_stream(bytes, 3)? {
            let mut r = Reader(row);
            let kind = r.byte()?;
            valid(kind >= previous_kind)?;
            previous_kind = kind;
            match kind {
                1 => {
                    bound(number(result.objects.len())? + 1, IMAGE_COUNT)?;
                    let id = r.array()?;
                    valid(
                        result
                            .objects
                            .last()
                            .is_none_or(|last: &Image| last.id < id),
                    )?;
                    let selection = r.byte()?;
                    valid(matches!(selection, 1 | 2))?;
                    let artifact = Artifact::read(&mut r, IMAGE_LIMIT, true)?;
                    total = add(total, artifact.total)?;
                    bound(total, STATE_LIMIT)?;
                    result.objects.push(Image {
                        id,
                        selection,
                        artifact,
                    });
                }
                2 => {
                    let workspace = r.text()?;
                    let task = r.text()?;
                    valid(result.parents.last().is_none_or(|last: &Parent| {
                        (&last.workspace, &last.task) < (&workspace, &task)
                    }))?;
                    let deleted = r.flag()?;
                    let protected = r.flag()?;
                    let version = if r.flag()? { Some(r.text()?) } else { None };
                    valid(version.is_some() || protected)?;
                    result.parents.push(Parent {
                        workspace,
                        task,
                        deleted,
                        protected,
                        version,
                    });
                }
                3 => {
                    let workspace = r.text()?;
                    let task = r.text()?;
                    let reference = r.text()?;
                    valid(result.references.last().is_none_or(|last: &Reference| {
                        (&last.workspace, &last.reference) < (&workspace, &reference)
                    }))?;
                    let deleted = r.flag()?;
                    let object = if r.flag()? { Some(r.array()?) } else { None };
                    result.references.push(Reference {
                        workspace,
                        task,
                        reference,
                        deleted,
                        object,
                    });
                }
                _ => return Err(Error::Invalid),
            }
            r.end()?;
        }
        for reference in &result.references {
            valid(
                result
                    .parents
                    .binary_search_by(|p| {
                        (&p.workspace, &p.task).cmp(&(&reference.workspace, &reference.task))
                    })
                    .is_ok(),
            )?;
            if let Some(object) = reference.object {
                valid(
                    result
                        .objects
                        .binary_search_by_key(&object, |o| o.id)
                        .is_ok(),
                )?;
            }
        }
        let mut current = std::collections::HashSet::new();
        for reference in &result.references {
            let parent = result
                .parents
                .binary_search_by(|p| {
                    (&p.workspace, &p.task).cmp(&(&reference.workspace, &reference.task))
                })
                .map_err(|_| Error::Invalid)?;
            if !reference.deleted
                && !result.parents[parent].deleted
                && let Some(object) = reference.object
            {
                current.insert(object);
            }
        }
        for object in &result.objects {
            valid((object.selection == 1) == current.contains(&object.id))?;
        }
        Ok(result)
    }
}

pub(super) fn prefix_encode(rows: &[(u64, String)]) -> Result<Vec<u8>> {
    let mut records = Vec::new();
    for (rank, id) in rows {
        let mut record = Vec::new();
        u64_bytes(&mut record, *rank);
        text(&mut record, id)?;
        records.push(record);
    }
    stream(2, &records)
}

pub(super) fn prefix_decode(bytes: &[u8], expected: u64) -> Result<Vec<(u64, String)>> {
    valid(expected < i64::MAX as u64)?;
    bound(expected, RECORD_LIMIT)?;
    let records = read_stream(bytes, 2)?;
    valid(number(records.len())? == expected)?;
    let mut ids = std::collections::HashSet::new();
    let mut result = Vec::new();
    for (index, record) in records.iter().enumerate() {
        let mut r = Reader(record);
        let rank = r.u64()?;
        valid(rank == add(number(index)?, 1)?)?;
        let id = r.text()?;
        valid(ids.insert(id.clone()))?;
        r.end()?;
        result.push((rank, id));
    }
    Ok(result)
}
