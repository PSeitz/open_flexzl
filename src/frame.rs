use std::collections::HashSet;

use crate::constants::*;
use crate::delta;
use crate::field_lz::{self, le_bytes_to_u32s, FieldLzStreams};
use crate::varint::{self, Reader};
use crate::{zstd_codec, Error};

pub(crate) fn compress_u32(input: &[u32]) -> Result<Vec<u8>, Error> {
    let chunk_count = if input.is_empty() {
        0
    } else {
        input.len().div_ceil(MAX_CHUNK_ELEMENTS_U32)
    };

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION_V1);
    out.push(KIND_U32_FIELD_LZ);
    out.push(OPENZL_TYPE_NUMERIC);
    varint::write_usize(U32_WIDTH, &mut out);
    varint::write_usize(input.len(), &mut out);
    varint::write_usize(chunk_count, &mut out);

    for chunk in input.chunks(MAX_CHUNK_ELEMENTS_U32) {
        write_chunk(chunk, &mut out)?;
    }

    Ok(out)
}

pub(crate) fn decompress_u32(input: &[u8]) -> Result<Vec<u32>, Error> {
    let mut reader = Reader::new(input);
    if reader.read_exact(MAGIC.len())? != MAGIC {
        return Err(Error::InvalidFrame("bad magic"));
    }
    if reader.read_u8("version")? != VERSION_V1 {
        return Err(Error::InvalidFrame("unsupported version"));
    }
    if reader.read_u8("kind")? != KIND_U32_FIELD_LZ {
        return Err(Error::InvalidFrame("unsupported kind"));
    }
    if reader.read_u8("final output type")? != OPENZL_TYPE_NUMERIC {
        return Err(Error::InvalidFrame("unsupported final output type"));
    }
    if reader.read_usize("final output element width")? != U32_WIDTH {
        return Err(Error::InvalidFrame(
            "unsupported final output element width",
        ));
    }

    let num_elements = reader.read_usize("num elements")?;
    let chunk_count = reader.read_usize("chunk count")?;
    if (chunk_count == 0) != (num_elements == 0) {
        return Err(Error::InvalidFrame(
            "chunk_count must be zero iff num_elements is zero",
        ));
    }

    let mut output = Vec::new();
    let mut total = 0usize;
    for _ in 0..chunk_count {
        let chunk = read_chunk(&mut reader)?;
        total = total
            .checked_add(chunk.len())
            .ok_or(Error::InvalidFrame("chunk element total overflow"))?;
        if total > num_elements {
            return Err(Error::InvalidFrame(
                "chunk element total exceeds frame element count",
            ));
        }
        output.extend_from_slice(&chunk);
    }

    if total != num_elements {
        return Err(Error::InvalidFrame(
            "chunk element total does not match frame element count",
        ));
    }
    if !reader.is_eof() {
        return Err(Error::TrailingBytes);
    }

    Ok(output)
}

fn write_chunk(chunk: &[u32], out: &mut Vec<u8>) -> Result<(), Error> {
    if chunk.is_empty() || chunk.len() > MAX_CHUNK_ELEMENTS_U32 {
        return Err(Error::InvalidFrame("invalid chunk element count"));
    }

    let raw = build_chunk_record(chunk, chunk, false)?;
    // Skip the delta probe when raw is already in either extreme — dense
    // matches (< 1% of source) or essentially incompressible (> 99%) — since
    // delta has no headroom to win in either case.
    let raw_bytes = chunk.len() * U32_WIDTH;
    let chosen = if raw.len() > raw_bytes / 100 && raw.len() < raw_bytes - raw_bytes / 100 {
        let deltas = delta::apply_u32(chunk);
        let dr = build_chunk_record(chunk, &deltas, true)?;
        if dr.len() < raw.len() { dr } else { raw }
    } else {
        raw
    };
    out.extend_from_slice(&chosen);
    Ok(())
}

