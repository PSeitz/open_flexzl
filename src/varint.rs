//! Canonical unsigned LEB128/LEBU64 helpers used by the native frame format.
//!
//! Decoding rejects overlong/non-canonical encodings so every integer has a
//! single wire representation.

use crate::Error;

pub(crate) fn write_u64(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
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
        let byte = *self.input.get(self.pos).ok_or(Error::UnexpectedEof)?;
        self.pos = self
            .pos
            .checked_add(1)
            .ok_or(Error::InvalidFrame("reader position overflow"))?;
        let _ = what;
        Ok(byte)
    }

    pub(crate) fn read_exact(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(Error::InvalidFrame("byte length overflow"))?;
        let bytes = self.input.get(self.pos..end).ok_or(Error::UnexpectedEof)?;
        self.pos = end;
        Ok(bytes)
    }

    pub(crate) fn read_u64(&mut self, _what: &'static str) -> Result<u64, Error> {
        let mut result = 0u64;
        for i in 0..10 {
            let byte = self.read_u8("varint byte")?;
            let payload = (byte & 0x7f) as u64;
            if i == 9 && payload > 1 {
                return Err(Error::InvalidVarint("u64 overflow"));
            }
            result |= payload << (i * 7);
            if byte & 0x80 == 0 {
                if i > 0 && result < (1u64 << (i * 7)) {
                    return Err(Error::InvalidVarint("non-canonical encoding"));
                }
                return Ok(result);
            }
        }
        Err(Error::InvalidVarint("more than 10 bytes"))
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
