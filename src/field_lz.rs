use crate::constants::{MAX_OFFSET_ELEMENTS, U16_WIDTH, U32_WIDTH};
use crate::Error;

#[derive(Debug, Clone, Default)]
pub(crate) struct FieldLzStreams {
    pub(crate) literals: Vec<u8>,
    pub(crate) tokens: Vec<u8>,
    pub(crate) offsets: Vec<u8>,
    pub(crate) extra_literal_lengths: Vec<u8>,
    pub(crate) extra_match_lengths: Vec<u8>,
}

impl FieldLzStreams {
    pub(crate) fn as_array(&self) -> [&[u8]; 5] {
        [
            &self.literals,
            &self.tokens,
            &self.offsets,
            &self.extra_literal_lengths,
            &self.extra_match_lengths,
        ]
    }
}

pub(crate) fn parse_u32(input: &[u32]) -> Result<FieldLzStreams, Error> {
    if input.len() > crate::constants::MAX_CHUNK_ELEMENTS_U32 {
        return Err(Error::LimitExceeded("FieldLZ chunk element count"));
    }

    let mut out = FieldLzStreams::default();
    let mut table = HashTable::new(table_size(input.len()));
    let mut anchor = 0usize;
    let mut i = 0usize;

    while i + 1 < input.len() {
        let hash = hash_pair(input[i], input[i + 1], table.mask());
        let candidate_plus_one = table.get(hash);
        let candidate = candidate_plus_one.wrapping_sub(1) as usize;
        table.set(hash, (i + 1) as u32);

        if candidate_plus_one != 0 && candidate < i {
            let offset = i - candidate;
            if offset <= MAX_OFFSET_ELEMENTS {
                let match_len = count_match(input, candidate, i);
                if match_len >= 2 {
                    emit_literals(&input[anchor..i], &mut out.literals);
                    emit_sequence(i - anchor, match_len, offset, &mut out)?;

                    let match_end = i
                        .checked_add(match_len)
                        .ok_or(Error::InvalidFieldLz("match end overflow during encoding"))?;
                    insert_matched_range(input, i, match_end, &mut table);
                    i = match_end;
                    anchor = i;
                    continue;
                }
            }
        }

        i += 1;
    }

    emit_literals(&input[anchor..], &mut out.literals);
    Ok(out)
}