fn build_chunk_record(
    chunk: &[u32],
    parser_input: &[u32],
    use_delta: bool,
) -> Result<Vec<u8>, Error> {
    let streams = field_lz::parse_u32(parser_input)?;
    let (stored_streams, transforms, final_stream_id, stream_slot_count) =
        plan_chunk_streams(&streams, chunk.len(), use_delta)?;

    let mut r = Vec::new();
    varint::write_usize(chunk.len(), &mut r);
    varint::write_usize(stream_slot_count, &mut r);
    varint::write_usize(stored_streams.len(), &mut r);
    varint::write_usize(transforms.len(), &mut r);
    varint::write_usize(final_stream_id, &mut r);

    for s in stored_streams {
        varint::write_usize(s.stream_id, &mut r);
        varint::write_usize(s.payload.len(), &mut r);
        r.extend_from_slice(&s.payload);
    }
    for t in transforms {
        varint::write_u64(t.transform_id, &mut r);
        varint::write_usize(t.inputs.len(), &mut r);
        for i in t.inputs {
            varint::write_usize(i, &mut r);
        }
        varint::write_usize(t.outputs.len(), &mut r);
        for o in t.outputs {
            varint::write_usize(o, &mut r);
        }
        varint::write_usize(t.private_header.len(), &mut r);
        r.extend_from_slice(&t.private_header);
    }
    Ok(r)
}

fn read_chunk(reader: &mut Reader<'_>) -> Result<Vec<u32>, Error> {
    let chunk_num_elements = reader.read_usize("chunk_num_elements")?;
    if chunk_num_elements == 0 || chunk_num_elements > MAX_CHUNK_ELEMENTS_U32 {
        return Err(Error::InvalidFrame("chunk element count out of range"));
    }

    let stream_slot_count = reader.read_usize("stream_slot_count")?;
    let stored_stream_count = reader.read_usize("stored_stream_count")?;
    let transform_count = reader.read_usize("transform_count")?;
    let final_stream_id = reader.read_usize("final_stream_id")?;

    if stream_slot_count > RUNTIME_STREAM_LIMIT {
        return Err(Error::LimitExceeded("stream slot count"));
    }
    if transform_count > RUNTIME_TRANSFORM_LIMIT {
        return Err(Error::LimitExceeded("transform count"));
    }
    if stored_stream_count > stream_slot_count {
        return Err(Error::InvalidMap(
            "stored_stream_count exceeds stream_slot_count",
        ));
    }
    if final_stream_id >= stream_slot_count {
        return Err(Error::InvalidMap("final stream id is out of range"));
    }

    let mut slots = vec![Slot::default(); stream_slot_count];

    for _ in 0..stored_stream_count {
        let stream_id = reader.read_usize("stored stream id")?;
        if stream_id >= stream_slot_count {
            return Err(Error::InvalidMap("stored stream id is out of range"));
        }
        if slots[stream_id].bytes.is_some() {
            return Err(Error::InvalidMap("stream slot defined more than once"));
        }
        let byte_len = reader.read_usize("stored stream byte length")?;
        let payload = reader.read_exact(byte_len)?.to_vec();
        slots[stream_id].bytes = Some(payload);
    }

    let mut log: Vec<ExecutedTransform> = Vec::with_capacity(transform_count);
    for _ in 0..transform_count {
        read_transform(reader, chunk_num_elements, &mut slots, &mut log)?;
    }

    if slots[final_stream_id].bytes.is_none() {
        return Err(Error::InvalidMap("final stream is undefined"));
    }
    let mut field_lz_outputs = log
        .iter()
        .filter(|t| t.id == STANDARD_TRANSFORM_ID_FIELD_LZ);
    let field_lz_output = field_lz_outputs
        .next()
        .map(|t| t.output)
        .ok_or(Error::InvalidMap(
            "chunk must contain exactly one FieldLZ transform",
        ))?;
    if field_lz_outputs.next().is_some() {
        return Err(Error::InvalidMap("more than one FieldLZ transform"));
    }
    let final_producer = log.iter().find(|t| t.output == final_stream_id);
    let valid_final = match final_producer {
        Some(t) if t.id == STANDARD_TRANSFORM_ID_FIELD_LZ => t.output == field_lz_output,
        Some(t) if t.id == STANDARD_TRANSFORM_ID_DELTA_INT => t.input == Some(field_lz_output),
        _ => false,
    };
    if !valid_final {
        return Err(Error::InvalidMap(
            "final stream must be produced by FieldLZ or by a delta_int consuming the FieldLZ output",
        ));
    }

    for (stream_id, slot) in slots.iter().enumerate() {
        if slot.bytes.is_none() {
            return Err(Error::InvalidMap("stream slot was never defined"));
        }
        if stream_id != final_stream_id && !slot.used {
            return Err(Error::InvalidMap(
                "non-final stream slot was never consumed",
            ));
        }
    }

    let final_bytes = slots[final_stream_id]
        .bytes
        .take()
        .ok_or(Error::InvalidMap("final stream is undefined"))?;
    let expected_bytes = chunk_num_elements
        .checked_mul(U32_WIDTH)
        .ok_or(Error::InvalidFrame("chunk byte length overflow"))?;
    if final_bytes.len() != expected_bytes {
        return Err(Error::InvalidMap(
            "final stream byte length does not match chunk element count",
        ));
    }
    le_bytes_to_u32s(&final_bytes)
}

