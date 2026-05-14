//! Native literal dictionary/categorical transform for low-cardinality `u32` literals.
//!
//! The transform stores a chunk-local table of unique `u32` literal values plus
//! a stream of compact indexes into that table. Decoding expands those indexes
//! back into the ordinary little-endian literal byte stream consumed by FieldLZ.

use rustc_hash::FxHashMap;

use crate::constants::{
    NATIVE_TRANSFORM_ID_LITERAL_DICT_U32, STANDARD_TRANSFORM_ID_ZSTD, U16_WIDTH, U32_WIDTH,
};
use crate::frame::{SideStreamRoute, StoredStreamRecord, TransformRecord};
use crate::varint;
use crate::Error;

/// Encoder-side dictionary candidate for a complete `u32` literal byte stream.
pub(crate) struct LiteralDictCandidate {
    /// Encounter-order dictionary table, stored as little-endian unique `u32`
    /// values. Code indexes refer to entries in this table.
    pub(crate) value_bytes: Vec<u8>,
    /// Per-literal dictionary indexes. Each index selects one table entry and
    /// expands back to one little-endian `u32` literal value during decode.
    pub(crate) dict_ord: Vec<u8>,
    /// Width in bytes of each code entry. The first encoder route emits `1`
    /// (`u8` codes); the decoder also accepts `2` for future `u16` routes.
    pub(crate) code_width: usize,
}

pub(crate) struct LiteralDictRouteChoice {
    pub(crate) candidate: LiteralDictCandidate,
    pub(crate) encoded_codes: EncodedSideStreamPayload,
}

pub(crate) struct EncodedSideStreamPayload {
    pub(crate) payload: Vec<u8>,
    pub(crate) is_zstd: bool,
}

pub(crate) fn build_side_stream_route(
    choice: LiteralDictRouteChoice,
    mut next_stream_id: usize,
) -> SideStreamRoute {
    let LiteralDictRouteChoice {
        candidate,
        encoded_codes,
    } = choice;
    let code_width = candidate.code_width;

    let dictionary_table_stream_id = next_stream_id;
    next_stream_id += 1;
    let stored_code_stream_id = next_stream_id;
    next_stream_id += 1;

    let mut transforms = Vec::with_capacity(2);
    let decoded_code_stream_id = if encoded_codes.is_zstd {
        let stream_id = next_stream_id;
        next_stream_id += 1;
        transforms.push(TransformRecord {
            transform_id: STANDARD_TRANSFORM_ID_ZSTD,
            inputs: vec![stored_code_stream_id],
            outputs: vec![stream_id],
            private_header: varint::encode_u64(code_width as u64),
        });
        stream_id
    } else {
        stored_code_stream_id
    };

    let decoded_literal_stream_id = next_stream_id;
    next_stream_id += 1;
    transforms.push(TransformRecord {
        transform_id: NATIVE_TRANSFORM_ID_LITERAL_DICT_U32,
        inputs: vec![dictionary_table_stream_id, decoded_code_stream_id],
        outputs: vec![decoded_literal_stream_id],
        private_header: varint::encode_u64(code_width as u64),
    });

    SideStreamRoute {
        stored_streams: vec![
            StoredStreamRecord {
                stream_id: dictionary_table_stream_id,
                payload: candidate.value_bytes,
            },
            StoredStreamRecord {
                stream_id: stored_code_stream_id,
                payload: encoded_codes.payload,
            },
        ],
        transforms,
        output_stream_id: decoded_literal_stream_id,
        next_stream_id,
    }
}

/// Build an encounter-order dictionary using one-byte codes.
///
/// Returns `None` when the stream is not a complete `u32` stream, is too small
/// to plausibly amortize the transform/table overhead, or has more than 256
/// distinct values.
pub(crate) fn build_u8_candidate(bytes: &[u8]) -> Option<LiteralDictCandidate> {
    build_candidate(bytes, u8::MAX as usize + 1, 1, |index, dict_ord| {
        dict_ord.push(index as u8);
    })
}

/// Build an encounter-order dictionary using two-byte little-endian codes.
pub(crate) fn build_u16_candidate(bytes: &[u8]) -> Option<LiteralDictCandidate> {
    build_candidate(bytes, u16::MAX as usize + 1, 2, |index, dict_ord| {
        dict_ord.extend_from_slice(&(index as u16).to_le_bytes());
    })
}