pub(crate) fn decode(inputs: [&[u8]; 5], chunk_num_elements: usize) -> Result<Vec<u8>, Error> {
    let literal_bytes = inputs[0];
    let token_bytes = inputs[1];
    let offset_bytes = inputs[2];
    let extra_ll_bytes = inputs[3];
    let extra_ml_bytes = inputs[4];

    let max_literal_bytes = chunk_num_elements
        .checked_mul(U32_WIDTH)
        .ok_or(Error::InvalidFieldLz("literal byte limit overflow"))?;
    let max_token_bytes = chunk_num_elements
        .checked_mul(U16_WIDTH)
        .ok_or(Error::InvalidFieldLz("token byte limit overflow"))?;

    if !literal_bytes.len().is_multiple_of(U32_WIDTH) {
        return Err(Error::InvalidFieldLz(
            "literal stream length is not a multiple of 4",
        ));
    }
    if literal_bytes.len() > max_literal_bytes {
        return Err(Error::InvalidFieldLz("literal stream is too long"));
    }
    if !token_bytes.len().is_multiple_of(U16_WIDTH) {
        return Err(Error::InvalidFieldLz(
            "token stream length is not a multiple of 2",
        ));
    }
    if token_bytes.len() > max_token_bytes {
        return Err(Error::InvalidFieldLz("token stream is too long"));
    }
    if !offset_bytes.len().is_multiple_of(U32_WIDTH)
        || !extra_ll_bytes.len().is_multiple_of(U32_WIDTH)
        || !extra_ml_bytes.len().is_multiple_of(U32_WIDTH)
    {
        return Err(Error::InvalidFieldLz(
            "offset and extra-length streams must be multiples of 4 bytes",
        ));
    }

    let token_count = token_bytes.len() / U16_WIDTH;
    let offset_count = offset_bytes.len() / U32_WIDTH;
    let extra_ll_count = extra_ll_bytes.len() / U32_WIDTH;
    let extra_ml_count = extra_ml_bytes.len() / U32_WIDTH;
    if offset_count > token_count || extra_ll_count > token_count || extra_ml_count > token_count {
        return Err(Error::InvalidFieldLz(
            "side-stream entry count exceeds token count",
        ));
    }

    let literals = read_u32_stream(literal_bytes);
    let tokens = read_u16_stream(token_bytes);
    let offsets = read_u32_stream(offset_bytes);
    let extra_ll = read_u32_stream(extra_ll_bytes);
    let extra_ml = read_u32_stream(extra_ml_bytes);

    let mut literal_pos = 0usize;
    let mut offset_pos = 0usize;
    let mut extra_ll_pos = 0usize;
    let mut extra_ml_pos = 0usize;
    let mut repeated_offsets = [1usize, 2, 4];
    let mut output = Vec::with_capacity(chunk_num_elements);

    for &token in &tokens {
        if token & 0xfc00 != 0 {
            return Err(Error::InvalidFieldLz("token reserved bits are non-zero"));
        }

        let offset_code = token & 0x0003;
        let literal_code = ((token >> 2) & 0x000f) as usize;
        let match_code = ((token >> 6) & 0x000f) as usize;

        let offset = match offset_code {
            0 => repeated_offsets[0],
            1 => {
                let offset = repeated_offsets[1];
                repeated_offsets = [
                    repeated_offsets[1],
                    repeated_offsets[0],
                    repeated_offsets[2],
                ];
                offset
            }
            2 => {
                let offset = repeated_offsets[2];
                repeated_offsets = [
                    repeated_offsets[2],
                    repeated_offsets[0],
                    repeated_offsets[1],
                ];
                offset
            }
            3 => {
                let raw = *offsets
                    .get(offset_pos)
                    .ok_or(Error::InvalidFieldLz("offset stream underflow"))?;
                offset_pos += 1;
                let offset = usize::try_from(raw)
                    .map_err(|_| Error::InvalidFieldLz("offset does not fit usize"))?;
                repeated_offsets = [offset, repeated_offsets[0], repeated_offsets[1]];
                offset
            }
            _ => unreachable!(),
        };

        let literal_len = if literal_code < 15 {
            literal_code
        } else {
            let extra = *extra_ll.get(extra_ll_pos).ok_or(Error::InvalidFieldLz(
                "extra literal length stream underflow",
            ))?;
            extra_ll_pos += 1;
            15usize
                .checked_add(usize::try_from(extra).map_err(|_| {
                    Error::InvalidFieldLz("extra literal length does not fit usize")
                })?)
                .ok_or(Error::InvalidFieldLz("literal length overflow"))?
        };

        let match_len =
            if match_code < 15 {
                1usize
                    .checked_add(match_code)
                    .ok_or(Error::InvalidFieldLz("match length overflow"))?
            } else {
                let extra = *extra_ml
                    .get(extra_ml_pos)
                    .ok_or(Error::InvalidFieldLz("extra match length stream underflow"))?;
                extra_ml_pos += 1;
                16usize
                    .checked_add(usize::try_from(extra).map_err(|_| {
                        Error::InvalidFieldLz("extra match length does not fit usize")
                    })?)
                    .ok_or(Error::InvalidFieldLz("match length overflow"))?
            };

        let literal_end = literal_pos
            .checked_add(literal_len)
            .ok_or(Error::InvalidFieldLz("literal position overflow"))?;
        if literal_end > literals.len() {
            return Err(Error::InvalidFieldLz("literal stream underflow"));
        }
        output.extend_from_slice(&literals[literal_pos..literal_end]);
        literal_pos = literal_end;

        if offset == 0 {
            return Err(Error::InvalidFieldLz("zero match offset"));
        }
        if offset > output.len() {
            return Err(Error::InvalidFieldLz("match offset points before output"));
        }
        let after_match = output
            .len()
            .checked_add(match_len)
            .ok_or(Error::InvalidFieldLz("output length overflow"))?;
        if after_match > chunk_num_elements {
            return Err(Error::InvalidFieldLz("match exceeds chunk output length"));
        }
        for _ in 0..match_len {
            let value = output[output.len() - offset];
            output.push(value);
        }
    }

    output.extend_from_slice(&literals[literal_pos..]);
    literal_pos = literals.len();

    if literal_pos != literals.len() {
        return Err(Error::InvalidFieldLz(
            "literal stream was not fully consumed",
        ));
    }
    if offset_pos != offsets.len() {
        return Err(Error::InvalidFieldLz(
            "offset stream was not fully consumed",
        ));
    }
    if extra_ll_pos != extra_ll.len() {
        return Err(Error::InvalidFieldLz(
            "extra literal length stream was not fully consumed",
        ));
    }
    if extra_ml_pos != extra_ml.len() {
        return Err(Error::InvalidFieldLz(
            "extra match length stream was not fully consumed",
        ));
    }
    if output.len() != chunk_num_elements {
        return Err(Error::InvalidFieldLz(
            "decoded output length does not match chunk length",
        ));
    }

    Ok(u32s_to_le_bytes(&output))
}