fn read_transform(
    reader: &mut Reader<'_>,
    chunk_num_elements: usize,
    slots: &mut [Slot],
    log: &mut Vec<ExecutedTransform>,
) -> Result<(), Error> {
    let transform_id = reader.read_u64("transform_id")?;
    let input_count = reader.read_usize("input_count")?;
    if input_count > RUNTIME_TRANSFORM_INPUT_LIMIT {
        return Err(Error::LimitExceeded("transform input count"));
    }
    let mut inputs = Vec::with_capacity(input_count);
    for _ in 0..input_count {
        let stream_id = reader.read_usize("input stream id")?;
        validate_input_stream_id(stream_id, slots)?;
        inputs.push(stream_id);
    }

    let output_count = reader.read_usize("output_count")?;
    if output_count > TRANSFORM_OUT_STREAM_LIMIT {
        return Err(Error::LimitExceeded("transform output count"));
    }
    let mut outputs = Vec::with_capacity(output_count);
    let mut output_set = HashSet::with_capacity(output_count);
    for _ in 0..output_count {
        let stream_id = reader.read_usize("output stream id")?;
        if stream_id >= slots.len() {
            return Err(Error::InvalidMap("transform output id is out of range"));
        }
        if slots[stream_id].bytes.is_some() || !output_set.insert(stream_id) {
            return Err(Error::InvalidMap(
                "transform output stream defined more than once",
            ));
        }
        outputs.push(stream_id);
    }

    let private_header_len = reader.read_usize("private_header_len")?;
    let private_header = reader.read_exact(private_header_len)?.to_vec();

    for &input in &inputs {
        slots[input].used = true;
    }

    let (output_id, output_bytes, element_width) = match transform_id {
        STANDARD_TRANSFORM_ID_ZSTD => {
            execute_zstd_transform(&inputs, &outputs, &private_header, slots)?
        }
        STANDARD_TRANSFORM_ID_FIELD_LZ => execute_field_lz_transform(
            &inputs,
            &outputs,
            &private_header,
            chunk_num_elements,
            slots,
        )?,
        STANDARD_TRANSFORM_ID_DELTA_INT => {
            execute_delta_int_transform(&inputs, &outputs, &private_header, slots)?
        }
        other => return Err(Error::UnsupportedTransform(other)),
    };

    log.push(ExecutedTransform {
        id: transform_id,
        input: inputs.first().copied(),
        output: output_id,
    });
    slots[output_id].bytes = Some(output_bytes);
    slots[output_id].element_width = element_width;
    Ok(())
}

fn execute_zstd_transform(
    inputs: &[usize],
    outputs: &[usize],
    private_header: &[u8],
    slots: &[Slot],
) -> Result<(usize, Vec<u8>, Option<usize>), Error> {
    if inputs.len() != 1 || outputs.len() != 1 {
        return Err(Error::InvalidTransform(
            "zstd transform must have exactly one input and one output",
        ));
    }
    let output_elt_width = varint::read_single_usize(private_header, "zstd output width")?;
    let input = slots[inputs[0]]
        .bytes
        .as_deref()
        .ok_or(Error::InvalidMap("zstd input stream is undefined"))?;
    let output = zstd_codec::decode_magicless(input, output_elt_width)?;
    Ok((outputs[0], output, Some(output_elt_width)))
}

fn execute_delta_int_transform(
    inputs: &[usize],
    outputs: &[usize],
    private_header: &[u8],
    slots: &[Slot],
) -> Result<(usize, Vec<u8>, Option<usize>), Error> {
    if inputs.len() != 1 || outputs.len() != 1 || !private_header.is_empty() {
        return Err(Error::InvalidTransform(
            "delta_int transform must have one input, one output, and an empty private header",
        ));
    }
    if slots[inputs[0]].element_width.is_some_and(|w| w != U32_WIDTH) {
        return Err(Error::InvalidTransform(
            "delta_int input element width must be 4 for u32",
        ));
    }
    let input = slots[inputs[0]]
        .bytes
        .as_deref()
        .ok_or(Error::InvalidMap("delta_int input stream is undefined"))?;
    Ok((outputs[0], delta::undo_bytes(input)?, Some(U32_WIDTH)))
}

