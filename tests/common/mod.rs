//! Shared helpers for integration tests: hand-build OFZL v1 frames so each
//! test can exercise a single negative case without depending on the encoder.

#![allow(dead_code)]

pub const MAGIC: &[u8; 4] = b"OFZL";
pub const VERSION_V1: u8 = 1;
pub const KIND_U32_FIELD_LZ: u8 = 1;
pub const OPENZL_TYPE_NUMERIC: u8 = 4;
pub const STANDARD_TRANSFORM_ID_ZSTD: u64 = 22;
pub const STANDARD_TRANSFORM_ID_FIELD_LZ: u64 = 24;
pub const U32_WIDTH: u64 = 4;

pub fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

pub fn encode_varint(value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_varint(value, &mut out);
    out
}

pub fn write_header(out: &mut Vec<u8>, num_elements: u64, chunk_count: u64) {
    out.extend_from_slice(MAGIC);
    out.push(VERSION_V1);
    out.push(KIND_U32_FIELD_LZ);
    out.push(OPENZL_TYPE_NUMERIC);
    write_varint(U32_WIDTH, out);
    write_varint(num_elements, out);
    write_varint(chunk_count, out);
}

#[derive(Clone)]
pub struct StoredStream {
    pub stream_id: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone)]
pub struct TransformRecord {
    pub transform_id: u64,
    pub inputs: Vec<u64>,
    pub outputs: Vec<u64>,
    pub private_header: Vec<u8>,
}

#[derive(Clone)]
pub struct ChunkBuilder {
    pub chunk_num_elements: u64,
    pub stream_slot_count: u64,
    pub final_stream_id: u64,
    pub stored: Vec<StoredStream>,
    pub transforms: Vec<TransformRecord>,
}

impl ChunkBuilder {
    pub fn write(&self, out: &mut Vec<u8>) {
        write_varint(self.chunk_num_elements, out);
        write_varint(self.stream_slot_count, out);
        write_varint(self.stored.len() as u64, out);
        write_varint(self.transforms.len() as u64, out);
        write_varint(self.final_stream_id, out);
        for stored in &self.stored {
            write_varint(stored.stream_id, out);
            write_varint(stored.payload.len() as u64, out);
            out.extend_from_slice(&stored.payload);
        }
        for transform in &self.transforms {
            write_varint(transform.transform_id, out);
            write_varint(transform.inputs.len() as u64, out);
            for &input in &transform.inputs {
                write_varint(input, out);
            }
            write_varint(transform.outputs.len() as u64, out);
            for &output in &transform.outputs {
                write_varint(output, out);
            }
            write_varint(transform.private_header.len() as u64, out);
            out.extend_from_slice(&transform.private_header);
        }
    }
}

/// Baseline FieldLZ chunk: side streams stored directly, FieldLZ transform consumes them.
pub fn baseline_field_lz_chunk(
    chunk_num_elements: u64,
    side_streams: [Vec<u8>; 5],
) -> ChunkBuilder {
    let stored: Vec<StoredStream> = side_streams
        .into_iter()
        .enumerate()
        .map(|(idx, payload)| StoredStream {
            stream_id: idx as u64,
            payload,
        })
        .collect();
    ChunkBuilder {
        chunk_num_elements,
        stream_slot_count: 6,
        final_stream_id: 5,
        stored,
        transforms: vec![TransformRecord {
            transform_id: STANDARD_TRANSFORM_ID_FIELD_LZ,
            inputs: vec![0, 1, 2, 3, 4],
            outputs: vec![5],
            private_header: encode_varint(chunk_num_elements),
        }],
    }
}

pub fn build_frame(num_elements: u64, chunks: &[ChunkBuilder]) -> Vec<u8> {
    let mut out = Vec::new();
    write_header(&mut out, num_elements, chunks.len() as u64);
    for chunk in chunks {
        chunk.write(&mut out);
    }
    out
}

/// Convenience: a single-element FieldLZ chunk holding `value` as a literal.
pub fn single_literal_chunk(value: u32) -> ChunkBuilder {
    baseline_field_lz_chunk(
        1,
        [
            value.to_le_bytes().to_vec(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ],
    )
}