fn build_candidate(
    bytes: &[u8],
    max_values: usize,
    code_width: usize,
    mut push_code: impl FnMut(usize, &mut Vec<u8>),
) -> Option<LiteralDictCandidate> {
    if bytes.len() < 256 || !bytes.len().is_multiple_of(U32_WIDTH) {
        return None;
    }

    let element_count = bytes.len() / U32_WIDTH;
    let mut val_to_dict_ord = FxHashMap::<u32, usize>::default();
    let mut values = Vec::<u32>::new();
    let mut dict_ord = Vec::with_capacity(element_count * code_width);

    for chunk in bytes.chunks_exact(U32_WIDTH) {
        let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let index = if let Some(&index) = val_to_dict_ord.get(&value) {
            index
        } else {
            if values.len() == max_values {
                return None;
            }
            let index = values.len();
            values.push(value);
            val_to_dict_ord.insert(value, index);
            index
        };
        push_code(index, &mut dict_ord);
    }

    // A dictionary with no repetition is just table + codes overhead. Require a
    // real low-cardinality signal before spending zstd work in the route gate.
    if values.len() >= element_count {
        return None;
    }

    let value_bytes = values.into_iter().flat_map(u32::to_le_bytes).collect();

    Some(LiteralDictCandidate {
        value_bytes,
        dict_ord,
        code_width,
    })
}

pub(crate) fn decode(
    value_bytes: &[u8],
    dict_ord: &[u8],
    code_width: usize,
) -> Result<Vec<u8>, Error> {
    if code_width != 1 && code_width != 2 {
        return Err(Error::InvalidTransform(
            "literal_dict_u32 code width must be 1 or 2",
        ));
    }
    if !value_bytes.len().is_multiple_of(U32_WIDTH) {
        return Err(Error::InvalidTransform(
            "literal_dict_u32 dictionary table length must be a multiple of 4",
        ));
    }
    if code_width == 2 && !dict_ord.len().is_multiple_of(U16_WIDTH) {
        return Err(Error::InvalidTransform(
            "literal_dict_u32 u16 code stream length must be a multiple of 2",
        ));
    }

    let table: Vec<u32> = value_bytes
        .chunks_exact(U32_WIDTH)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    if table.is_empty() && !dict_ord.is_empty() {
        return Err(Error::InvalidTransform(
            "literal_dict_u32 code references an empty dictionary",
        ));
    }

    let code_count = dict_ord.len() / code_width;
    let mut out = Vec::with_capacity(code_count.checked_mul(U32_WIDTH).ok_or(
        Error::InvalidTransform("literal_dict_u32 output length overflow"),
    )?);

    for chunk in dict_ord.chunks_exact(code_width) {
        let code = match code_width {
            1 => chunk[0] as usize,
            2 => u16::from_le_bytes([chunk[0], chunk[1]]) as usize,
            _ => unreachable!(),
        };
        let value = *table.get(code).ok_or(Error::InvalidTransform(
            "literal_dict_u32 code is out of dictionary range",
        ))?;
        out.extend_from_slice(&value.to_le_bytes());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u8_candidate_round_trips() {
        let values: Vec<u32> = (0..512).map(|i| [11, 22, 11, 33][i % 4]).collect();
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        let candidate = build_u8_candidate(&bytes).unwrap();
        assert_eq!(candidate.code_width, 1);
        assert_eq!(
            decode(&candidate.value_bytes, &candidate.dict_ord, 1).unwrap(),
            bytes
        );
    }

    #[test]
    fn u16_candidate_round_trips() {
        let values: Vec<u32> = (0..1_024).map(|i| (i % 300) as u32).collect();
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        let candidate = build_u16_candidate(&bytes).unwrap();
        assert_eq!(candidate.code_width, 2);
        assert_eq!(
            decode(&candidate.value_bytes, &candidate.dict_ord, 2).unwrap(),
            bytes
        );
    }

    #[test]
    fn decode_rejects_out_of_range_code() {
        let table = 123u32.to_le_bytes();
        let err = decode(&table, &[1], 1).unwrap_err().to_string();
        assert!(err.contains("out of dictionary range"));
    }
}