fn execute_field_lz_transform(
    inputs: &[usize],
    outputs: &[usize],
    private_header: &[u8],
    chunk_num_elements: usize,
    slots: &[Slot],
) -> Result<(usize, Vec<u8>, Option<usize>), Error> {
    if inputs.len() != FIELD_LZ_INPUT_COUNT || outputs.len() != 1 {
        return Err(Error::InvalidTransform(
            "FieldLZ transform must have five inputs and one output",
        ));
    }
    let declared_elements = varint::read_single_usize(private_header, "FieldLZ chunk length")?;
    if declared_elements != chunk_num_elements {
        return Err(Error::InvalidTransform(
            "FieldLZ private chunk length does not match chunk record",
        ));
    }

    let expected_widths = [U32_WIDTH, U16_WIDTH, U32_WIDTH, U32_WIDTH, U32_WIDTH];
    for (&stream_id, expected_width) in inputs.iter().zip(expected_widths) {
        if let Some(actual_width) = slots[stream_id].element_width {
            if actual_width != expected_width {
                return Err(Error::InvalidTransform(
                    "FieldLZ input element width does not match its stream role",
                ));
            }
        }
    }

    let input_refs = [
        slots[inputs[0]]
            .bytes
            .as_deref()
            .ok_or(Error::InvalidMap("FieldLZ input 0 is undefined"))?,
        slots[inputs[1]]
            .bytes
            .as_deref()
            .ok_or(Error::InvalidMap("FieldLZ input 1 is undefined"))?,
        slots[inputs[2]]
            .bytes
            .as_deref()
            .ok_or(Error::InvalidMap("FieldLZ input 2 is undefined"))?,
        slots[inputs[3]]
            .bytes
            .as_deref()
            .ok_or(Error::InvalidMap("FieldLZ input 3 is undefined"))?,
        slots[inputs[4]]
            .bytes
            .as_deref()
            .ok_or(Error::InvalidMap("FieldLZ input 4 is undefined"))?,
    ];
    let output = field_lz::decode(input_refs, chunk_num_elements)?;
    Ok((outputs[0], output, Some(U32_WIDTH)))
}

fn validate_input_stream_id(stream_id: usize, slots: &[Slot]) -> Result<(), Error> {
    if stream_id >= slots.len() {
        return Err(Error::InvalidMap("transform input id is out of range"));
    }
    if slots[stream_id].bytes.is_none() {
        return Err(Error::InvalidMap("transform input stream is undefined"));
    }
    Ok(())
}

fn plan_chunk_streams(
    streams: &FieldLzStreams,
    chunk_num_elements: usize,
    use_delta: bool,
) -> Result<(Vec<StoredStream>, Vec<TransformRecord>, usize, usize), Error> {
    let side_streams = streams.as_array();
    let widths = [U32_WIDTH, U16_WIDTH, U32_WIDTH, U32_WIDTH, U32_WIDTH];

    let mut stored_streams = Vec::with_capacity(FIELD_LZ_INPUT_COUNT);
    let mut transforms = Vec::with_capacity(FIELD_LZ_INPUT_COUNT + 2);
    let mut field_lz_inputs = Vec::with_capacity(FIELD_LZ_INPUT_COUNT);
    let mut next_stream_id = 0usize;

    for (bytes, width) in side_streams.into_iter().zip(widths) {
        if bytes.len() < DEFAULT_MIN_STREAM_SIZE {
            let stream_id = next_stream_id;
            next_stream_id += 1;
            stored_streams.push(StoredStream {
                stream_id,
                payload: bytes.to_vec(),
            });
            field_lz_inputs.push(stream_id);
        } else {
            let stored_id = next_stream_id;
            let decoded_id = next_stream_id + 1;
            next_stream_id += 2;
            let payload = zstd_codec::encode_magicless(bytes, DEFAULT_COMPRESSION_LEVEL)?;
            stored_streams.push(StoredStream {
                stream_id: stored_id,
                payload,
            });
            transforms.push(TransformRecord {
                transform_id: STANDARD_TRANSFORM_ID_ZSTD,
                inputs: vec![stored_id],
                outputs: vec![decoded_id],
                private_header: varint::encode_u64(width as u64),
            });
            field_lz_inputs.push(decoded_id);
        }
    }

    let field_lz_output_id = next_stream_id;
    next_stream_id += 1;
    transforms.push(TransformRecord {
        transform_id: STANDARD_TRANSFORM_ID_FIELD_LZ,
        inputs: field_lz_inputs,
        outputs: vec![field_lz_output_id],
        private_header: varint::encode_u64(chunk_num_elements as u64),
    });

    let final_stream_id = if use_delta {
        let delta_output_id = next_stream_id;
        next_stream_id += 1;
        transforms.push(TransformRecord {
            transform_id: STANDARD_TRANSFORM_ID_DELTA_INT,
            inputs: vec![field_lz_output_id],
            outputs: vec![delta_output_id],
            private_header: Vec::new(),
        });
        delta_output_id
    } else {
        field_lz_output_id
    };

    Ok((stored_streams, transforms, final_stream_id, next_stream_id))
}

