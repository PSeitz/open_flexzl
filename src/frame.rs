//! Native OFZL frame encoder/decoder.
//!
//! A frame is split into independent chunks. Each chunk contains a tiny
//! decode graph: stored byte streams plus transform records in execution
//! order. The current encoder emits a deliberately small graph
//! (`store-or-zstd side streams -> FieldLZ -> optional delta_int`), while the
//! decoder validates the generic graph structure so more OpenZL-style routes
//! can be added later.

use std::collections::HashSet;

use crate::constants::*;
use crate::delta;
use crate::field_lz::{self, le_bytes_to_u32s, FieldLzSideStreams};
use crate::varint::{self, Reader};
use crate::{transpose, zstd_codec, Error};

// FieldLZ consumes five side streams in this exact order:
// literals, tokens, explicit offsets, extra literal lengths, extra match
// lengths. The element width is carried through zstd headers and checked before
// the FieldLZ transform runs.
const FIELD_LZ_INPUT_WIDTHS: [usize; FIELD_LZ_INPUT_COUNT] =
    [U32_WIDTH, U16_WIDTH, U32_WIDTH, U32_WIDTH, U32_WIDTH];

const LITERAL_ONLY_MIN_ELEMENTS: usize = 1024;
const LITERAL_ONLY_MIN_EQUAL_VALUE_RATIO: Ratio = Ratio::new(1, 2);
const LITERAL_ONLY_MAX_EQUAL_VALUE_RATIO: Ratio = Ratio::new(9, 10);
const LITERAL_ONLY_MAX_EQUAL_DELTA_RATIO: Ratio = Ratio::new(1, 2);