fn emit_sequence(
    literal_len: usize,
    match_len: usize,
    offset: usize,
    out: &mut FieldLzStreams,
) -> Result<(), Error> {
    if offset == 0 || offset > MAX_OFFSET_ELEMENTS {
        return Err(Error::InvalidFieldLz("encoder produced invalid offset"));
    }
    if match_len == 0 {
        return Err(Error::InvalidFieldLz("encoder produced zero-length match"));
    }

    let literal_code = if literal_len < 15 {
        literal_len as u16
    } else {
        let extra = literal_len - 15;
        push_u32(
            u32::try_from(extra).map_err(|_| Error::LimitExceeded("literal length extra"))?,
            &mut out.extra_literal_lengths,
        );
        15
    };

    let match_code = if match_len < 16 {
        (match_len - 1) as u16
    } else {
        let extra = match_len - 16;
        push_u32(
            u32::try_from(extra).map_err(|_| Error::LimitExceeded("match length extra"))?,
            &mut out.extra_match_lengths,
        );
        15
    };

    let token = 3u16 | (literal_code << 2) | (match_code << 6);
    push_u16(token, &mut out.tokens);
    push_u32(
        u32::try_from(offset).map_err(|_| Error::LimitExceeded("match offset"))?,
        &mut out.offsets,
    );
    Ok(())
}

fn insert_matched_range(input: &[u32], start: usize, end: usize, table: &mut HashTable) {
    // Mirror OpenZL's fast parser by inserting only a handful of hash
    // entries per match instead of every position in the range. For short
    // matches we still cover every interior position (cheap, useful);
    // beyond that we fall back to start+1 / end-1 plus periodic mid-points
    // so cyclic data still finds future matches after a phase shift.
    let limit = input.len().saturating_sub(1);
    let mut put = |pos: usize| {
        if pos > start && pos < end && pos < limit {
            let hash = hash_pair(input[pos], input[pos + 1], table.mask());
            table.set(hash, (pos + 1) as u32);
        }
    };

    let span = end.saturating_sub(start);
    if span < 16 {
        for pos in (start + 1)..end.saturating_sub(1) {
            put(pos);
        }
    } else {
        put(start + 1);
        let mut pos = start + 8;
        while pos + 1 < end {
            put(pos);
            pos += 8;
        }
        put(end.saturating_sub(1));
    }
}

