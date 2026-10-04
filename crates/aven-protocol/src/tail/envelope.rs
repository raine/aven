//! Keyless envelope/projection parsing. Parsing is not device authorization.
use crate::codec::bytes;
use crate::wire::tail::RECORD_LIMIT;
use anyhow::{Result, ensure};
const BASE32: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const MAX_MOVE_TASKS: usize = 256;
fn valid(ok: bool) -> Result<()> {
    ensure!(ok, "error encrypted-tail-invalid");
    Ok(())
}
fn encode_task(bytes: &[u8]) -> String {
    let mut value = 0_u128;
    for byte in bytes {
        value = (value << 8) | u128::from(*byte);
    }
    (0..16)
        .rev()
        .map(|shift| BASE32[((value >> (shift * 5)) & 31) as usize] as char)
        .collect()
}
#[derive(Clone, PartialEq, Eq)]
pub enum Projection {
    None,
    Move {
        source: String,
        target: String,
        tasks: Vec<String>,
    },
    Ref {
        workspace: String,
        task: String,
        reference: String,
        descriptor: Vec<u8>,
        deleted: bool,
        version: Option<String>,
    },
    Unref {
        workspace: String,
        task: String,
        reference: String,
    },
    Parent {
        action: super::parent::ParentAction,
        workspace: String,
        task: String,
        deleted: bool,
        version: Option<String>,
    },
}
impl Projection {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Move {
                source,
                target,
                tasks,
            } => {
                let mut out = vec![4];
                encode_text(&mut out, source);
                encode_text(&mut out, target);
                out.extend((tasks.len() as u16).to_be_bytes());
                for task in tasks {
                    let mut bits = 0_u128;
                    for byte in task.bytes() {
                        bits = (bits << 5)
                            | BASE32
                                .iter()
                                .position(|v| *v == byte)
                                .expect("validated task ID") as u128;
                    }
                    out.extend_from_slice(&bits.to_be_bytes()[6..]);
                }
                return out;
            }
            Self::Ref {
                workspace,
                task,
                reference,
                descriptor,
                deleted,
                version,
            } => {
                let mut out = vec![2];
                for text in [workspace, task, reference] {
                    encode_text(&mut out, text);
                }
                out.extend((descriptor.len() as u32).to_be_bytes());
                out.extend(descriptor);
                out.push(u8::from(*deleted));
                out.push(u8::from(version.is_some()));
                if let Some(v) = version {
                    encode_text(&mut out, v);
                }
                return out;
            }
            Self::Unref {
                workspace,
                task,
                reference,
            } => {
                let mut out = vec![3];
                for text in [workspace, task, reference] {
                    encode_text(&mut out, text);
                }
                return out;
            }
            _ => {}
        }
        let Self::Parent {
            action,
            workspace,
            task,
            deleted,
            version,
        } = self
        else {
            return vec![0];
        };
        let mut out = vec![1, *action as u8];
        encode_text(&mut out, workspace);
        encode_text(&mut out, task);
        out.push(u8::from(*deleted));
        out.push(u8::from(version.is_some()));
        if let Some(v) = version {
            encode_text(&mut out, v);
        }
        out
    }
    pub fn decode(input: &[u8]) -> Result<Self> {
        decode_projection(input)
    }
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        valid(n <= self.0.len())?;
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }
    fn blob(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = u32::from_be_bytes(self.array()?) as usize;
        valid(n <= max)?;
        self.take(n)
    }
}
pub struct Envelope<'a> {
    pub vault: [u8; 32],
    pub stream: [u8; 32],
    pub generation: [u8; 32],
    pub id: String,
    pub projection: Projection,
    pub header: &'a [u8],
    pub body: &'a [u8],
    pub nonce: [u8; 24],
}
pub fn parse(record: &[u8]) -> Result<Envelope<'_>> {
    valid(record.len() <= RECORD_LIMIT)?;
    let mut r = Reader(record);
    let header = r.blob(4544)?;
    let body = r.blob(131072 + 16)?;
    valid(r.0.is_empty() && body.len() >= 16)?;
    let mut r = Reader(header);
    valid(r.take(8)? == b"AVEN\x01\x00\x01\x01")?;
    let vault = r.blob(32)?.try_into()?;
    let stream = r.blob(32)?.try_into()?;
    let generation = r.blob(32)?.try_into()?;
    valid(r.blob(32)?.len() == 32)?;
    let id = std::str::from_utf8(r.blob(256)?)?.to_owned();
    valid(!id.is_empty())?;
    valid(r.array::<4>()? == 18u32.to_be_bytes())?;
    let projection = Projection::decode(r.blob(4096)?)?;
    let nonce = r.blob(24)?.try_into()?;
    valid(r.0.is_empty())?;
    Ok(Envelope {
        vault,
        stream,
        generation,
        id,
        projection,
        header,
        body,
        nonce,
    })
}
pub fn encode_text(out: &mut Vec<u8>, s: &str) {
    bytes(out, s.as_bytes());
}
pub fn decode_projection(input: &[u8]) -> Result<Projection> {
    let mut r = Reader(input);
    let result = match r.take(1)?[0] {
        0 => Projection::None,
        1 => {
            let action = super::parent::ParentAction::from_byte(r.take(1)?[0])
                .ok_or_else(|| anyhow::anyhow!("error encrypted-tail-invalid"))?;
            let workspace = std::str::from_utf8(r.blob(256)?)?.to_owned();
            let task = std::str::from_utf8(r.blob(256)?)?.to_owned();
            let deleted = r.take(1)?[0];
            valid(deleted <= 1)?;
            let version = match r.take(1)?[0] {
                0 => None,
                1 => Some(std::str::from_utf8(r.blob(256)?)?.to_owned()),
                _ => anyhow::bail!("error encrypted-tail-projection"),
            };
            valid(
                !workspace.is_empty()
                    && !task.is_empty()
                    && version.as_ref().is_none_or(|s| !s.is_empty()),
            )?;
            valid(
                action != super::parent::ParentAction::Create
                    || (deleted == 0 && version.is_some()),
            )?;
            Projection::Parent {
                action,
                workspace,
                task,
                deleted: deleted == 1,
                version,
            }
        }
        kind @ (2 | 3) => {
            let mut text = || -> Result<String> {
                let s = std::str::from_utf8(r.blob(256)?)?.to_owned();
                valid(!s.is_empty())?;
                Ok(s)
            };
            let workspace = text()?;
            let task = text()?;
            let reference = text()?;
            if kind == 3 {
                Projection::Unref {
                    workspace,
                    task,
                    reference,
                }
            } else {
                let descriptor = r.blob(1984)?.to_vec();
                crate::images::Descriptor::decode(&descriptor)?;
                let deleted = r.take(1)?[0];
                valid(deleted <= 1)?;
                let version = match r.take(1)?[0] {
                    0 => None,
                    1 => {
                        let s = std::str::from_utf8(r.blob(256)?)?.to_owned();
                        valid(!s.is_empty())?;
                        Some(s)
                    }
                    _ => anyhow::bail!("error encrypted-image-hint"),
                };
                Projection::Ref {
                    workspace,
                    task,
                    reference,
                    descriptor,
                    deleted: deleted == 1,
                    version,
                }
            }
        }
        4 => {
            let source = std::str::from_utf8(r.blob(16)?)?.to_owned();
            let target = std::str::from_utf8(r.blob(16)?)?.to_owned();
            for id in [&source, &target] {
                ensure!(
                    id.len() == 16 && id.bytes().all(|byte| BASE32.contains(&byte)),
                    "workspace ID must be 16 Crockford Base32 characters"
                );
            }
            valid(source != target)?;
            let count = u16::from_be_bytes(r.array()?) as usize;
            valid((1..=MAX_MOVE_TASKS).contains(&count))?;
            let mut tasks: Vec<String> = Vec::with_capacity(count);
            for _ in 0..count {
                let mut bytes = [0_u8; 16];
                bytes[6..].copy_from_slice(r.take(10)?);
                let task = encode_task(&bytes[6..]);
                valid(tasks.last().is_none_or(|previous| previous < &task))?;
                tasks.push(task);
            }
            Projection::Move {
                source,
                target,
                tasks,
            }
        }
        _ => anyhow::bail!("error encrypted-tail-projection"),
    };
    valid(r.0.is_empty())?;
    Ok(result)
}
