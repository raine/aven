use super::{Error, Result};

pub(super) const CHUNK: u64 = 1_048_576;
pub(super) const STATE_LIMIT: u64 = 256 * CHUNK;
pub(super) const CATALOG_LIMIT: u64 = 16 * CHUNK;
pub(super) const RECORD_LIMIT: u64 = 1_000_000;
pub(super) const IMAGE_LIMIT: u64 = 25 * CHUNK;
pub(super) const IMAGE_COUNT: u64 = 1024;
pub(super) const ID_LIMIT: u64 = 256;

pub(super) fn valid(ok: bool) -> Result<()> {
    if ok { Ok(()) } else { Err(Error::Invalid) }
}

pub(super) fn bound(value: u64, maximum: u64) -> Result<()> {
    if value <= maximum {
        Ok(())
    } else {
        Err(Error::ResourceLimit)
    }
}

pub(super) fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or(Error::Invalid)
}

pub(super) fn size(value: u64) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::ResourceLimit)
}

pub(super) fn number(value: usize) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::ResourceLimit)
}

pub(super) fn count(total: u64) -> u64 {
    (total / CHUNK + u64::from(!total.is_multiple_of(CHUNK))).max(1)
}

pub(super) fn u64_bytes(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub(super) fn bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    u64_bytes(out, number(value.len())?);
    out.extend_from_slice(value);
    Ok(())
}

pub(super) fn text(out: &mut Vec<u8>, value: &str) -> Result<()> {
    valid(!value.is_empty())?;
    bound(number(value.len())?, ID_LIMIT)?;
    bytes(out, value.as_bytes())
}

pub(super) struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        valid(n <= self.0.len())?;
        let (value, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(value)
    }
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?.try_into().map_err(|_| Error::Invalid)
    }
    pub fn byte(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    pub fn flag(&mut self) -> Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Invalid),
        }
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    pub fn bytes(&mut self, maximum: u64) -> Result<&'a [u8]> {
        let len = self.u64()?;
        bound(len, maximum)?;
        self.take(size(len)?)
    }
    pub fn text(&mut self) -> Result<String> {
        let value = self.bytes(ID_LIMIT)?;
        valid(!value.is_empty())?;
        Ok(std::str::from_utf8(value)
            .map_err(|_| Error::Invalid)?
            .to_owned())
    }
    pub fn end(self) -> Result<()> {
        valid(self.0.is_empty())
    }
}

// Every sequential stream includes its class and count in its committed bytes.
pub(super) fn stream(class: u8, records: &[Vec<u8>]) -> Result<Vec<u8>> {
    bound(number(records.len())?, RECORD_LIMIT)?;
    let mut out = b"AVBC\0\x01".to_vec();
    out.push(class);
    u64_bytes(&mut out, number(records.len())?);
    for record in records {
        bound(
            add(number(out.len())?, add(8, number(record.len())?)?)?,
            CATALOG_LIMIT,
        )?;
        bytes(&mut out, record)?;
    }
    Ok(out)
}

pub(super) fn read_stream(input: &[u8], class: u8) -> Result<Vec<&[u8]>> {
    bound(number(input.len())?, CATALOG_LIMIT)?;
    let mut r = Reader(input);
    valid(r.take(6)? == b"AVBC\0\x01" && r.byte()? == class)?;
    let n = r.u64()?;
    bound(n, RECORD_LIMIT)?;
    valid(n <= number(r.0.len())? / 8)?;
    let mut out = Vec::new();
    for _ in 0..n {
        out.push(r.bytes(CATALOG_LIMIT)?);
    }
    r.end()?;
    Ok(out)
}
