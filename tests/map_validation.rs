//! Negative tests covering chunk decoding-map validation and frame-header
//! checks. Each test starts from a hand-built valid baseline and mutates a
//! single field, so the failure mode is unambiguous.

mod common;

use common::*;
use open_flexzl::decompress_u32;

fn baseline_single_value_frame() -> Vec<u8> {
    build_frame(1, &[single_literal_chunk(0x1122_3344)])
}

#[test]
fn baseline_single_value_round_trips() {
    let frame = baseline_single_value_frame();
    assert_eq!(decompress_u32(&frame).unwrap(), vec![0x1122_3344]);
}

#[test]
fn rejects_unsupported_version() {
    let mut frame = baseline_single_value_frame();
    frame[4] = 0x02;
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_unsupported_kind() {
    let mut frame = baseline_single_value_frame();
    frame[5] = 0x02;
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_unsupported_final_output_type() {
    let mut frame = baseline_single_value_frame();
    frame[6] = OPENZL_TYPE_NUMERIC + 1;
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_unsupported_final_output_element_width() {
    // Header layout up to and including final_output_elt_width is
    // [magic 4][ver 1][kind 1][type 1][elt_width varint]. Replace width 4
    // with 8.
    let mut frame = baseline_single_value_frame();
    frame[7] = 0x08;
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_chunk_count_zero_with_nonempty_input() {
    // num_elements=1, chunk_count=0 contradicts the (chunk_count==0)<=>(num_elements==0) rule.
    let mut frame = Vec::new();
    write_header(&mut frame, 1, 0);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_chunk_count_nonzero_with_empty_input() {
    let mut frame = Vec::new();
    write_header(&mut frame, 0, 1);
    // Even malformed chunk content: top-level mismatch should fail first.
    frame.push(0); // dummy chunk_num_elements varint=0 (also invalid)
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_chunk_num_elements_zero() {
    let bad = ChunkBuilder {
        chunk_num_elements: 0,
        ..single_literal_chunk(0x42)
    };
    let frame = build_frame(1, &[bad]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_chunk_num_elements_above_max() {
    // MAX_CHUNK_ELEMENTS_U32 == 4_194_304; one above is invalid.
    let mut chunk = single_literal_chunk(0x42);
    chunk.chunk_num_elements = 4_194_305;
    let frame = build_frame(4_194_305, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_chunk_total_below_frame_total() {
    // Frame says num_elements=2, single chunk has 1.
    let frame = build_frame(2, &[single_literal_chunk(0x42)]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_chunk_total_above_frame_total() {
    // Frame says num_elements=1, single chunk has 2.
    let chunk = baseline_field_lz_chunk(
        2,
        [vec![0u8; 8], Vec::new(), Vec::new(), Vec::new(), Vec::new()],
    );
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_final_stream_id_out_of_range() {
    let mut chunk = single_literal_chunk(0x42);
    chunk.final_stream_id = chunk.stream_slot_count;
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_stored_stream_id_out_of_range() {
    let mut chunk = single_literal_chunk(0x42);
    chunk.stored[0].stream_id = chunk.stream_slot_count;
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_duplicate_stored_stream_slot() {
    let mut chunk = single_literal_chunk(0x42);
    // Two stored streams share slot 0; this should be flagged before the
    // FieldLZ transform consumes inputs.
    chunk.stored[1].stream_id = 0;
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_undefined_stream_slot() {
    // stream_slot_count=7 leaves slot 6 undefined.
    let mut chunk = single_literal_chunk(0x42);
    chunk.stream_slot_count = 7;
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_non_final_unused_stream() {
    // Add an extra stored stream at slot 6 that is never referenced.
    let mut chunk = single_literal_chunk(0x42);
    chunk.stream_slot_count = 7;
    chunk.stored.push(StoredStream {
        stream_id: 6,
        payload: vec![0xaa, 0xbb],
    });
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_unsupported_transform_id() {
    let mut chunk = single_literal_chunk(0x42);
    chunk.transforms[0].transform_id = 99;
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_more_than_one_field_lz_transform() {
    // Two FieldLZ transforms, both consuming the same five inputs and producing
    // distinct outputs. The second one violates the single-FieldLZ rule.
    let mut chunk = single_literal_chunk(0x42);
    chunk.stream_slot_count = 7;
    chunk.transforms.push(TransformRecord {
        transform_id: STANDARD_TRANSFORM_ID_FIELD_LZ,
        inputs: vec![0, 1, 2, 3, 4],
        outputs: vec![6],
        private_header: encode_varint(1),
    });
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_duplicated_transform_output_id() {
    // Make a zstd-like transform record with output ids [5, 5].
    let mut chunk = single_literal_chunk(0x42);
    chunk.stream_slot_count = 7;
    chunk.transforms.insert(
        0,
        TransformRecord {
            transform_id: STANDARD_TRANSFORM_ID_ZSTD,
            inputs: vec![0],
            outputs: vec![6, 6],
            private_header: encode_varint(4),
        },
    );
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_transform_input_count_above_limit() {
    // RUNTIME_TRANSFORM_INPUT_LIMIT == 2048; a varint of 2049 trips the limit
    // check before any input ids are read.
    let mut frame = Vec::new();
    write_header(&mut frame, 1, 1);
    write_varint(1, &mut frame); // chunk_num_elements
    write_varint(6, &mut frame); // stream_slot_count
    write_varint(0, &mut frame); // stored_stream_count
    write_varint(1, &mut frame); // transform_count
    write_varint(5, &mut frame); // final_stream_id
    write_varint(STANDARD_TRANSFORM_ID_FIELD_LZ, &mut frame);
    write_varint(2049, &mut frame); // input_count above limit
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_transform_output_count_above_limit() {
    // TRANSFORM_OUT_STREAM_LIMIT == 100_000.
    let mut frame = Vec::new();
    write_header(&mut frame, 1, 1);
    write_varint(1, &mut frame);
    write_varint(6, &mut frame);
    write_varint(0, &mut frame);
    write_varint(1, &mut frame);
    write_varint(5, &mut frame);
    write_varint(STANDARD_TRANSFORM_ID_FIELD_LZ, &mut frame);
    write_varint(0, &mut frame); // input_count = 0 (also fails FieldLZ contract, but we expect the limit check on outputs to fire after we read inputs)
    write_varint(100_001, &mut frame); // output_count above limit
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_stream_slot_count_above_limit() {
    // RUNTIME_STREAM_LIMIT == 110_000.
    let mut frame = Vec::new();
    write_header(&mut frame, 1, 1);
    write_varint(1, &mut frame);
    write_varint(110_001, &mut frame);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_transform_count_above_limit() {
    // RUNTIME_TRANSFORM_LIMIT == 20_000.
    let mut frame = Vec::new();
    write_header(&mut frame, 1, 1);
    write_varint(1, &mut frame);
    write_varint(6, &mut frame);
    write_varint(0, &mut frame);
    write_varint(20_001, &mut frame);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_stored_stream_count_above_slot_count() {
    let mut frame = Vec::new();
    write_header(&mut frame, 1, 1);
    write_varint(1, &mut frame);
    write_varint(6, &mut frame);
    write_varint(7, &mut frame); // stored_stream_count > stream_slot_count
    write_varint(1, &mut frame);
    write_varint(5, &mut frame);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_zstd_transform_with_empty_input() {
    // A zstd transform fed an empty stored stream must be rejected.
    let chunk = ChunkBuilder {
        chunk_num_elements: 1,
        stream_slot_count: 7,
        final_stream_id: 6,
        stored: vec![
            StoredStream {
                stream_id: 0,
                payload: Vec::new(),
            }, // would-be zstd input, empty
            StoredStream {
                stream_id: 2,
                payload: Vec::new(),
            },
            StoredStream {
                stream_id: 3,
                payload: Vec::new(),
            },
            StoredStream {
                stream_id: 4,
                payload: Vec::new(),
            },
            StoredStream {
                stream_id: 5,
                payload: Vec::new(),
            },
        ],
        transforms: vec![
            TransformRecord {
                transform_id: STANDARD_TRANSFORM_ID_ZSTD,
                inputs: vec![0],
                outputs: vec![1],
                private_header: encode_varint(U32_WIDTH),
            },
            TransformRecord {
                transform_id: STANDARD_TRANSFORM_ID_FIELD_LZ,
                inputs: vec![1, 2, 3, 4, 5],
                outputs: vec![6],
                private_header: encode_varint(1),
            },
        ],
    };
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_field_lz_private_header_chunk_length_mismatch() {
    let mut chunk = single_literal_chunk(0x42);
    // Private header says 7 elements but chunk record says 1.
    chunk.transforms[0].private_header = encode_varint(7);
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_trailing_bytes_after_final_chunk() {
    let mut frame = baseline_single_value_frame();
    frame.push(0xff);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_truncated_chunk_record() {
    let mut frame = baseline_single_value_frame();
    // Drop the last 4 bytes — the FieldLZ private header / output id.
    frame.truncate(frame.len() - 4);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_final_stream_not_produced_by_field_lz() {
    // Build a chunk where the final stream is a stored stream, no FieldLZ at all.
    let chunk = ChunkBuilder {
        chunk_num_elements: 1,
        stream_slot_count: 1,
        final_stream_id: 0,
        stored: vec![StoredStream {
            stream_id: 0,
            payload: 0x42u32.to_le_bytes().to_vec(),
        }],
        transforms: Vec::new(),
    };
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}

#[test]
fn rejects_field_lz_input_count_not_five() {
    // FieldLZ transform with only 4 inputs.
    let mut chunk = single_literal_chunk(0x42);
    chunk.transforms[0].inputs.pop();
    let frame = build_frame(1, &[chunk]);
    assert!(decompress_u32(&frame).is_err());
}
