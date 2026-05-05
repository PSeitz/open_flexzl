//! Binary golden fixtures for the OFZL v1 frame format.
//!
//! These fixtures cover the three semantic vectors in `plan.md`. All side
//! streams are below `DEFAULT_MIN_STREAM_SIZE` (10 bytes) and therefore go
//! through direct stored streams, so the encoded bytes are independent of the
//! zstd library version.
//!
//! When a zstd-dependent fixture is added in the future, document the exact
//! `zstd` crate version it was generated against in this file.

use open_flexzl::{compress_u32, decompress_u32};

const EMPTY_FRAME: &[u8] = &[
    // header
    b'O', b'F', b'Z', b'L', // magic
    0x01, // version
    0x01, // kind = u32 FieldLZ
    0x04, // final_output_type = NUMERIC
    0x04, // final_output_elt_width
    0x00, // num_elements
    0x00, // chunk_count
];

const ONE_LITERAL_FRAME: &[u8] = &[
    // header: num_elements=1, chunk_count=1
    b'O', b'F', b'Z', b'L', 0x01, 0x01, 0x04, 0x04, 0x01, 0x01,
    // chunk: chunk_num_elements=1, slots=6, stored=5, transforms=1, final=5
    0x01, 0x06, 0x05, 0x01, 0x05, // stored 0 (literals): id=0, len=4, payload 0x11223344 LE
    0x00, 0x04, 0x44, 0x33, 0x22, 0x11, // stored 1 (tokens): id=1, len=0
    0x01, 0x00, // stored 2 (offsets): id=2, len=0
    0x02, 0x00, // stored 3 (extra_ll): id=3, len=0
    0x03, 0x00, // stored 4 (extra_ml): id=4, len=0
    0x04,
    0x00, // transform: id=24 (FieldLZ), 5 inputs [0..=4], 1 output [5], private=varint(1)
    0x18, 0x05, 0x00, 0x01, 0x02, 0x03, 0x04, 0x01, 0x05, 0x01, 0x01,
];

const REPEATED_PAIR_FRAME: &[u8] = &[
    // header: num_elements=4, chunk_count=1
    b'O', b'F', b'Z', b'L', 0x01, 0x01, 0x04, 0x04, 0x04, 0x01,
    // chunk: chunk_num_elements=4
    0x04, 0x06, 0x05, 0x01, 0x05, // stored 0 (literals): [7, 8] LE = 8 bytes
    0x00, 0x08, 0x07, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00,
    // stored 1 (tokens): [0x004b] LE = 2 bytes
    0x01, 0x02, 0x4b, 0x00, // stored 2 (offsets): [2] LE = 4 bytes
    0x02, 0x04, 0x02, 0x00, 0x00, 0x00, // stored 3, stored 4: empty
    0x03, 0x00, 0x04, 0x00, // transform: FieldLZ, private=varint(4)
    0x18, 0x05, 0x00, 0x01, 0x02, 0x03, 0x04, 0x01, 0x05, 0x01, 0x04,
];

const REPEATED_RUN_FRAME: &[u8] = &[
    // header: num_elements=5, chunk_count=1
    b'O', b'F', b'Z', b'L', 0x01, 0x01, 0x04, 0x04, 0x05, 0x01,
    // chunk: chunk_num_elements=5
    0x05, 0x06, 0x05, 0x01, 0x05, // stored 0 (literals): [5] LE = 4 bytes
    0x00, 0x04, 0x05, 0x00, 0x00, 0x00, // stored 1 (tokens): [0x00c7] LE = 2 bytes
    0x01, 0x02, 0xc7, 0x00, // stored 2 (offsets): [1] LE = 4 bytes
    0x02, 0x04, 0x01, 0x00, 0x00, 0x00, // stored 3, stored 4: empty
    0x03, 0x00, 0x04, 0x00, // transform: FieldLZ, private=varint(5)
    0x18, 0x05, 0x00, 0x01, 0x02, 0x03, 0x04, 0x01, 0x05, 0x01, 0x05,
];

#[track_caller]
fn assert_golden(input: &[u32], expected: &[u8]) {
    let encoded = compress_u32(input).expect("encode succeeds");
    assert_eq!(
        encoded, expected,
        "encoded bytes for {:?} did not match golden fixture",
        input
    );
    let decoded = decompress_u32(expected).expect("decode succeeds");
    assert_eq!(decoded, input, "decoded values did not match input");
}

#[test]
fn golden_empty_frame() {
    assert_golden(&[], EMPTY_FRAME);
}

#[test]
fn golden_one_literal_frame() {
    assert_golden(&[0x1122_3344], ONE_LITERAL_FRAME);
}

#[test]
fn golden_repeated_pair_frame() {
    assert_golden(&[7, 8, 7, 8], REPEATED_PAIR_FRAME);
}

#[test]
fn golden_repeated_run_frame() {
    assert_golden(&[5, 5, 5, 5, 5], REPEATED_RUN_FRAME);
}