struct StoredStream {
    stream_id: usize,
    payload: Vec<u8>,
}

struct TransformRecord {
    transform_id: u64,
    inputs: Vec<usize>,
    outputs: Vec<usize>,
    private_header: Vec<u8>,
}

#[derive(Clone, Default)]
struct Slot {
    bytes: Option<Vec<u8>>,
    used: bool,
    element_width: Option<usize>,
}

struct ExecutedTransform {
    id: u64,
    input: Option<usize>,
    output: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_frame_matches_plan_vector() {
        let frame = compress_u32(&[]).unwrap();
        assert_eq!(
            frame,
            vec![0x4f, 0x46, 0x5a, 0x4c, 0x01, 0x01, 0x04, 0x04, 0x00, 0x00]
        );
        assert_eq!(decompress_u32(&frame).unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn round_trip_basic_datasets() {
        let datasets: Vec<Vec<u32>> = vec![
            vec![0x1122_3344],
            vec![1, 2, 3, 4, 5, 6],
            vec![7, 8, 7, 8],
            vec![5, 5, 5, 5, 5],
            (0..1_000).collect(),
            (0..2_000).map(|i| (i % 7) as u32).collect(),
            (0..2_000u32)
                .map(|i| {
                    i.wrapping_mul(1_664_525)
                        .wrapping_add(1_013_904_223)
                        .rotate_left(i % 31)
                })
                .collect(),
        ];

        for dataset in datasets {
            let frame = compress_u32(&dataset).unwrap();
            let decoded = decompress_u32(&frame).unwrap();
            assert_eq!(decoded, dataset);
        }
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut frame = compress_u32(&[1, 2, 1, 2]).unwrap();
        frame.push(0);
        assert!(matches!(decompress_u32(&frame), Err(Error::TrailingBytes)));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut frame = compress_u32(&[]).unwrap();
        frame[0] = 0;
        assert!(decompress_u32(&frame).is_err());
    }

    #[test]
    fn direct_small_stream_store_is_used() {
        let frame = compress_u32(&[0x1122_3344]).unwrap();
        let zstd_magic = [0x28, 0xb5, 0x2f, 0xfd];
        assert!(!frame.windows(zstd_magic.len()).any(|w| w == zstd_magic));
        assert_eq!(decompress_u32(&frame).unwrap(), vec![0x1122_3344]);
    }

    #[test]
    fn rejects_reserved_token_bits_in_frame() {
        let frame = raw_field_lz_frame(
            1,
            &[
                0u32.to_le_bytes().as_slice(),
                &0x0400u16.to_le_bytes(),
                &[],
                &[],
                &[],
            ],
        );
        assert!(decompress_u32(&frame).is_err());
    }

    #[test]
    fn rejects_literal_length_mismatch_in_frame() {
        let frame = raw_field_lz_frame(1, &[&[0xaa], &[], &[], &[], &[]]);
        assert!(decompress_u32(&frame).is_err());
    }

    #[test]
    fn rejects_offset_underflow_in_frame() {
        let token = 0x0003u16.to_le_bytes();
        let offset = 1u32.to_le_bytes();
        let frame = raw_field_lz_frame(2, &[&[], &token, &offset, &[], &[]]);
        assert!(decompress_u32(&frame).is_err());
    }

    #[test]
    fn rejects_output_length_mismatch_in_frame() {
        let literal = 123u32.to_le_bytes();
        let frame = raw_field_lz_frame(2, &[&literal, &[], &[], &[], &[]]);
        assert!(decompress_u32(&frame).is_err());
    }

    #[test]
    fn rejects_truncated_frame() {
        let mut frame = compress_u32(&[1, 2, 1, 2]).unwrap();
        frame.pop();
        assert!(decompress_u32(&frame).is_err());
    }

    #[test]
    fn rejects_zstd_output_width_mismatch_for_field_lz_input() {
        let literal_bytes = field_lz::u32s_to_le_bytes(&[1, 2, 3]);
        let compressed_literals =
            zstd_codec::encode_magicless(&literal_bytes, DEFAULT_COMPRESSION_LEVEL)
                .expect("zstd fixture compression");

        let mut frame = Vec::new();
        frame.extend_from_slice(MAGIC);
        frame.push(VERSION_V1);
        frame.push(KIND_U32_FIELD_LZ);
        frame.push(OPENZL_TYPE_NUMERIC);
        varint::write_usize(U32_WIDTH, &mut frame);
        varint::write_usize(3, &mut frame);
        varint::write_usize(1, &mut frame);

        varint::write_usize(3, &mut frame); // chunk_num_elements
        varint::write_usize(7, &mut frame); // stream slots: zstd payload, zstd output, 4 raw sides, final
        varint::write_usize(5, &mut frame); // stored streams
        varint::write_usize(2, &mut frame); // zstd + FieldLZ transforms
        varint::write_usize(6, &mut frame); // final stream

        varint::write_usize(0, &mut frame);
        varint::write_usize(compressed_literals.len(), &mut frame);
        frame.extend_from_slice(&compressed_literals);
        for stream_id in 2..=5 {
            varint::write_usize(stream_id, &mut frame);
            varint::write_usize(0, &mut frame);
        }

        varint::write_u64(STANDARD_TRANSFORM_ID_ZSTD, &mut frame);
        varint::write_usize(1, &mut frame);
        varint::write_usize(0, &mut frame);
        varint::write_usize(1, &mut frame);
        varint::write_usize(1, &mut frame);
        let wrong_width = varint::encode_u64(3);
        varint::write_usize(wrong_width.len(), &mut frame);
        frame.extend_from_slice(&wrong_width);

        varint::write_u64(STANDARD_TRANSFORM_ID_FIELD_LZ, &mut frame);
        varint::write_usize(5, &mut frame);
        for stream_id in 1..=5 {
            varint::write_usize(stream_id, &mut frame);
        }
        varint::write_usize(1, &mut frame);
        varint::write_usize(6, &mut frame);
        let private = varint::encode_u64(3);
        varint::write_usize(private.len(), &mut frame);
        frame.extend_from_slice(&private);

        assert!(decompress_u32(&frame).is_err());
    }

    fn raw_field_lz_frame(chunk_num_elements: usize, side_streams: &[&[u8]; 5]) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(MAGIC);
        frame.push(VERSION_V1);
        frame.push(KIND_U32_FIELD_LZ);
        frame.push(OPENZL_TYPE_NUMERIC);
        varint::write_usize(U32_WIDTH, &mut frame);
        varint::write_usize(chunk_num_elements, &mut frame);
        varint::write_usize(1, &mut frame);

        varint::write_usize(chunk_num_elements, &mut frame);
        varint::write_usize(6, &mut frame);
        varint::write_usize(5, &mut frame);
        varint::write_usize(1, &mut frame);
        varint::write_usize(5, &mut frame);

        for (stream_id, payload) in side_streams.iter().enumerate() {
            varint::write_usize(stream_id, &mut frame);
            varint::write_usize(payload.len(), &mut frame);
            frame.extend_from_slice(payload);
        }

        varint::write_u64(STANDARD_TRANSFORM_ID_FIELD_LZ, &mut frame);
        varint::write_usize(5, &mut frame);
        for stream_id in 0..5 {
            varint::write_usize(stream_id, &mut frame);
        }
        varint::write_usize(1, &mut frame);
        varint::write_usize(5, &mut frame);
        let private = varint::encode_u64(chunk_num_elements as u64);
        varint::write_usize(private.len(), &mut frame);
        frame.extend_from_slice(&private);
        frame
    }
}
