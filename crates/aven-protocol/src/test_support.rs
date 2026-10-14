/// Bytes split into a selector and length-prefixed fields.
pub struct Input<'a>(pub &'a [u8]);

impl<'a> Input<'a> {
    pub fn byte(&mut self) -> u8 {
        let Some((&b, rest)) = self.0.split_first() else {
            return 0;
        };
        self.0 = rest;
        b
    }
    /// A U16 big-endian length-prefixed field, truncated at the end of input.
    pub fn part(&mut self) -> &'a [u8] {
        let len = usize::from(u16::from_be_bytes([self.byte(), self.byte()]));
        let (part, rest) = self.0.split_at(len.min(self.0.len()));
        self.0 = rest;
        part
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Encodes raw selector bytes and fields so the last field is the unframed rest.
pub fn frame(head: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut out = head.to_vec();
    if let Some((last, framed)) = parts.split_last() {
        for part in framed {
            out.extend(u16::try_from(part.len()).expect("small part").to_be_bytes());
            out.extend(*part);
        }
        out.extend(*last);
    }
    out
}