fn count_match(input: &[u32], candidate: usize, current: usize) -> usize {
    let mut len = 0usize;
    while current + len < input.len() && input[candidate + len] == input[current + len] {
        len += 1;
    }
    len
}

fn emit_literals(literals: &[u32], out: &mut Vec<u8>) {
    out.reserve(literals.len() * U32_WIDTH);
    for &value in literals {
        push_u32(value, out);
    }
}

fn read_u16_stream(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(U16_WIDTH)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn read_u32_stream(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(U32_WIDTH)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

pub(crate) fn u32s_to_le_bytes(values: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * U32_WIDTH);
    for &value in values {
        push_u32(value, &mut bytes);
    }
    bytes
}

pub(crate) fn le_bytes_to_u32s(bytes: &[u8]) -> Result<Vec<u32>, Error> {
    if !bytes.len().is_multiple_of(U32_WIDTH) {
        return Err(Error::InvalidFieldLz(
            "u32 byte stream length is not a multiple of 4",
        ));
    }
    Ok(read_u32_stream(bytes))
}

fn push_u16(value: u16, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(value: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn table_size(input_len: usize) -> usize {
    input_len.next_power_of_two().clamp(1 << 12, 1 << 20)
}

fn hash_pair(a: u32, b: u32, mask: usize) -> usize {
    let mixed = ((a as u64) << 32) ^ b as u64;
    let hash = mixed.wrapping_mul(0x9e37_79b1_85eb_ca87);
    (hash as usize) & mask
}

struct HashTable {
    entries: Vec<u32>,
}

impl HashTable {
    fn new(size: usize) -> Self {
        debug_assert!(size.is_power_of_two());
        Self {
            entries: vec![0; size],
        }
    }

    fn mask(&self) -> usize {
        self.entries.len() - 1
    }

    fn get(&self, index: usize) -> u32 {
        self.entries[index]
    }

    fn set(&mut self, index: usize, value: u32) {
        self.entries[index] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16s(bytes: &[u8]) -> Vec<u16> {
        read_u16_stream(bytes)
    }

    fn u32s(bytes: &[u8]) -> Vec<u32> {
        read_u32_stream(bytes)
    }

    #[test]
    fn parses_repeated_pair_as_expected() {
        let streams = parse_u32(&[7, 8, 7, 8]).unwrap();
        assert_eq!(u32s(&streams.literals), vec![7, 8]);
        assert_eq!(u16s(&streams.tokens), vec![0x004b]);
        assert_eq!(u32s(&streams.offsets), vec![2]);
        assert!(streams.extra_literal_lengths.is_empty());
        assert!(streams.extra_match_lengths.is_empty());
    }

    #[test]
    fn parses_repeated_run_as_expected() {
        let streams = parse_u32(&[5, 5, 5, 5, 5]).unwrap();
        assert_eq!(u32s(&streams.literals), vec![5]);
        assert_eq!(u16s(&streams.tokens), vec![0x00c7]);
        assert_eq!(u32s(&streams.offsets), vec![1]);
        assert!(streams.extra_literal_lengths.is_empty());
        assert!(streams.extra_match_lengths.is_empty());
    }

    #[test]
    fn field_lz_round_trip_without_zstd() {
        let input = [1, 2, 1, 2, 1, 2, 9, 9, 9, 9, 3];
        let streams = parse_u32(&input).unwrap();
        let decoded = decode(streams.as_array(), input.len()).unwrap();
        assert_eq!(le_bytes_to_u32s(&decoded).unwrap(), input);
    }

    #[test]
    fn rejects_reserved_token_bits() {
        let literals = 0u32.to_le_bytes();
        let token = 0x0400u16.to_le_bytes();
        let inputs = [&literals[..], &token[..], &[][..], &[][..], &[][..]];
        assert!(decode(inputs, 1).is_err());
    }
}
