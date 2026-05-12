//! Canonical unsigned LEB128/LEBU64 helpers used by the native frame format.
//!
//! Decoding rejects overlong/non-canonical encodings so every integer has a
//! single wire representation.

use crate::Error;

const U64_VARINT_MAX_BYTES: usize = 10;
const VARINT_CONTINUATION_BIT: u8 = 1 << 7;
const VARINT_PAYLOAD_MASK: u8 = VARINT_CONTINUATION_BIT - 1;

pub(crate) fn write_u64(mut value: u64, out: &mut Vec<u8>) {
    while value >= u64::from(VARINT_CONTINUATION_BIT) {
        out.push((value as u8 & VARINT_PAYLOAD_MASK) | VARINT_CONTINUATION_BIT);
        value >>= 7;
    }
    out.push(value as u8);
}

pub(crate) fn write_usize(value: usize, out: &mut Vec<u8>) {
    write_u64(value as u64, out);
}

pub(crate) fn encode_u64(value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_u64(value, &mut out);
    out
}

pub(crate) fn read_single_usize(bytes: &[u8], what: &'static str) -> Result<usize, Error> {
    let mut reader = Reader::new(bytes);
    let value = reader.read_usize(what)?;
    if !reader.is_eof() {
        return Err(Error::InvalidVarint("private header has trailing bytes"));
    }
    Ok(value)
}

#[derive(Clone, Copy)]
pub(crate) struct Reader<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    pub(crate) fn is_eof(&self) -> bool {
        self.pos == self.input.len()
    }

    pub(crate) fn read_u8(&mut self, what: &'static str) -> Result<u8, Error> {
        let byte = *self.input.get(self.pos).ok_or(Error::UnexpectedEof(what))?;
        self.pos += 1;
        Ok(byte)
    }

    pub(crate) fn read_exact(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], Error> {
        let end = self.pos + len;
        let bytes = self
            .input
            .get(self.pos..end)
            .ok_or(Error::UnexpectedEof(what))?;
        self.pos = end;
        Ok(bytes)
    }

    pub(crate) fn read_u64(&mut self, what: &'static str) -> Result<u64, Error> {
        let mut value = 0u64;

        for byte_index in 0..U64_VARINT_MAX_BYTES {
            let byte = self.read_u8(what)?;
            let payload = u64::from(byte & VARINT_PAYLOAD_MASK);
            let shift = byte_index * 7;

            if byte_index == U64_VARINT_MAX_BYTES - 1 && payload > 1 {
                return Err(Error::InvalidVarint("varint overflows u64"));
            }

            value |= payload << shift;
            if byte & VARINT_CONTINUATION_BIT != 0 {
                continue;
            }

            if byte_index > 0 && value < (1u64 << shift) {
                return Err(Error::InvalidVarint("non-canonical varint"));
            }
            return Ok(value);
        }

        Err(Error::InvalidVarint("varint exceeds 10 bytes"))
    }

    pub(crate) fn read_usize(&mut self, what: &'static str) -> Result<usize, Error> {
        let value = self.read_u64(what)?;
        usize::try_from(value).map_err(|_| Error::LimitExceeded("value does not fit usize"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_round_trip() {
        let values = [
            0,
            1,
            2,
            10,
            127,
            128,
            255,
            16_384,
            u32::MAX as u64,
            u64::MAX,
        ];
        for value in values {
            let encoded = encode_u64(value);
            let mut reader = Reader::new(&encoded);
            assert_eq!(reader.read_u64("value").unwrap(), value);
            assert!(reader.is_eof());
        }
    }

    #[test]
    fn rejects_non_canonical_varint() {
        let mut reader = Reader::new(&[0x80, 0x00]);
        assert!(reader.read_u64("value").is_err());

        let mut reader = Reader::new(&[0x81, 0x00]);
        assert!(reader.read_u64("value").is_err());
    }

    #[test]
    fn rejects_overflow_varint() {
        let mut reader = Reader::new(&[0xff; 10]);
        assert!(reader.read_u64("value").is_err());
    }
}