pub(crate) fn compress_u32(input: &[u32]) -> Result<Vec<u8>, Error> {
    let chunk_count = input.len().div_ceil(MAX_CHUNK_ELEMENTS_U32);

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION_V1);
    out.push(KIND_U32_FIELD_LZ);
    out.push(OPENZL_TYPE_NUMERIC);
    varint::write_usize(U32_WIDTH, &mut out);
    varint::write_usize(input.len(), &mut out);
    varint::write_usize(chunk_count, &mut out);

    for chunk in input.chunks(MAX_CHUNK_ELEMENTS_U32) {
        debug_assert!(!chunk.is_empty());
        debug_assert!(chunk.len() <= MAX_CHUNK_ELEMENTS_U32);
        choose_and_write_best_chunk_encoding(chunk, &mut out)?;
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
        let chunk = decode_chunk_encoding(&mut reader)?;
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

fn choose_and_write_best_chunk_encoding(chunk: &[u32], out: &mut Vec<u8>) -> Result<(), Error> {
    debug_assert!(!chunk.is_empty());
    debug_assert!(chunk.len() <= MAX_CHUNK_ELEMENTS_U32);

    let analysis = ChunkValueAnalysis::scan_u32(chunk);
    // TODO: Replace these hand-tuned route thresholds with generic candidate
    // evaluation (or, longer-term, a Rust-native selector model) as tracked in
    // `plan.md`. Avoid accumulating dataset-specific branches here.
    if analysis.element_count >= LITERAL_ONLY_MIN_ELEMENTS
        && LITERAL_ONLY_MIN_EQUAL_VALUE_RATIO
            .is_at_most(analysis.equal_value_pairs, analysis.element_count)
        && LITERAL_ONLY_MAX_EQUAL_VALUE_RATIO
            .is_above(analysis.equal_value_pairs, analysis.element_count)
        && LITERAL_ONLY_MAX_EQUAL_DELTA_RATIO
            .is_above(analysis.equal_delta_pairs, analysis.element_count)
    {
        out.extend_from_slice(&serialize_literal_only_chunk_encoding(chunk)?);
        return Ok(());
    }

    // From here on, route selection must choose exactly one full encoding.
    // Sampling is allowed, but we do not build both raw and delta full-chunk
    // candidates just to compare them.
    let chunk_bytes = if DELTA_INT_FIELD_LZ_STRATEGY.should_build(chunk, &analysis)? {
        DELTA_INT_FIELD_LZ_STRATEGY.encode_chunk(chunk)?
    } else {
        serialize_chunk_encoding_candidate(chunk.len(), chunk, false)?
    };
    out.extend_from_slice(&chunk_bytes);
    Ok(())
}

fn serialize_chunk_encoding_candidate(
    chunk_num_elements: usize,
    field_lz_input_values: &[u32],
    append_delta_transform: bool,
) -> Result<Vec<u8>, Error> {
    let streams = field_lz::encode_u32_to_side_streams(field_lz_input_values)?;
    serialize_field_lz_side_streams(chunk_num_elements, &streams, append_delta_transform)
}

fn serialize_literal_only_chunk_encoding(chunk: &[u32]) -> Result<Vec<u8>, Error> {
    let streams = FieldLzSideStreams {
        literals: field_lz::u32s_to_le_bytes(chunk),
        ..FieldLzSideStreams::default()
    };
    serialize_field_lz_side_streams(chunk.len(), &streams, false)
}

fn serialize_field_lz_side_streams(
    chunk_num_elements: usize,
    streams: &FieldLzSideStreams,
    append_delta_transform: bool,
) -> Result<Vec<u8>, Error> {
    let plan =
        build_field_lz_side_stream_graph(streams, chunk_num_elements, append_delta_transform)?;

    let mut r = Vec::new();
    varint::write_usize(chunk_num_elements, &mut r);
    varint::write_usize(plan.stream_slot_count, &mut r);
    varint::write_usize(plan.stored_streams.len(), &mut r);
    varint::write_usize(plan.transforms.len(), &mut r);
    varint::write_usize(plan.final_stream_id, &mut r);

    for s in plan.stored_streams {
        varint::write_usize(s.stream_id, &mut r);
        varint::write_usize(s.payload.len(), &mut r);
        r.extend_from_slice(&s.payload);
    }
    for t in plan.transforms {
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

fn decode_chunk_encoding(reader: &mut Reader<'_>) -> Result<Vec<u32>, Error> {
    // The map is encoded in dependency order: stored streams are available
    // first, then every transform may only reference streams that have already
    // been defined. This lets us execute while reading and still validate that
    // every declared stream slot is defined exactly once.
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

    let mut slots = vec![StreamSlot::default(); stream_slot_count];

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

    let mut transform_log: Vec<TransformLogEntry> = Vec::with_capacity(transform_count);
    for _ in 0..transform_count {
        read_and_execute_transform_record(
            reader,
            chunk_num_elements,
            &mut slots,
            &mut transform_log,
        )?;
    }

    if slots[final_stream_id].bytes.is_none() {
        return Err(Error::InvalidMap("final stream is undefined"));
    }
    // For a `compress_u32()` frame, the only legal final producers are:
    //   FieldLZ                         (raw path)
    //   FieldLZ -> delta_int            (delta path)
    // Side-stream zstd transforms are internal inputs to FieldLZ and must not
    // become the final chunk stream.
    let mut field_lz_outputs = transform_log
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
    let final_producer = transform_log.iter().find(|t| t.output == final_stream_id);
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
    debug_assert!(chunk_num_elements <= MAX_CHUNK_ELEMENTS_U32);
    let expected_bytes = chunk_num_elements * U32_WIDTH;
    if final_bytes.len() != expected_bytes {
        return Err(Error::InvalidMap(
            "final stream byte length does not match chunk element count",
        ));
    }
    le_bytes_to_u32s(&final_bytes)
}

fn read_and_execute_transform_record(
    reader: &mut Reader<'_>,
    chunk_num_elements: usize,
    slots: &mut [StreamSlot],
    transform_log: &mut Vec<TransformLogEntry>,
) -> Result<(), Error> {
    let transform_id = reader.read_u64("transform_id")?;
    let input_count = reader.read_usize("input_count")?;
    if input_count > RUNTIME_TRANSFORM_INPUT_LIMIT {
        return Err(Error::LimitExceeded("transform input count"));
    }
    let mut inputs = Vec::with_capacity(input_count);
    for _ in 0..input_count {
        let stream_id = reader.read_usize("input stream id")?;
        validate_transform_input_stream_id(stream_id, slots)?;
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

    // The wire format allows multi-output transforms because OpenZL uses them
    // for routes such as transpose-split and quantize. The transforms supported
    // today each produce exactly one output, represented by `TransformOutput`.
    let output = match transform_id {
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
        STANDARD_TRANSFORM_ID_TRANSPOSE_SPLIT4 => {
            if !private_header.is_empty() {
                return Err(Error::InvalidTransform(
                    "transpose_split4 transform must have an empty private header",
                ));
            }
            execute_decode_transpose_split4_transform(&inputs, &outputs, chunk_num_elements, slots)?
        }
        STANDARD_TRANSFORM_ID_DELTA_INT => {
            execute_delta_int_transform(&inputs, &outputs, &private_header, slots)?
        }
        other => return Err(Error::UnsupportedTransform(other)),
    };

    transform_log.push(TransformLogEntry {
        id: transform_id,
        input: inputs.first().copied(),
        output: output.stream_id,
    });
    slots[output.stream_id].element_width = output.element_width;
    slots[output.stream_id].bytes = Some(output.bytes);
    Ok(())
}

fn execute_zstd_transform(
    inputs: &[usize],
    outputs: &[usize],
    private_header: &[u8],
    slots: &[StreamSlot],
) -> Result<TransformOutput, Error> {
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
    let bytes = zstd_codec::decode_magicless(input, output_elt_width)?;
    Ok(TransformOutput {
        stream_id: outputs[0],
        bytes,
        element_width: Some(output_elt_width),
    })
}

fn execute_decode_transpose_split4_transform(
    inputs: &[usize],
    outputs: &[usize],
    chunk_num_elements: usize,
    slots: &[StreamSlot],
) -> Result<TransformOutput, Error> {
    if inputs.len() != U32_WIDTH || outputs.len() != 1 {
        return Err(Error::InvalidTransform(
            "transpose_split4 transform must have four inputs and one output",
        ));
    }

    let mut lanes = [&[][..]; U32_WIDTH];
    for (lane, &stream_id) in lanes.iter_mut().zip(inputs) {
        if slots[stream_id].element_width.is_some_and(|w| w != 1) {
            return Err(Error::InvalidTransform(
                "transpose_split4 input element width must be 1",
            ));
        }
        *lane = slots[stream_id].bytes.as_deref().ok_or(Error::InvalidMap(
            "transpose_split4 input stream is undefined",
        ))?;
    }

    Ok(TransformOutput {
        stream_id: outputs[0],
        bytes: transpose::decode_split4(lanes, chunk_num_elements)?,
        element_width: Some(U32_WIDTH),
    })
}

fn execute_delta_int_transform(
    inputs: &[usize],
    outputs: &[usize],
    private_header: &[u8],
    slots: &[StreamSlot],
) -> Result<TransformOutput, Error> {
    if inputs.len() != 1 || outputs.len() != 1 || !private_header.is_empty() {
        return Err(Error::InvalidTransform(
            "delta_int transform must have one input, one output, and an empty private header",
        ));
    }
    if slots[inputs[0]]
        .element_width
        .is_some_and(|w| w != U32_WIDTH)
    {
        return Err(Error::InvalidTransform(
            "delta_int input element width must be 4 for u32",
        ));
    }
    let input = slots[inputs[0]]
        .bytes
        .as_deref()
        .ok_or(Error::InvalidMap("delta_int input stream is undefined"))?;
    Ok(TransformOutput {
        stream_id: outputs[0],
        bytes: delta::decode_u32_delta_bytes(input)?,
        element_width: Some(U32_WIDTH),
    })
}

fn execute_field_lz_transform(
    inputs: &[usize],
    outputs: &[usize],
    private_header: &[u8],
    chunk_num_elements: usize,
    slots: &[StreamSlot],
) -> Result<TransformOutput, Error> {
    if inputs.len() != FIELD_LZ_INPUT_COUNT || outputs.len() != 1 {
        return Err(Error::InvalidTransform(
            "FieldLZ transform must have five inputs and one output",
        ));
    }
    let declared_elements = varint::read_single_usize(private_header, "FieldLZ chunk length")?;
    if declared_elements != chunk_num_elements {
        return Err(Error::InvalidTransform(
            "FieldLZ private chunk length does not match chunk encoding",
        ));
    }

    for (&stream_id, expected_width) in inputs.iter().zip(FIELD_LZ_INPUT_WIDTHS) {
        if let Some(actual_width) = slots[stream_id].element_width {
            if actual_width != expected_width {
                return Err(Error::InvalidTransform(
                    "FieldLZ input element width does not match its stream role",
                ));
            }
        }
    }

    let mut input_refs = [&[][..]; FIELD_LZ_INPUT_COUNT];
    for (input_ref, &stream_id) in input_refs.iter_mut().zip(inputs) {
        *input_ref = slots[stream_id]
            .bytes
            .as_deref()
            .ok_or(Error::InvalidMap("FieldLZ input stream is undefined"))?;
    }
    let bytes = field_lz::decode_side_streams(input_refs, chunk_num_elements)?;
    Ok(TransformOutput {
        stream_id: outputs[0],
        bytes,
        element_width: Some(U32_WIDTH),
    })
}

fn validate_transform_input_stream_id(stream_id: usize, slots: &[StreamSlot]) -> Result<(), Error> {
    if stream_id >= slots.len() {
        return Err(Error::InvalidMap("transform input id is out of range"));
    }
    if slots[stream_id].bytes.is_none() {
        return Err(Error::InvalidMap("transform input stream is undefined"));
    }
    Ok(())
}

fn build_field_lz_side_stream_graph(
    streams: &FieldLzSideStreams,
    chunk_num_elements: usize,
    append_delta_transform: bool,
) -> Result<ChunkGraphPlan, Error> {
    // Every FieldLZ side stream is either stored directly if tiny, or stored as
    // a zstd payload followed by a zstd decode step. Literal streams may take a
    // richer route first: split u32 bytes into four byte lanes, code each lane
    // independently, then transpose the lanes back before FieldLZ consumes the
    // literal stream. Stats gate that candidate so random/wide literals don't
    // pay for four zstd encodes just to lose to the raw route.
    let side_streams = streams.as_decode_inputs();

    let mut stored_streams = Vec::with_capacity(FIELD_LZ_INPUT_COUNT + U32_WIDTH);
    let mut transforms = Vec::with_capacity(FIELD_LZ_INPUT_COUNT + U32_WIDTH + 3);
    let mut field_lz_inputs = Vec::with_capacity(FIELD_LZ_INPUT_COUNT);
    let mut next_stream_id = 0usize;

    let literal_route = encode_literal_side_stream_route(side_streams[0], next_stream_id)?;
    next_stream_id = literal_route.next_stream_id;
    field_lz_inputs.push(literal_route.output_stream_id);
    append_side_stream_route(literal_route, &mut stored_streams, &mut transforms);

    for (bytes, width) in side_streams[1..]
        .iter()
        .zip(FIELD_LZ_INPUT_WIDTHS[1..].iter())
    {
        let route = encode_raw_side_stream_route(bytes, *width, next_stream_id)?;
        next_stream_id = route.next_stream_id;
        field_lz_inputs.push(route.output_stream_id);
        append_side_stream_route(route, &mut stored_streams, &mut transforms);
    }

    let field_lz_output_id = next_stream_id;
    next_stream_id += 1;
    transforms.push(TransformRecord {
        transform_id: STANDARD_TRANSFORM_ID_FIELD_LZ,
        inputs: field_lz_inputs,
        outputs: vec![field_lz_output_id],
        private_header: varint::encode_u64(chunk_num_elements as u64),
    });

    // If the candidate was parsed from deltas, FieldLZ reconstructs the delta
    // stream first. The final stream is produced by a following delta_int
    // transform that converts deltas back to original values.
    let final_stream_id = if append_delta_transform {
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

    Ok(ChunkGraphPlan {
        stored_streams,
        transforms,
        final_stream_id,
        stream_slot_count: next_stream_id,
    })
}

fn encode_literal_side_stream_route(
    bytes: &[u8],
    next_stream_id: usize,
) -> Result<SideStreamRoute, Error> {
    if transpose::should_try_literal_split4(bytes) {
        return build_transposed_literal_side_stream_candidate(bytes, next_stream_id);
    }
    encode_raw_side_stream_route(bytes, U32_WIDTH, next_stream_id)
}

fn encode_raw_side_stream_route(
    bytes: &[u8],
    element_width: usize,
    next_stream_id: usize,
) -> Result<SideStreamRoute, Error> {
    if bytes.len() < DEFAULT_MIN_STREAM_SIZE {
        let stream_id = next_stream_id;
        return Ok(SideStreamRoute {
            stored_streams: vec![StoredStreamRecord {
                stream_id,
                payload: bytes.to_vec(),
            }],
            transforms: Vec::new(),
            output_stream_id: stream_id,
            next_stream_id: next_stream_id + 1,
        });
    }

    let stored_id = next_stream_id;
    let decoded_id = next_stream_id + 1;
    let payload = zstd_codec::encode_magicless(bytes, DEFAULT_COMPRESSION_LEVEL)?;
    Ok(SideStreamRoute {
        stored_streams: vec![StoredStreamRecord {
            stream_id: stored_id,
            payload,
        }],
        transforms: vec![TransformRecord {
            transform_id: STANDARD_TRANSFORM_ID_ZSTD,
            inputs: vec![stored_id],
            outputs: vec![decoded_id],
            private_header: varint::encode_u64(element_width as u64),
        }],
        output_stream_id: decoded_id,
        next_stream_id: next_stream_id + 2,
    })
}

fn build_transposed_literal_side_stream_candidate(
    bytes: &[u8],
    mut next_stream_id: usize,
) -> Result<SideStreamRoute, Error> {
    debug_assert!(bytes.len().is_multiple_of(U32_WIDTH));
    let lanes = transpose::encode_split4(bytes);
    let mut stored_streams = Vec::with_capacity(U32_WIDTH);
    let mut transforms = Vec::with_capacity(U32_WIDTH + 1);
    let mut lane_stream_ids = Vec::with_capacity(U32_WIDTH);

    for lane in &lanes {
        let route = encode_raw_side_stream_route(lane, 1, next_stream_id)?;
        next_stream_id = route.next_stream_id;
        lane_stream_ids.push(route.output_stream_id);
        append_side_stream_route(route, &mut stored_streams, &mut transforms);
    }

    let output_stream_id = next_stream_id;
    next_stream_id += 1;
    // Mandatory for this candidate: FieldLZ consumes one width-4 literal stream,
    // so the selected byte lanes must be recombined before the FieldLZ step.
    transforms.push(TransformRecord {
        transform_id: STANDARD_TRANSFORM_ID_TRANSPOSE_SPLIT4,
        inputs: lane_stream_ids,
        outputs: vec![output_stream_id],
        private_header: Vec::new(),
    });

    Ok(SideStreamRoute {
        stored_streams,
        transforms,
        output_stream_id,
        next_stream_id,
    })
}

fn append_side_stream_route(
    route: SideStreamRoute,
    stored_streams: &mut Vec<StoredStreamRecord>,
    transforms: &mut Vec<TransformRecord>,
) {
    stored_streams.extend(route.stored_streams);
    transforms.extend(route.transforms);
}

static DELTA_INT_FIELD_LZ_STRATEGY: DeltaIntFieldLzStrategy = DeltaIntFieldLzStrategy;

trait ChunkEncodingStrategy {
    fn should_build(&self, chunk: &[u32], analysis: &ChunkValueAnalysis) -> Result<bool, Error>;
    fn encode_chunk(&self, chunk: &[u32]) -> Result<Vec<u8>, Error>;
}

struct Ratio {
    numerator: usize,
    denominator: usize,
}

impl Ratio {
    const fn new(numerator: usize, denominator: usize) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    fn is_at_most(&self, count: usize, total: usize) -> bool {
        count * self.denominator >= total * self.numerator
    }

    fn is_above(&self, count: usize, total: usize) -> bool {
        count * self.denominator < total * self.numerator
    }
}

fn should_build_delta_from_samples(chunk: &[u32]) -> Result<bool, Error> {
    if chunk.len() < DELTA_SAMPLE_MIN_ELEMENTS {
        return Ok(true);
    }

    let mut sampled_raw_len = 0usize;
    let mut sampled_delta_len = 0usize;
    for (start, end) in delta_sample_ranges(chunk.len()) {
        let sample = &chunk[start..end];
        let raw_bytes = field_lz::u32s_to_le_bytes(sample);
        sampled_raw_len +=
            zstd_codec::encode_magicless(&raw_bytes, DEFAULT_COMPRESSION_LEVEL)?.len();

        let deltas = delta::encode_u32_deltas(sample);
        let delta_bytes = field_lz::u32s_to_le_bytes(&deltas);
        sampled_delta_len +=
            zstd_codec::encode_magicless(&delta_bytes, DEFAULT_COMPRESSION_LEVEL)?.len();
    }

    Ok(sampled_delta_len < sampled_raw_len)
}

fn delta_sample_ranges(element_count: usize) -> impl Iterator<Item = (usize, usize)> {
    let sample_len = DELTA_SAMPLE_LEN.min(element_count);
    let sample_count = DELTA_SAMPLE_COUNT.min(element_count.div_ceil(sample_len));
    (0..sample_count).map(move |index| {
        let start = if sample_count == 1 {
            (element_count - sample_len) / 2
        } else {
            index * (element_count - sample_len) / (sample_count - 1)
        };
        (start, start + sample_len)
    })
}

struct DeltaIntFieldLzStrategy;

const DELTA_SAMPLE_MIN_ELEMENTS: usize = 16 * 1024;
const DELTA_SAMPLE_LEN: usize = 4 * 1024;
const DELTA_SAMPLE_COUNT: usize = 1;

impl ChunkEncodingStrategy for DeltaIntFieldLzStrategy {
    fn should_build(&self, chunk: &[u32], analysis: &ChunkValueAnalysis) -> Result<bool, Error> {
        if analysis.element_count < 2 {
            return Ok(false);
        }

        // Long runs of identical values are already exactly what the raw FieldLZ
        // path handles well; delta mostly turns them into zero runs with extra
        // graph overhead.
        if analysis.equal_value_pairs * 4 >= analysis.element_count {
            return Ok(false);
        }

        // Constant or mostly-constant strides are the strongest signal for
        // delta_int; build the full candidate without spending extra sampling
        // work to confirm it.
        if analysis.equal_delta_pairs * 2 >= analysis.element_count {
            return Ok(true);
        }

        // Otherwise, compact deltas are only a size signal. Before paying for a
        // complete second FieldLZ + zstd candidate, sample a few sub-slices with
        // raw zstd bytes and build the full delta candidate only when sampled
        // delta bytes are smaller than sampled raw bytes.
        let min_compact_gain = (analysis.element_count / 4).max(1);
        let has_compact_delta_signal = analysis.compact_delta_16 * 2 >= analysis.element_count
            && analysis.compact_delta_16 >= analysis.compact_value_16 + min_compact_gain;
        if !has_compact_delta_signal {
            return Ok(false);
        }

        should_build_delta_from_samples(chunk)
    }

    fn encode_chunk(&self, chunk: &[u32]) -> Result<Vec<u8>, Error> {
        let deltas = delta::encode_u32_deltas(chunk);
        serialize_chunk_encoding_candidate(chunk.len(), &deltas, true)
    }
}

/// Cheap one-pass statistics used by encoder strategies to avoid building
/// expensive candidates that are unlikely to win.
#[derive(Default)]
struct ChunkValueAnalysis {
    /// Number of `u32` values in the source chunk.
    element_count: usize,
    /// Adjacent pairs where `value[i] == value[i - 1]`; strong signal that raw
    /// FieldLZ already handles the chunk well.
    equal_value_pairs: usize,
    /// Adjacent pairs where the wrapping delta is unchanged; strong signal for
    /// the `delta_int -> FieldLZ` strategy.
    equal_delta_pairs: usize,
    /// Source values whose high two bytes are either `00 00` or `ff ff`.
    compact_value_16: usize,
    /// Wrapping deltas whose high two bytes are either `00 00` or `ff ff`.
    compact_delta_16: usize,
}

impl ChunkValueAnalysis {
    fn scan_u32(chunk: &[u32]) -> Self {
        let mut analysis = Self {
            element_count: chunk.len(),
            ..Self::default()
        };
        let mut previous_value = 0u32;
        let mut previous_delta = None;

        for (index, &value) in chunk.iter().enumerate() {
            if compressible_high_16_bits(value) {
                analysis.compact_value_16 += 1;
            }
            if index != 0 && value == previous_value {
                analysis.equal_value_pairs += 1;
            }

            let delta = value.wrapping_sub(previous_value);
            if compressible_high_16_bits(delta) {
                analysis.compact_delta_16 += 1;
            }
            if previous_delta == Some(delta) {
                analysis.equal_delta_pairs += 1;
            }

            previous_value = value;
            previous_delta = Some(delta);
        }

        analysis
    }
}

fn compressible_high_16_bits(value: u32) -> bool {
    // Values whose high two bytes are all 0x00 or all 0xff have highly
    // compressible high bytes in the little-endian literal stream.
    value <= 0x0000_ffff || value >= 0xffff_0000
}

struct ChunkGraphPlan {
    stored_streams: Vec<StoredStreamRecord>,
    transforms: Vec<TransformRecord>,
    final_stream_id: usize,
    stream_slot_count: usize,
}

struct SideStreamRoute {
    stored_streams: Vec<StoredStreamRecord>,
    transforms: Vec<TransformRecord>,
    output_stream_id: usize,
    next_stream_id: usize,
}

// A payload that appears directly in the serialized chunk encoding. If this
// stream feeds a zstd transform, the payload is magicless zstd bytes. If it
// feeds FieldLZ directly, the payload is already the raw side-stream bytes.
struct StoredStreamRecord {
    stream_id: usize,
    payload: Vec<u8>,
}

// Serialized transform step. Records are written and read in decode order.
struct TransformRecord {
    transform_id: u64,
    inputs: Vec<usize>,
    outputs: Vec<usize>,
    private_header: Vec<u8>,
}

#[derive(Clone, Default)]
struct StreamSlot {
    bytes: Option<Vec<u8>>,
    used: bool,
    element_width: Option<usize>,
}

struct TransformOutput {
    stream_id: usize,
    bytes: Vec<u8>,
    element_width: Option<usize>,
}

// Minimal execution log used for final-map validation. It is intentionally not
// a full graph IR; it only answers "who produced the final stream?".
struct TransformLogEntry {
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
    fn delta_sample_ranges_are_bounded_and_spread() {
        let element_count = 200_000;
        let ranges: Vec<_> = delta_sample_ranges(element_count).collect();
        assert_eq!(ranges.len(), DELTA_SAMPLE_COUNT);
        assert_eq!(
            ranges[0],
            (
                (element_count - DELTA_SAMPLE_LEN) / 2,
                (element_count + DELTA_SAMPLE_LEN) / 2
            )
        );
        assert!(ranges
            .iter()
            .all(|&(start, end)| start < end && end <= element_count));
    }

    #[test]
    fn delta_sampling_keeps_strong_constant_stride_signal() {
        let input: Vec<u32> = (0..20_000u32).map(|i| 1_200_000 + i * 37).collect();
        let analysis = ChunkValueAnalysis::scan_u32(&input);

        assert!(DELTA_INT_FIELD_LZ_STRATEGY
            .should_build(&input, &analysis)
            .unwrap());
    }

    #[test]
    fn transposed_literal_route_round_trips_when_selected() {
        let input: Vec<u32> = (0..4096).map(|i| ((i * 37) & 0xffff) as u32).collect();
        let streams = FieldLzSideStreams {
            literals: field_lz::u32s_to_le_bytes(&input),
            ..FieldLzSideStreams::default()
        };
        let plan = build_field_lz_side_stream_graph(&streams, input.len(), false).unwrap();
        assert!(plan
            .transforms
            .iter()
            .any(|t| t.transform_id == STANDARD_TRANSFORM_ID_TRANSPOSE_SPLIT4));

        let mut chunk = Vec::new();
        varint::write_usize(input.len(), &mut chunk);
        varint::write_usize(plan.stream_slot_count, &mut chunk);
        varint::write_usize(plan.stored_streams.len(), &mut chunk);
        varint::write_usize(plan.transforms.len(), &mut chunk);
        varint::write_usize(plan.final_stream_id, &mut chunk);
        for stored in plan.stored_streams {
            varint::write_usize(stored.stream_id, &mut chunk);
            varint::write_usize(stored.payload.len(), &mut chunk);
            chunk.extend_from_slice(&stored.payload);
        }
        for transform in plan.transforms {
            varint::write_u64(transform.transform_id, &mut chunk);
            varint::write_usize(transform.inputs.len(), &mut chunk);
            for input_id in transform.inputs {
                varint::write_usize(input_id, &mut chunk);
            }
            varint::write_usize(transform.outputs.len(), &mut chunk);
            for output_id in transform.outputs {
                varint::write_usize(output_id, &mut chunk);
            }
            varint::write_usize(transform.private_header.len(), &mut chunk);
            chunk.extend_from_slice(&transform.private_header);
        }

        let decoded = decode_chunk_encoding(&mut Reader::new(&chunk)).unwrap();
        assert_eq!(decoded, input);
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
