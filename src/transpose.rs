//! Byte-lane transpose helpers for fixed-width `u32` literal streams.
//!
//! The encoder side splits an interleaved little-endian `u32` byte stream into
//! four byte-position lanes. The decoder-side transform recombines those lanes
//! before FieldLZ consumes the literal stream.

use crate::constants::{MAX_CHUNK_ELEMENTS_U32, U32_WIDTH};
use crate::Error;

pub(crate) fn should_try_literal_split4(bytes: &[u8]) -> bool {
    // split4 only applies to reasonably sized streams of complete little-endian
    // u32 literals; below this, four zstd streams plus a transpose transform is
    // usually overhead rather than a win.
    if bytes.len() < 256 || !bytes.len().is_multiple_of(U32_WIDTH) {
        return false;
    }

    let mut seen = [[false; 256]; U32_WIDTH];
    let mut cardinality = [0usize; U32_WIDTH];
    let mut stable_high_pairs = 0usize;
    let element_count = bytes.len() / U32_WIDTH;

    for element in bytes.chunks_exact(U32_WIDTH) {
        // Count values whose high 16 bits are all 0s or all 1s. Those are common
        // for small signed/unsigned integers and make the high-byte lanes highly
        // compressible after transposition.
        if (element[2] == 0 && element[3] == 0) || (element[2] == 0xff && element[3] == 0xff) {
            stable_high_pairs += 1;
        }
        for (lane, &byte) in element.iter().enumerate() {
            let seen_byte = &mut seen[lane][byte as usize];
            if !*seen_byte {
                *seen_byte = true;
                // Per-lane distinct byte count estimates how compressible each
                // lane will be when encoded independently.
                cardinality[lane] += 1;
            }
        }
    }

    // At least half the values look sign- or zero-extended, so lanes 2 and 3
    // should compress very well after transposition.
    if stable_high_pairs * 2 >= element_count {
        return true;
    }

    // Both high-byte lanes use only a small byte alphabet, which is another
    // common shape for small integers and narrow numeric ranges. Large streams
    // with an almost-constant top byte and a modest third-byte alphabet are the
    // same shape with a wider bounded magnitude; split4 exposes the cheap top
    // byte and compressible third-byte lane without trying competing routes.
    if (cardinality[2] <= 16 && cardinality[3] <= 16)
        || (element_count >= 100_000 && cardinality[2] <= 32 && cardinality[3] <= 4)
    {
        return true;
    }

    // Any two lanes are nearly constant, regardless of position, so splitting
    // is likely to expose long low-entropy streams.
    if cardinality.iter().filter(|&&count| count <= 4).count() >= 2 {
        return true;
    }

    false
}

pub(crate) fn encode_split4(bytes: &[u8]) -> [Vec<u8>; U32_WIDTH] {
    debug_assert!(bytes.len().is_multiple_of(U32_WIDTH));
    let element_count = bytes.len() / U32_WIDTH;
    let mut lanes: [Vec<u8>; U32_WIDTH] =
        std::array::from_fn(|_| Vec::with_capacity(element_count));
    for element in bytes.chunks_exact(U32_WIDTH) {
        for (lane, &byte) in lanes.iter_mut().zip(element) {
            lane.push(byte);
        }
    }
    lanes
}

pub(crate) fn decode_split4(
    lanes: [&[u8]; U32_WIDTH],
    chunk_num_elements: usize,
) -> Result<Vec<u8>, Error> {
    let element_count = lanes[0].len();
    if lanes.iter().any(|lane| lane.len() != element_count) {
        return Err(Error::InvalidTransform(
            "transpose_split4 input lanes must have equal lengths",
        ));
    }

    if element_count > chunk_num_elements {
        return Err(Error::InvalidTransform(
            "transpose_split4 output length exceeds chunk length",
        ));
    }
    debug_assert!(chunk_num_elements <= MAX_CHUNK_ELEMENTS_U32);

    let mut bytes = Vec::with_capacity(element_count * U32_WIDTH);
    for index in 0..element_count {
        for lane in &lanes {
            bytes.push(lane[index]);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field_lz;

    #[test]
    fn split4_gate_accepts_large_bounded_wide_tail_shape() {
        let input: Vec<u32> = (0..100_000u32)
            .map(|i| ((i % 18) << 16) | ((i.wrapping_mul(65_537)) & 0xffff))
            .collect();
        let literal_bytes = field_lz::u32s_to_le_bytes(&input);
        assert!(should_try_literal_split4(&literal_bytes));
    }

    #[test]
    fn split4_round_trips_literal_bytes() {
        let input = [0x1122_3344u32, 0xaabb_ccddu32, 0x0102_0304u32];
        let literal_bytes = field_lz::u32s_to_le_bytes(&input);
        let lanes = encode_split4(&literal_bytes);
        assert_eq!(lanes[0], vec![0x44, 0xdd, 0x04]);
        assert_eq!(lanes[1], vec![0x33, 0xcc, 0x03]);
        assert_eq!(lanes[2], vec![0x22, 0xbb, 0x02]);
        assert_eq!(lanes[3], vec![0x11, 0xaa, 0x01]);

        let lane_refs = [
            lanes[0].as_slice(),
            lanes[1].as_slice(),
            lanes[2].as_slice(),
            lanes[3].as_slice(),
        ];
        assert_eq!(
            decode_split4(lane_refs, input.len()).unwrap(),
            literal_bytes
        );
    }
}
