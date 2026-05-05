//! Property tests covering round-trip correctness on arbitrary and structured
//! `Vec<u32>` inputs.

use open_flexzl::{compress_u32, decompress_u32};
use proptest::collection::vec;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 1024,
        ..ProptestConfig::default()
    })]

    #[test]
    fn round_trip_arbitrary(input in vec(any::<u32>(), 0..1024)) {
        let frame = compress_u32(&input).unwrap();
        let decoded = decompress_u32(&frame).unwrap();
        prop_assert_eq!(decoded, input);
    }

    #[test]
    fn round_trip_low_cardinality(
        cardinality in 1u32..16,
        len in 0usize..2048,
        seed in any::<u64>(),
    ) {
        let mut state = seed;
        let input: Vec<u32> = (0..len).map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u32 % cardinality
        }).collect();
        let frame = compress_u32(&input).unwrap();
        let decoded = decompress_u32(&frame).unwrap();
        prop_assert_eq!(decoded, input);
    }

    #[test]
    fn round_trip_repeated_blocks(
        block in vec(any::<u32>(), 1..32),
        repeats in 1usize..64,
    ) {
        let mut input = Vec::with_capacity(block.len() * repeats);
        for _ in 0..repeats {
            input.extend_from_slice(&block);
        }
        let frame = compress_u32(&input).unwrap();
        let decoded = decompress_u32(&frame).unwrap();
        prop_assert_eq!(decoded, input);
    }

    #[test]
    fn round_trip_monotonic(start in any::<u32>(), step in 0u32..256, len in 0usize..2048) {
        let input: Vec<u32> = (0..len).map(|i| start.wrapping_add(step.wrapping_mul(i as u32))).collect();
        let frame = compress_u32(&input).unwrap();
        let decoded = decompress_u32(&frame).unwrap();
        prop_assert_eq!(decoded, input);
    }

    #[test]
    fn round_trip_mixed_runs(
        segments in vec((any::<u32>(), 1usize..32), 1..32),
    ) {
        let mut input = Vec::new();
        for (value, count) in segments {
            for _ in 0..count {
                input.push(value);
            }
        }
        let frame = compress_u32(&input).unwrap();
        let decoded = decompress_u32(&frame).unwrap();
        prop_assert_eq!(decoded, input);
    }
}
