# open_flexzl plan

Status: v1 checklist approved; initial implementation landed.

## Current handoff summary

A new session should be able to resume from this document alone.

Important current consensus:

- The implementation approval checklist near the end is now approved.
- Existing scratch/prototype code under `open_flexzl/src/` was discarded; the crate now has a fresh v1 implementation based on this plan. Do not recover the old prototype.
- The target remains a Rust-native `u32` FieldLZ compressor, not OpenZL frame compatibility.
- We approved using the same broad semantics as OpenZL with a simpler native chunk-local decoding-map encoding.
- The frame should use OpenZL-like stream type values and standard transform IDs where they line up, e.g. `22 = zstd` and `24 = field_lz`.
- `field_lz = 24` is the transform that consumes the five FieldLZ side streams and regenerates the typed chunk. The `u32` meaning comes from `KIND_U32_FIELD_LZ` plus final output metadata.
- Final output metadata should be stored: type `OPENZL_TYPE_NUMERIC`, element width `4`, total element count.
- A codec/transform interface is central. Side-stream routing should be represented as transform chains in the chunk map.
- The implementation now supports zstd (`22`), FieldLZ (`24`), delta_int (`1`), transpose_split4 (`31`), and direct raw stored streams for tiny side streams.
- This zstd/bootstrap route is not a frame-format limitation; reference side-stream routing is tracked for later parity, but FSE/Huffman/bitpack are not needed for the first version because zstd side streams are good enough initially.
- Target zstd behavior is modern OpenZL magicless zstd frames with content size present. Rust support was verified with `zstd = { version = "0.13", features = ["experimental"] }`; the implementation uses high-level magicless encode/decode settings plus `ZSTD_getFrameHeader_advanced(..., ZSTD_f_zstd1_magicless)` for content-size validation.
- Direct small-stream store is included in milestone 1, separately from store-on-expansion. OpenZL’s small-stream threshold is strict `< 10` bytes.
- Default side-stream zstd level is deliberately `2` for the public no-options encoder. The benchmark's external `zstd_on_raw` column still uses level `6`, so its ratio/speed is a comparison point rather than the OFZL internal setting.
- Quantize is tracked as a post-v1 reference-parity candidate if raw zstd side streams prove to be a ratio bottleneck: it is reversible `value -> code + raw extra bits`, not dictionary coding.
- Literal dictionary/categorical coding has landed as native transform `1001` for low-cardinality literal streams. It is separate from quantize: encode a table of unique `u32` values plus compact `u8` codes, then reconstruct the normal little-endian literal stream before FieldLZ consumes it. The decoder accepts `u8` and `u16` code widths; the first encoder route targets the strong `<= 256` unique-value case.

Suggested next-session order:

1. Re-read this plan and the implementation approval checklist.
2. Phase 3 hardening landed: map-validation negative tests (`tests/map_validation.rs`) and property tests for arbitrary/structured inputs (`tests/properties.rs`). Binary golden fixtures are deliberately deferred — the wire format is still in flux (delta_int just landed, more transforms expected) and pinning byte-exact frames now would be churn for every feature.
3. Phase 4 `binggan` benchmarks landed (`benches/compression.rs`) with three comparison columns: `open_flexzl`, `zstd_on_raw` (raw `u32` LE bytes through zstd at level 6), and `openzl` (via the `rust-openzl 0.1` crate, which calls into the upstream OpenZL C library). Datasets: checked-in curated real-world files from `test_data` plus `all_real_world_concatenated` (loaded via `tests/common/datasets.rs`; resolution: `OFZL_TEST_DATA_DIR` env var → `$CARGO_MANIFEST_DIR/test_data`; missing files are skipped). The same loader powers `tests/real_world.rs`, which round-trips every available dataset including the multi-chunk `ten_value_cycle` (~16.5 MiB → 2 chunks). The benchmark ratio delta display is percentage-point difference, not relative percent change.
4. **OpenZL ratio gap has mainly been transform/route work, not parser work.** Earlier dev-machine results before `delta_int`, `transpose_split4`, and `literal_dict_u32` exposed large gaps (for example, monotonic data compressed like raw zstd until the delta route landed). Keep old numbers as historical context only; rerun `benches/compression.rs` after route changes before making priority decisions. On encode speed, OFZL has often been faster than OpenZL, so ratio work should stay focused on structure-exposing transforms and cheap explicit route gates rather than expensive parser or best-of-N selection changes.
5. Stage-2 cleanup landed: hash-table insertion is now sparse (start+1, periodic mid-points, end-1) instead of every position, mirroring OpenZL's fast parser. Net effect on the listed datasets: +52% encode on `repeated_blocks` (6.9 → 10.5 GB/s), +26% on `low_cardinality`, +16% on `single_symbol_floods`, with a ~1% ratio cost on `ten_value_cycle` (135597 → 137027 bytes) where some phase-shift matches go unfound.
6. Repeated-offset emission was prototyped (rep[0]-take-immediately) and reverted: the trade was poor (5–32% encode regression for 1–15% ratio wins on a few datasets) compared with the larger benchmark-driven opportunities in transform/route work.
7. Stage 3 landed: the OpenZL `delta_int` transform (Standard Transform ID `1`, `value -> first_value + cumulative deltas`) sits *after* FieldLZ in the decode chain — FieldLZ regenerates a delta stream and `delta_int` undoes the delta with a prefix-sum. Decoder validation accepts the final chunk stream produced either by FieldLZ directly (raw path) or by a `delta_int` whose input is the FieldLZ output (delta path); either way exactly one FieldLZ transform per chunk. The encoder no longer builds both full raw and delta FieldLZ candidates just to compare them. It chooses one route from cheap analysis: skip delta for strong equal-value runs, force delta for strong constant-stride signals, and otherwise sample one centered 4 Ki-element slice by comparing zstd(raw bytes) vs zstd(delta bytes) before paying for the full delta FieldLZ route.
8. Byte-transposed literal lanes landed as transform ID `31` (`transpose_split4`). The literal route splits little-endian `u32` bytes into four byte lanes, stores/zstd-codes each lane, then recombines lanes before FieldLZ consumes the logical literal stream. The encoder does not build a competing raw-literal route once the split gate passes; it relies on a heuristic gate: minimum 256 bytes, stable sign/zero-extended high 16 bits, small high-byte cardinalities, two nearly constant lanes, plus the latest large bounded wide-tail case (`element_count >= 100_000`, third-byte cardinality `<= 32`, top-byte cardinality `<= 4`).
9. Route-selection decision: do **not** add a best-of-N candidate selector. Avoid dataset-shaped branches, but keep route choice as explicit heuristics plus cheap targeted sampling per route. Improve those heuristics when benchmarks expose misses; do not routinely build multiple complete chunk maps just to pick the smallest.
10. Side-stream route priority clarified: zstd already includes strong entropy coding, so native Huffman/FSE/bitpack should not be assumed to improve ratio enough to justify near-term complexity. The first structure-exposing literal routes have landed (`literal_dict_u32` and `transpose_split4`); prioritize benchmark-driven tuning of their explicit gates and maybe offset/length quantization next. Native entropy codecs remain lower-priority work for reduced per-stream overhead, faster decode, or OpenZL reference parity.
11. Dictionary-case update: native `literal_dict_u32` (`1001`) has landed for low-cardinality literal streams. `hot_head_dictionary` now uses it and improves to ~8.15% output in the benchmark slice (beating both `zstd_on_raw` and OpenZL there). `bursty_mid_dictionary` remains outside the first encoder gate because it has ~2,360 unique values; its remaining gap is more likely literal coding quality plus metadata/offset-length coding.
12. Rust CI landed under `.github/workflows/rust.yml`: build, `cargo fmt --all -- --check`, clippy with `-D warnings`, and tests on stable Rust for pushes/PRs to `main`.

## Draft open-question recommendations

These are recommendations, not final approval decisions. Use them to resolve or revise the unchecked checklist items.

- Wire constants: approve as-is. They are small, versioned, and keep useful OpenZL type/transform ID alignment without committing to OpenZL frame compatibility.
- Varints: approve canonical unsigned LEBU64 as-is. It is simple enough to implement ourselves and strict enough for deterministic fixtures and corruption tests.
- Zstd magicless support: local `zstd`/`zstd-safe` crates expose magicless frame format behind the `experimental` feature (`FrameFormat::Magicless`, `CParameter::Format`, `DParameter::Format`, and high-level `include_magicbytes(false)` helpers). Keep the target as magicless zstd with content size present. Implementation should still include a small spike/test because strict content-size validation for magicless frames may need `zstd_safe::zstd_sys::ZSTD_getFrameHeader_advanced(..., ZSTD_f_zstd1_magicless)` rather than only `zstd_safe::get_frame_content_size()`.
- Direct small-stream store: include this in milestone 1. It is not a new codec; it is just a planner choice where a raw stored stream is wired directly into the consuming transform. It avoids emitting zstd frames for empty/tiny FieldLZ side streams and matches OpenZL's strict `< 10` byte rule early. This means the first useful encoder route is "zstd-or-store per side stream" rather than literally zstd for every side stream.
- Empty input: approve `chunk_count = 0`; non-empty frames use one or more non-empty chunks.
- Decoder strictness: approve. The decoder/map validator is the compatibility boundary, so it should be stricter than the first encoder needs.
- V1 parser: approve as a fast-enough `WIDTH = 4` parser, lz4_flex-inspired and explicit-offset-only at first, with minimum emitted match length 2. Existing scratch code that emits repeated offsets or length-1 matches should be treated as a source of ideas, not the accepted v1 encoder contract.
- Golden fixtures: deferred. The format is still evolving and locking byte-exact frames creates churn on every encoder change. Revisit once the codec set is stable.
- Benchmarks: use `binggan`; do not use Criterion.
- Side-stream entropy codecs: do not include FSE/Huffman/bitpack in the first version. Zstd-compressed side streams are good enough for v1; prioritize a fast match finder before extra side-stream codecs.

## Big-picture implementation roadmap

### Phase 0: freeze the v1 contract

Inputs:

- This plan.
- A small zstd magicless/content-size spike.
- A decision on small-stream store.

Outputs:

- Approved checklist.
- Exact dependency choice, likely `zstd = { version = "0.13", features = ["experimental"] }` plus existing `thiserror`.
- A reset decision: replace the existing `open_flexzl/src/` scratch implementation rather than evolving it in place. Salvage only small, reviewed snippets such as tests or simple helper ideas.

### Phase 1: decode-first foundation

Build the pieces that define what frames are accepted:

- canonical LEBU64 reader/writer
- constants and error types
- top-level frame parser/writer skeleton
- chunk map parser and strict validator
- chunk-local stream table and transform execution loop
- direct raw stored-stream support
- zstd transform decode wrapper
- FieldLZ transform decode wrapper and side-stream byte validation

A decode-first slice can be tested with hand-built frames before the encoder is complete.

### Phase 2: minimal encoder and round trips

Build the first complete `compress_u32()` path:

- chunker (`16 MiB` source chunks)
- v1 fast-enough FieldLZ parser producing the five logical side streams
- side-stream serialization to canonical bytes
- simple side-stream planner: raw direct store for `< 10` bytes, otherwise magicless zstd transform
- chunk map writer
- frame writer

This phase should produce deterministic maps and round-trip all required datasets, modulo zstd library output details.

### Phase 3: hardening and compatibility tests

Before optimizing, make the bootstrap format robust:

- semantic test vectors (binary golden fixtures deferred until the codec set is stable)
- property tests for arbitrary and structured `Vec<u32>` values
- corrupt/truncated-frame tests
- map validation tests, including duplicate streams, undefined streams, invalid final stream, transform limit errors, and trailing bytes
- FieldLZ corruption tests: reserved token bits, side-stream length mismatches, offset underflow, unused stream entries, output length mismatch

### Phase 4: measurement baseline

Add `binggan` benchmarks and compare against:

- zstd on raw little-endian `u32` bytes as an external baseline
- OpenZL `le-u32` profile when available
- synthetic datasets listed below

This establishes whether parser work or side-stream codecs are the next bottleneck.

### Phase 5: incremental OpenZL-parity components

Add components without changing the outer frame/map:

1. Faster FieldLZ parsers and repeated-offset emission.
2. Compression/decompression level options and parser route selection policy. The current no-options default is zstd level `2` by decision.
3. Literal dictionary/categorical route for low-cardinality literal streams has landed as native ID `1001`; continue tuning the gate from benchmark evidence, especially around cases that exceed the first `u8`/`<= 256` unique-value route.
4. Literal transpose/split routes have landed; continue tuning their gate from benchmark evidence before implementing custom per-lane entropy codecs.
5. Quantize offsets/lengths (`25`, `26`) if raw zstd side streams leave meaningful ratio on the table; quantization may help by exposing smaller code streams before zstd.
6. Heuristic route selection: keep explicit route gates and cheap targeted sampling. Do not add a best-of-N candidate selector, and do not add dataset-name-shaped branches.
7. Rust-native GBT selector parity: possible long-term project mirroring upstream OpenZL's learned numeric selector. Requires a fixed feature extractor, training corpus generated by benchmarking available routes, offline model training, deterministic model export, and benchmark validation across the full suite. Do not start this until the route set and heuristic baseline are stable.
8. Bitpack/constant/Huffman/FSE routes only as long-term speed/overhead/reference-parity work, not as first-version or near-term ratio requirements.

## Component inventory: reuse vs implement ourselves

### Reuse directly

- `zstd` / `zstd-safe` / `zstd-sys`: zstd frame encode/decode, including magicless mode behind the `experimental` feature. We still implement the transform wrapper and validation policy ourselves.
- `thiserror`: error definitions.
- Dev-only crates such as `proptest`/`quickcheck` for property tests and `binggan` for benchmarks. Do not use Criterion.
- OpenZL source code: reuse as a semantic reference and porting guide only, not as linked runtime code.

### Implement in this crate

- Native frame reader/writer and all v1 constants.
- Canonical varint codec and overflow checks.
- Chunker and chunk-total accounting.
- Chunk-local decoding-map validator/executor.
- Stream table representation and element-width metadata.
- Transform trait/interface and planner.
- Direct raw stored-stream route.
- FieldLZ side-stream byte encoders/decoders.
- FieldLZ token decoder and strict corruption checks.
- V1 fast-enough FieldLZ parser/match finder specialized for `u32`.

### Port/adapt from OpenZL later

- Fast and greedy FieldLZ match finders, using `lz4_flex` and OpenZL as design blueprints rather than linked dependencies.
- Repeated-offset encoder heuristics.
- Offset and length quantizers.
- Further literal dictionary/categorical gate tuning, including deciding whether a future `u16` route is worthwhile for medium-cardinality literal streams.
- Further literal transpose/split heuristic tuning and any selector logic that remains explicit and cheap.
- Heuristic route-selection plumbing and targeted sampling for individual routes. Best-of-N candidate evaluation is explicitly not planned.
- Optional long-term GBT selector parity with OpenZL's numeric graph selector; this should be trained/exported as a Rust-native deterministic model rather than linked to OpenZL's C runtime.
- Bitpack/FSE/Huffman/constant transform contracts and route choices, with lower priority than structure-exposing transforms because zstd already provides strong entropy coding.

These should be ports/adaptations into Rust-native code. Avoid linking the OpenZL C graph/runtime, because this crate deliberately does not target OpenZL frame compatibility or the OpenZL graph registry.

### Evaluate later, but do not assume reuse

- Existing Rust bitpacking/FSE/Huffman crates. They are not needed for v1 and should not be a near-term ratio assumption because zstd already covers strong entropy coding. If they are evaluated later for decode speed, stream overhead, or OpenZL parity, and if `open_flexzl` uses OpenZL standard transform IDs, the wire contract must match the transform contract we define. That likely means implementing small, deterministic native versions ourselves or wrapping libraries behind strict compatibility tests.

## Goal

Port the OpenZL `u32` FieldLZ path to Rust under the crate name `open_flexzl`.

The target is a Rust-native compressor focused on `u32` data, with a design that can later extend to other fixed-width integer types.

## Core decisions

- Crate name: `open_flexzl`
- Initial public type: `u32`
- Input API: accept `&[u32]`
- No OpenZL frame compatibility
- No OpenZL graph registry/runtime
- No ACE/training
- No store-on-expansion
- No whole-frame fallback such as “choose smallest of raw/zstd/FieldLZ”
- Compression should always be FieldLZ-shaped
- The native frame should include a simple chunk-local decoding map with OpenZL-like transform IDs
- A codec/transform interface is central to the compressor design; side-stream routing should be planned as transform chains
- The initial implementation routes FieldLZ side streams through store-or-zstd by default, with additional landed literal routes (`literal_dict_u32` and `transpose_split4`) layered through the transform interface; this bootstrap route is not a frame-format limitation
- Track OpenZL’s reference side-stream routing, but do not implement FSE/Huffman/bitpack in v1; zstd side streams are good enough initially, so prioritize benchmark-driven route/transform tuning before custom entropy codecs
- Logical FieldLZ literal input remains plain little-endian `u32` bytes, but the encoder may store that stream through the landed byte-transposed split4 route when its heuristic gate predicts a win.
- Default compression level for the public v1 API: zstd level `2`, chosen deliberately as the current speed/ratio tradeoff rather than OpenZL’s global default `6`.
- FieldLZ token semantics should match OpenZL’s token/repeated-offset model
- Large inputs should be chunked in the native frame; default chunk source size is `16 MiB`

## Type model

FieldLZ itself mostly sees fixed-width fields. For `u32`, the width is 4 bytes and matches are whole-element matches.

Future types should be modeled as:

```text
field width + semantic transform
```

Examples:

- `u32`: width 4, no transform
- `i32`: width 4, likely zigzag or another signed semantic transform before FieldLZ
- `u16`: width 2, no transform
- `u64`: width 8, no transform

So `u32` and `i32` are not necessarily identical at the profile/API level, even though the low-level FieldLZ parse can operate on 4-byte fields for both.

Match-finder datatype scope:

- OpenZL's FieldLZ model is field-width-aware. The public header describes fixed-size streams of width `1`, `2`, `4`, or `8`; the current encoder binding accepts power-of-two widths `2`, `4`, and `8` and explicitly rejects width `1` with a TODO, while the internal fast/greedy match finders have specialized parse functions for `1`, `2`, `4`, and `8` plus a generic fallback.
- OpenZL's match finders operate over bytes internally, but `fieldSize` controls stepping, hashing, match counting, and offset conversion, so emitted offsets/lengths are field-aligned and later converted to element units.
- v1 public API only needs to accept `&[u32]`, but the FieldLZ core should operate on canonical fixed-width byte slices: `&[u8]` plus a compile-time `WIDTH`.
- Internally, positions, offsets, and lengths are still element-based. The byte slice is only the physical representation; the parser steps by `WIDTH` and never emits byte-offset matches.
- For `compress_u32()`, the boundary layer presents each chunk as canonical little-endian bytes, then calls the monomorphized `WIDTH = 4` parser. On little-endian targets this should use a carefully contained zero-copy byte view from the start; on big-endian targets it must copy/convert to preserve deterministic frame bytes.
- The literal side stream can be represented as bytes plus element width instead of `Vec<u32>` in the core. The public `decompress_u32()` converts the final canonical little-endian bytes back to `Vec<u32>`.
- Future `i32` support can likely reuse the `WIDTH = 4` physical path after a semantic transform into canonical bytes.
- Future `u16` and `u64` support should instantiate separate `WIDTH = 2` and `WIDTH = 8` parser paths, not a single slow runtime-width matcher.
- Any generic abstraction must compile away in hot loops; if it does not, keep specialized implementations.

Rust implementation direction for monomorphization:

- Use static dispatch only in the parser/match-finder hot path. Do not use trait objects or dynamic field-width switches inside the inner loop.
- Prefer a const-generic core shaped like `parse_fixed_width<const WIDTH: usize>(canonical_bytes: &[u8], ...)`, with debug/assert validation that `canonical_bytes.len() % WIDTH == 0` and with all loop increments in units of `WIDTH`.
- Specialize helper functions by `WIDTH`, e.g. pair hashing over `2 * WIDTH` bytes, field equality, and fast match counting. LLVM should monomorphize `WIDTH = 4` for v1.
- A small sealed trait may still be useful at the API boundary for converting typed inputs to canonical bytes, but it should not introduce dynamic dispatch in the parser.
- Public APIs remain profile-specific (`compress_u32()` first). Generic internals are for code reuse and monomorphized performance, not for exposing an unstable generic compression API in v1.

## What “FieldLZ-shaped” means

The main transform parses the input into side streams:

```text
u32 input
  -> FieldLZ parser
  -> literals
  -> tokens
  -> offsets
  -> extra literal lengths
  -> extra match lengths
```

The frame stores a decoding map that explains how stored byte streams and transform steps regenerate these five logical FieldLZ input streams, and then how FieldLZ regenerates the original chunk.

The bootstrap encoder can use zstd-or-direct-store for every side stream. Later encoder milestones should change only the chosen map/transform chains, not the FieldLZ parser or outer frame shape.

## OpenZL reference points

Important files in the original OpenZL tree:

- u32 CLI profile setup:
  - `cli/utils/compress_profiles.cpp`
- numeric serial segmenter:
  - `src/openzl/compress/segmenters/segmenter_numeric.c`
- FieldLZ public graph ID:
  - `include/openzl/codecs/zl_field_lz.h`
- FieldLZ graph descriptor:
  - `src/openzl/codecs/lz/graph_lz.h`
- FieldLZ dynamic graph registration:
  - `src/openzl/compress/graph_registry.c`
- FieldLZ graph construction/routing:
  - `src/openzl/codecs/lz/encode_lz_binding.c`
- FieldLZ parser/kernel:
  - `src/openzl/codecs/lz/common_field_lz.h`
  - `src/openzl/codecs/lz/encode_field_lz.c`
  - `src/openzl/codecs/lz/encode_field_lz_sequences.*`
  - `src/openzl/codecs/lz/encode_match_finder_fast_field_lz.c`
  - `src/openzl/codecs/lz/encode_match_finder_greedy_field_lz.c`
- FieldLZ decoder:
  - `src/openzl/codecs/lz/decode_field_lz.c`
- Literal side-stream graph/selector, for later non-zstd codecs:
  - `src/openzl/codecs/lz/encode_field_lz_literals_selector.*`
- Quantizers, for later offset/length codecs:
  - `src/openzl/codecs/quantize/*`

## Graph shape to emulate

OpenZL’s FieldLZ graph is not a single static graph file. It is a dynamic graph function:

```text
EI_fieldLzDynGraph(...)
```

in:

```text
src/openzl/codecs/lz/encode_lz_binding.c
```

OpenZL roughly does:

```text
numeric input?
  -> convert numeric to fixed-width struct/token stream

run ZL_NODE_FIELD_LZ
  -> literals
  -> tokens
  -> offsets
  -> extra literal lengths
  -> extra match lengths

route side streams to child codecs
```

For `open_flexzl` we keep the FieldLZ parse and side-stream concept, and represent side-stream routing with a compact chunk-local decoding map. The first implementation can choose the simplest map, but the plan tracks the reference OpenZL routing so better maps can be added without changing the outer frame.

## v1 wire format spec

Rust-native, simple, versioned, and chunked. Multi-byte fixed-width values are little-endian.

### Constants

```text
MAGIC:                         b"OFZL" = 4f 46 5a 4c
VERSION_V1:                    1
KIND_U32_FIELD_LZ:             1
OPENZL_TYPE_SERIAL:            1
OPENZL_TYPE_STRUCT:            2
OPENZL_TYPE_NUMERIC:           4
OPENZL_TYPE_STRING:            8
STANDARD_TRANSFORM_ID_DELTA_INT: 1
STANDARD_TRANSFORM_ID_ZSTD:    22
STANDARD_TRANSFORM_ID_FIELD_LZ: 24
STANDARD_TRANSFORM_ID_TRANSPOSE_SPLIT4: 31
FIELD_LZ_INPUT_COUNT:          5
MAX_CHUNK_BYTES:               16 * 1024 * 1024
MAX_CHUNK_ELEMENTS_U32:        4,194,304
MAX_OFFSET_ELEMENTS:           4,194,303
DEFAULT_COMPRESSION_LEVEL:     2
DEFAULT_MIN_STREAM_SIZE:       10
RUNTIME_TRANSFORM_INPUT_LIMIT: 2,048
RUNTIME_TRANSFORM_LIMIT:       20,000
RUNTIME_STREAM_LIMIT:          110,000
TRANSFORM_OUT_STREAM_LIMIT:    100,000
```

The `OPENZL_TYPE_*` values intentionally match OpenZL’s `ZL_Type` enum values. For this crate’s initial API, the final output type is always `OPENZL_TYPE_NUMERIC` with element width `4`, and `KIND_U32_FIELD_LZ` supplies the unsigned-`u32` semantic meaning.

`STANDARD_TRANSFORM_ID_ZSTD = 22` and `STANDARD_TRANSFORM_ID_FIELD_LZ = 24` intentionally match OpenZL’s `ZL_StandardTransformID_zstd` and `ZL_StandardTransformID_field_lz` values.

Other OpenZL standard transform IDs to track for reference side-stream routing include:

```text
STANDARD_TRANSFORM_ID_TRANSPOSE_SPLIT4: 31
STANDARD_TRANSFORM_ID_QUANTIZE_OFFSETS: 25
STANDARD_TRANSFORM_ID_QUANTIZE_LENGTHS: 26
STANDARD_TRANSFORM_ID_BITPACK_SERIAL:   27
STANDARD_TRANSFORM_ID_BITPACK_INT:      28
STANDARD_TRANSFORM_ID_CONSTANT_SERIAL:  44
STANDARD_TRANSFORM_ID_CONSTANT_FIXED:   45
STANDARD_TRANSFORM_ID_FSE_V2:           49
STANDARD_TRANSFORM_ID_HUFFMAN_V2:       50
```

Native transform IDs that do not claim OpenZL compatibility:

```text
NATIVE_TRANSFORM_ID_LITERAL_DICT_U32: 1001
```

`literal_dict_u32` is deliberately native for now; if an OpenZL-standard dictionary/categorical transform is later identified and its contract matches, the ID/contract can be revisited before binary fixtures are frozen.

`FIELD_LZ_INPUT_COUNT` is fixed because the FieldLZ transform consumes exactly five logical streams: literals, tokens, offsets, extra literal lengths, and extra match lengths. This is not a limit on how many stored streams or transform steps a chunk map may contain.

The runtime limits above copy OpenZL’s current high-format limits from `src/openzl/common/limits.c`. OpenZL calls transform executions graph nodes; this plan uses transform terminology in the frame spec.

`MAX_OFFSET_ELEMENTS` is one less than the maximum `u32` chunk element count. It is an encoder/window bound for emitted matches; decoder corruption validation primarily follows OpenZL by checking that each offset is non-zero and does not point before the already produced output in the current chunk.

### Varints

All variable-length integers use canonical unsigned LEB64 / LEBU64:

- 7 payload bits per byte, least-significant group first.
- The high bit (`0x80`) means another byte follows.
- Encodings must be minimal/canonical. For example, zero is exactly `00`; `80 00` is invalid.
- At most 10 bytes are accepted.
- On the 10th byte, only payload bit 0 may be set; larger values overflow `u64` and are invalid.
- Decoded lengths/counts must fit in `usize` on the current platform, and all byte-size arithmetic must be checked for overflow.

### Top-level frame

```text
magic:                  4 bytes, exactly MAGIC
version:                u8, exactly VERSION_V1
kind:                   u8, exactly KIND_U32_FIELD_LZ
final_output_type:      u8, exactly OPENZL_TYPE_NUMERIC for `compress_u32()`
final_output_elt_width: varint, exactly 4 for `compress_u32()`
num_elements:           varint, total original u32 element count
chunk_count:            varint
chunks:                 repeated chunk_count times
```

`final_output_type` and `final_output_elt_width` follow the reference frame’s practice of carrying final output type/size metadata, while `kind` carries the Rust-native semantic profile.

`chunk_count` must be `0` iff `num_elements == 0`; otherwise it must be at least `1`.
A v1 decoder rejects trailing bytes after the final chunk.

### Chunk record and decoding map

Each chunk is an independent decode graph. This is intentionally closer to OpenZL’s decoding map than a fixed list of side-stream blobs, but it remains much simpler than the full OpenZL frame format.

```text
chunk_num_elements: varint
stream_slot_count:  varint
stored_stream_count: varint
transform_count:    varint
final_stream_id:    varint
stored_streams:     repeated stored_stream_count times
transforms:         repeated transform_count times, in decode order
```

Chunk validation:

- `chunk_num_elements` must be in `1..=MAX_CHUNK_ELEMENTS_U32`.
- The checked sum of all `chunk_num_elements` values must equal top-level `num_elements`.
- Encoders should emit full `MAX_CHUNK_ELEMENTS_U32` chunks except for the final chunk, but decoders only need to enforce the bounds and total.
- `stream_slot_count` must be at most `RUNTIME_STREAM_LIMIT`.
- `transform_count` must be at most `RUNTIME_TRANSFORM_LIMIT`.
- `stored_stream_count` must be at most `stream_slot_count`.
- Stream IDs are chunk-local slots in `0..stream_slot_count`.
- Every stream slot must be defined exactly once, either by a stored stream or by a transform output.
- Transform records are in decode order; every transform input must refer to an already-defined stream.
- Each transform `input_count` must be at most `RUNTIME_TRANSFORM_INPUT_LIMIT`.
- Each transform `output_count` must be at most `TRANSFORM_OUT_STREAM_LIMIT`.
- Transform output IDs must be unique and not previously defined.
- `final_stream_id` must be defined by the end of the map.
- For `KIND_U32_FIELD_LZ`, `final_stream_id` must be the sole output of exactly one FieldLZ transform.
- FieldLZ history and repeated offsets reset at every chunk boundary.

Stored stream record:

```text
stream_id:           varint
byte_len:            varint
payload:             [u8; byte_len]
```

Stored streams are raw byte streams. In the v1 default encoder, they are either zstd frame payloads consumed by zstd transform records, or raw tiny side streams consumed directly by FieldLZ if direct small-stream store is enabled. A stored stream with `byte_len = 0` is valid for raw direct-store streams, but remains invalid as the input to a zstd transform.

Transform record:

```text
transform_id:        varint
input_count:         varint
input_stream_ids:    repeated input_count varints
output_count:        varint
output_stream_ids:   repeated output_count varints
private_header_len:  varint
private_header:      [u8; private_header_len]
```

A v1 decoder rejects unknown `version` or `kind` values. It also rejects any `transform_id` that the implementation does not support yet. Adding support for more OpenZL-standard transform IDs should not require changing this outer map format.

### v1 transform contracts

#### Zstd transform, ID 22

```text
input_count:        exactly 1
output_count:       exactly 1
private_header:     output_elt_width as one canonical LEBU64
input stream:       zstd frame bytes
output stream:      decoded bytes with element width `output_elt_width`
```

Zstd rules:

- Reference target: OpenZL’s current zstd transform behavior for modern frame versions uses magicless zstd frames because the decoding map already identifies the transform as zstd.
- Before implementation approval, verify that the chosen Rust zstd binding exposes magicless encode/decode, likely through a lower-level API such as `zstd-safe` rather than only the high-level `zstd::bulk` helpers.
- Do not silently substitute normal zstd frames. If magicless support is unavailable or undesirable, record normal zstd frames as an explicit native-frame divergence and update this transform contract.
- The zstd frame content size must be present and must not be `unknown` or `error`.
- `output_elt_width` must be non-zero.
- The decoded byte length from the zstd frame content size must be a multiple of `output_elt_width`.
- Decoder verifies that zstd decompression produces exactly the advertised content size.
- Encoder uses compression level `2` for the public `compress_u32()` default unless a future options API overrides it.
- Empty uncompressed streams are encoded as valid zstd frames for empty input; an empty stored payload is invalid for the zstd transform.
- Store-on-expansion remains a deliberate initial non-goal unless this plan is revised; OpenZL-style small-stream store routing is separate and should be supported by feeding a raw stored stream directly into the consuming transform.

#### Delta transform, ID 1

```text
input_count:        exactly 1
output_count:       exactly 1
private_header:     empty
input stream:       u32 elements (delta sequence, wrapping subtract of previous)
output stream:      u32 elements (running prefix sum, wrapping)
```

Sits after FieldLZ in the decode chain when the delta path is chosen. Encoder
uses heuristics and cheap sampling to decide whether to build the delta route;
it does not build full raw and delta FieldLZ candidates just to compare them.

#### Transpose/split4 transform, ID 31

```text
input_count:        exactly 4
output_count:       exactly 1
private_header:     empty
input streams:      four byte lanes, element width 1, equal lengths
output stream:      interleaved little-endian u32 literal bytes, element width 4
```

This transform is currently used only for the FieldLZ literal side stream. It
recombines four byte-position lanes into the plain literal stream consumed by
FieldLZ, so FieldLZ's logical input contract remains unchanged.

#### Literal dictionary/categorical transform, native ID 1001

```text
input_count:        exactly 2
output_count:       exactly 1
private_header:     code_width as one canonical LEBU64 (1 or 2)
input 0:            dictionary table, little-endian unique u32 values
input 1:            code stream, u8 or little-endian u16 indexes into table
output stream:      little-endian u32 literal bytes, element width 4
```

This transform is intended for the FieldLZ literal side stream only. It is
lossless categorical coding: each literal value is replaced by a compact index
into a chunk-local dictionary table, and decode expands the codes back to the
normal plain literal byte stream before FieldLZ runs. Initial encoder gating:
only consider complete `u32` literal streams with low cardinality (start with
`<= 256` unique values for `u8` codes; `u16` support is optional/follow-up), and
require a cheap estimate such as `dictionary_table_bytes + zstd(code_bytes)` to
beat the current raw/split literal heuristic by a safety margin. This is not
quantize and should not be used for high-cardinality streams like the current
`bursty_mid_dictionary` sample.

#### FieldLZ transform, ID 24

```text
input_count:        exactly 5
output_count:       exactly 1
private_header:     chunk_num_elements as one canonical LEBU64
input 0:            literals bytes, element width 4
input 1:            tokens bytes, element width 2
input 2:            offsets bytes, element width 4
input 3:            extra literal lengths bytes, element width 4
input 4:            extra match lengths bytes, element width 4
output stream:      decoded numeric u32 elements, element width 4
```

The FieldLZ private `chunk_num_elements` must equal the chunk record’s `chunk_num_elements`.

Canonical v1 FieldLZ input stream encodings:

- input 0, literals: little-endian `u32` values, one plain literal stream after any upstream transpose_split4 route has been decoded
- input 1, tokens: little-endian `u16` values
- input 2, offsets: little-endian `u32` values, measured in `u32` elements
- input 3, extra literal lengths: little-endian `u32` values, measured in `u32` elements
- input 4, extra match lengths: little-endian `u32` values, measured in `u32` elements

Pre-FieldLZ validation after side-stream decompression:

- literals byte length must be a multiple of 4 and at most `chunk_num_elements * 4`
- tokens byte length must be a multiple of 2 and at most `chunk_num_elements * 2`
- offsets, extra literal lengths, and extra match lengths byte lengths must be multiples of 4
- offsets and extra-length entry counts must each be at most the token count

## FieldLZ token model

Mirror OpenZL’s FieldLZ token concept:

```text
token: u16
  bits 0..1:   offset code
  bits 2..5:   literal length code
  bits 6..9:   match length code
  bits 10..15: reserved, must be zero in v1
```

A v1 decoder rejects tokens with non-zero reserved bits.

Length semantics for `u32`:

- Minimum match length is `1` element.
- Literal length code `< 15` means that many literal elements.
- Literal length code `15` means `15 + next(extra literal lengths)` literal elements. An extra value of `0` is valid and represents exactly 15 literals.
- Match length code `< 15` means `1 + code` match elements.
- Match length code `15` means `1 + 15 + next(extra match lengths)` match elements. An extra value of `0` is valid and represents exactly 16 match elements.
- Length arithmetic must be checked for `u32`, `usize`, and output-size overflow.

Offset semantics for `u32`:

- Offsets are measured in elements, not bytes.
- Initial repeated offsets at the start of every chunk are `[1, 2, 4]` elements.
- Offset code `0` uses repeated offset 0 and does not change the repeated-offset table.
- Offset code `1` uses repeated offset 1 and moves it to the front: `[r1, r0, r2]`.
- Offset code `2` uses repeated offset 2 and moves it to the front: `[r2, r0, r1]`.
- Offset code `3` reads the next explicit offset from the offsets stream and moves it to the front: `[new, r0, r1]`.

Side streams provide explicit offsets and extra lengths when the token cannot encode them directly.
The v1 fast-enough parser may choose to emit only explicit-offset matches, but the v1 decoder and stream model must support the full repeated-offset semantics from the start.

### Chunk decode semantics

For each chunk:

1. Initialize output as empty and repeated offsets as `[1, 2, 4]`.
2. For each token:
   - Decode/update the offset using the offset code.
   - Decode literal and match lengths, consuming extra length streams as needed.
   - Copy `literal_length` elements from the literal stream into output.
   - Validate `offset != 0` and `offset <= output.len()` after literals are copied.
   - Copy `match_length` elements from `output[output.len() - offset..]` into output. Overlapping match copies are allowed and required.
3. After all tokens, append all remaining literals as last literals.
4. Reject the chunk unless:
   - all literal elements are consumed exactly,
   - all explicit offsets are consumed exactly,
   - all extra literal and match lengths are consumed exactly,
   - output length equals `chunk_num_elements`.

The frame decoder concatenates decoded chunks and verifies the final output length equals top-level `num_elements`.

## Match finding plan

Match finding is performance-critical. The first encoder should be a linear-time, allocation-bounded fast parser, not a deliberately slow reference parser.

### V1 parser: fast-enough fixed-width byte parser, instantiated for `u32`

Canonical v1 encoder contract:

- Parse each chunk independently.
- Core parser input is canonical little-endian bytes with `const WIDTH: usize = 4` for `u32`.
- Whole-element matches only; offsets and lengths are measured in elements, and byte positions are always `element_index * WIDTH`.
- Decoder supports match length 1, but the v1 encoder only emits matches of length at least 2 elements.
- The v1 fast parser emits explicit-offset tokens only (`offset_code = 3`). Repeated-offset emission was prototyped (rep[0]-take-immediately) and reverted: a side-by-side bench against `rust-openzl 0.1` showed the parser is not the bottleneck — OFZL is often *faster* than OpenZL on encode. The larger ratio opportunities have been structure-exposing transforms and side-stream routes; `delta_int`, `transpose_split4`, and `literal_dict_u32` have since landed, while quantized offset/length streams and optional entropy-codec parity remain future work. The decoder still supports the full repeated-offset semantics, so the wire format does not need to change if the encoder starts emitting them later.
- Use a deterministic single-entry hash table keyed by the next two fixed-width fields, i.e. `2 * WIDTH` bytes (`8` bytes for `u32`).
- At element position `i`, look up a recent previous position with the same two-field key. If the offset is in `1..=MAX_OFFSET_ELEMENTS`, extend the match whole-element-wise until mismatch or chunk end.
- If the extended match length is at least 2, emit literals from the current anchor to `i`, then emit the match.
- If no match is emitted, insert the pair at `i` and advance by one or by an adaptive skip step.
- After emitting a match covering element range `[i, match_end)`, insert enough pair starts in the matched range for good follow-up matches; this can be tuned like LZ4/lz4_flex instead of blindly inserting every position if benchmarks favor speed.
- Any trailing elements after the last token are emitted as last literals.
- The same input and options must produce the same frame bytes, modulo zstd library/version behavior.

Use `lz4_flex` as a blueprint for the hot-loop architecture, adapted to FieldLZ semantics:

- reusable power-of-two hash table, preferably allocated once per compression context/chunk worker
- table entries as element positions (`u32` or `usize`), using `pos + 1` if zero means empty
- multiplicative hash of the next `2 * WIDTH` bytes (`8` bytes for `u32`) rather than LZ4's 4-byte sequence hash
- adaptive skip/acceleration after repeated misses, measured in elements
- fast match extension using word-wide comparisons where profitable, but equality remains element-wise and endian-independent at the semantic level
- no byte-offset matches, no byte-length matches, no LZ4 frame/token format
- chunk window bounded by `MAX_OFFSET_ELEMENTS`

Reference files/ideas:

```text
lz4_flex block compressor hot loop and hashtable design
src/openzl/codecs/lz/encode_match_finder_fast_field_lz.c
```

### Stage 2: rep-offset and parser tuning

After v1 round trips and `binggan` benchmarks are in place:

- add repeated-offset candidate checks and repeated-offset token emission
- tune hash-table size and adaptive skip policy
- tune matched-range insertion policy
- reduce allocations and copies in side-stream construction
- compare safe vs carefully-contained unsafe match counting only if benchmarks justify it

### Stage 3: greedy/high-ratio parser

Port/adapt ideas from:

```text
encode_match_finder_greedy_field_lz.c
```

This may become a compression-level option later.

## Codec / transform interface and side-stream planner

The codec interface is central to the compressor. Use OpenZL terminology where possible: a transform consumes one or more streams, has a standard transform ID, optional private header bytes, and regenerates one or more streams.

Conceptually:

```rust
struct Stream {
    id: u32,
    ty: StreamType,
    element_width: usize,
    bytes: Vec<u8>,
}

struct TransformRecord {
    transform_id: u64,
    inputs: Vec<u32>,
    outputs: Vec<u32>,
    private_header: Vec<u8>,
}

trait TransformCodec {
    const STANDARD_ID: u64;
    fn encode(/* typed streams + options */) -> Result</* stored streams + transform records */, Error>;
    fn decode(/* input streams + private header */) -> Result<Vec<Stream>, Error>;
}
```

The exact Rust API can differ, but the design point is that FieldLZ parsing should produce logical side streams, and a side-stream planner should choose transform chains for those streams.

### Reference side-stream routing to track

OpenZL’s `EI_fieldLzDynGraph()` and helper graphs route FieldLZ side streams roughly as follows:

- literals:
  - reference route: transpose/split fixed-width fields into byte lanes, then run a per-lane selector
  - selector options include store, constant, Huffman/delta-Huffman, zstd/delta-zstd depending on stats and compression/decompression levels
  - bootstrap route: plain literal stream through zstd
  - implemented route: transpose/split byte lanes and feed those lanes to store-or-zstd first; this targets numeric data with stable high bytes without immediately reimplementing entropy codecs
  - implemented route: native `literal_dict_u32` for low-cardinality literal streams, targeting hot-head dictionary-like columns
  - current decision: if the literal dictionary gate passes, use dictionary; otherwise if the literal split heuristic passes, use the transposed route. Keep these as explicit heuristic gates, not a generic best-of-N selector.
- tokens:
  - reference route: bitpack for small/fast-decode cases, otherwise Huffman
  - v1/bootstrap route: token stream through zstd or direct store for tiny streams
  - bitpack/Huffman are not near-term ratio requirements because zstd already entropy-codes the token bytes; revisit for per-stream overhead, decode speed, or OpenZL parity
- offsets:
  - reference route: `quantize_offsets` into `u8` codes plus serial raw extra bits, then FSE or bitpack for codes and store/raw for extra bits
  - v1/bootstrap route: offsets stream through zstd or direct store for tiny streams
  - near-term experiment: quantize offsets and then zstd the code/extra streams if offset streams are a measured ratio bottleneck; FSE/bitpack are not required for the first quantize experiment
- extra literal lengths and extra match lengths:
  - reference route: `quantize_lengths` into `u8` codes plus serial raw extra bits, then FSE or bitpack for codes and store/raw for extra bits
  - v1/bootstrap route: length streams through zstd or direct store for tiny streams
  - near-term experiment: quantize lengths only if length streams are large enough to matter; FSE/bitpack are not required for the first quantize experiment
- small streams:
  - reference route: store streams whose byte size is below the configured minimum stream size (`ZL_MINSTREAMSIZE_DEFAULT = 10`, strict `< 10` in `EI_fieldLzDynGraph()`)
  - in this native map, direct store means the stored stream payload is already the raw bytes consumed by the next transform; no zstd transform is inserted for that stream
  - this is distinct from store-on-expansion, which is still a separate first-milestone non-goal

Literal dictionary/categorical coding is not quantize. Dictionary coding maps repeated full `u32` values to compact table indexes; quantize splits integer magnitudes into a bucket code plus exact remainder bits.

The landed `literal_dict_u32` route is intentionally a FieldLZ literal side-stream codec: parse the original chunk with FieldLZ first, then dictionary-code only the remaining literal values. This avoids paying dictionary codes for values already represented by FieldLZ matches. A dictionary-before-FieldLZ route would be a different future categorical pre-transform (`original values -> codes -> FieldLZ over codes -> inverse dictionary after FieldLZ`) and would need separate width/route-selection work; do not conflate it with the literal side-stream transform.

Quantize is not dictionary coding. It is a reversible integer split:

```text
value -> (small code, raw extra bits)
```

For offsets, the code is essentially `floor(log2(value))`, and the extra bits store the low bits needed to reconstruct the exact offset. For lengths, values below 16 have direct codes, then the scheme switches to power-of-two buckets. The code stream is narrow (`u8`) and usually compresses well. First evaluate these structure-exposing splits with zstd-compressed code/extra streams; FSE/bitpack/Huffman are reference-parity or speed/overhead follow-ups, not prerequisites for trying quantize.

Compression level and decompression level should eventually influence parser and side-stream route choices like OpenZL. The public no-options API currently uses zstd compression level `2` and OpenZL default decompression level behavior unless/until options are added.

### Implementation milestones for codecs/transforms

1. Required bootstrap transforms zstd (`22`) and FieldLZ (`24`) are implemented.
2. Direct stored-stream routing for streams below `DEFAULT_MIN_STREAM_SIZE` (`< 10` bytes), matching OpenZL’s small-stream route, is implemented.
3. `delta_int` (`1`) and literal `transpose_split4` (`31`) are implemented.
4. Native literal dictionary/categorical coding (`1001`) for low-cardinality literal streams has landed; first encoder route targets `<= 256` unique `u32` literals with `u8` codes.
5. Prioritize benchmark-driven heuristic tuning before adding more side-stream codecs.
6. Add quantize offsets/lengths (`25`, `26`) later only if benchmarks show raw zstd offset/length streams are a ratio bottleneck; first try zstd over quantized code/extra streams.
7. Add bitpack/FSE/Huffman/constant only as longer-term speed/overhead/reference-parity work, not v1 work and not a near-term ratio assumption.

Adding a transform should add support for another `transform_id` and planner route; it should not require changing the FieldLZ token model or outer chunk map.

## Public API v1

```rust
pub fn compress_u32(input: &[u32]) -> Result<Vec<u8>, Error>;
pub fn decompress_u32(input: &[u8]) -> Result<Vec<u32>, Error>;
```

Possible later additions:

```rust
pub struct CompressOptions {
    pub compression_level: i32,
    pub zstd_level: i32,
}

pub fn compress_u32_with_options(input: &[u32], options: &CompressOptions) -> Result<Vec<u8>, Error>;
```

Do not add options until the basic plan is settled.

## Dependencies

Allowed:

- `zstd` crate for v1 side-stream coding
- small error crate such as `thiserror`
- property/fuzz test dependencies as dev-dependencies
- `binggan` as the benchmark harness

Do not use Criterion. Avoid large codec dependencies other than zstd until we explicitly add more side-stream codecs.

## Tests

Minimum tests:

- empty input
- one element
- small literals-only input
- repeated pattern input
- long repeated run
- monotonic sequence
- low-cardinality data
- deterministic pseudo-random data
- corrupt/truncated frames
- side-stream length mismatch

Property tests:

- generated `Vec<u32>` round-trips
- generated repeated blocks round-trip
- generated mixed literal/match regions round-trip

## Benchmarks

Benchmarks use `binggan` and should measure:

- compression throughput
- decompression throughput
- compression ratio

Datasets:

- checked-in real-world representative files under `test_data`
- `all_real_world_concatenated`
- synthetic datasets remain in the bench source as disabled helpers for ad-hoc runs

Comparisons:

- OpenZL `le-u32` profile when available
- zstd on raw little-endian `u32` bytes as an external baseline, but not as an internal fallback

## Non-goals for the first implementation milestone

- OpenZL frame compatibility
- OpenZL graph registry/runtime
- CLI
- training/ACE
- store-on-expansion, unless this plan is explicitly revised to match that OpenZL default
- automatic whole-frame fallback such as “choose smallest of raw/zstd/FieldLZ”
- full OpenZL literal selector
- Huffman/FSE/flatpack/bitpack implementations
- support for types other than `u32`

The side-stream codecs above are not non-goals for the whole project, but they are not first-version requirements. For v1, store-or-zstd side streams plus the landed structure-exposing literal/delta routes are good enough; use benchmarks to decide whether route tuning, quantize, or parser work is next.

## Resolved v1 planning decisions

1. Approved: the frame uses same semantics with a simpler native chunk-local decoding-map encoding, not exact OpenZL frame compatibility.
2. The map uses OpenZL-like standard transform IDs, not fixed per-stream codec IDs.
3. The first implementation encoder started with independently store-or-zstd-coded FieldLZ input streams; literal dictionary, literal transpose, and delta routes now extend that bootstrap through the transform interface.
4. Reference side-stream routing is tracked and should be implemented incrementally through the transform interface.
5. The logical FieldLZ literal stream is still plain little-endian `u32` bytes, but byte-transposed literal side-stream routing has landed as transform ID `31`, and native literal dictionary routing has landed as transform ID `1001` for low-cardinality streams.
6. The public `compress_u32()` default zstd compression level is `2`.
7. The token model follows exact OpenZL FieldLZ semantics, including repeated offsets and extra lengths.
8. Large inputs are chunked instead of rejected solely because they exceed the FieldLZ offset window.

## Deferred / tracked optimizations

- Literal dictionary/categorical tuning: native ID `1001` is implemented for low-cardinality `u32` literal streams. The current encoder starts with `<= 256` unique values and `u8` codes, dictionary table as little-endian `u32`s, code stream through store-or-zstd, and a heuristic estimate/safety margin. Continue tuning from benchmark evidence and evaluate whether a future `u16` route helps medium-cardinality literal streams.
- Byte-transposed literal lane heuristic tuning: split4 is implemented; continue improving its gate for numeric data with stable high bytes or bounded byte-lane cardinality.
- Quantized offset/length code+extra streams, initially with zstd over the split streams if benchmarks show these side streams dominate.
- Heuristic route selection: keep improving cheap analysis and targeted sampling. Best-of-N candidate selection is explicitly rejected for now.
- Rust-native GBT selector parity: long-term only. Steps: define candidate labels, implement stable feature extraction, generate training data from representative chunks, train offline, export the model into deterministic Rust data/code, and validate against the benchmark suite. Do not add model-shaped code until the route set and heuristic baseline are stable.
- Non-zstd side-stream codecs and selectors, tracked in the reference side-stream routing section, lower priority than structure-exposing transforms because zstd already provides strong entropy coding.
- Compression-level options, decompression-level options, and parser strategy options.

## v1 spec test vectors

These are semantic/pre-zstd vectors. Full binary frame fixtures should be checked in later using the v1 constants, canonical varints, zstd level 2, and the exact zstd crate version in `Cargo.lock` once the codec set is stable.

### Empty frame

Input: `[]`

```text
final_output_type      = OPENZL_TYPE_NUMERIC (4)
final_output_elt_width = 4
num_elements           = 0
chunk_count            = 0
full frame bytes       = 4f 46 5a 4c 01 01 04 04 00 00
```

### One literal

Input: `[0x11223344]`

```text
chunk_num_elements = 1
literals           = [0x11223344]
tokens             = []
offsets            = []
extra_ll           = []
extra_ml           = []
```

Literal stream bytes before zstd:

```text
44 33 22 11
```

### Repeated pair

Input: `[7, 8, 7, 8]`

V1 parser semantic parse:

```text
chunk_num_elements = 4
literals           = [7, 8]
tokens             = [0x004b]
offsets            = [2]
extra_ll           = []
extra_ml           = []
```

Token `0x004b` means explicit offset, literal length 2, match length 2:

```text
offset_code = 3
ll_code     = 2
ml_code     = 1
```

### Repeated run

Input: `[5, 5, 5, 5, 5]`

V1 parser semantic parse:

```text
chunk_num_elements = 5
literals           = [5]
tokens             = [0x00c7]
offsets            = [1]
extra_ll           = []
extra_ml           = []
```

Token `0x00c7` means explicit offset, literal length 1, match length 4:

```text
offset_code = 3
ll_code     = 1
ml_code     = 3
```

## Implementation approval checklist

Approved checklist:

- [x] Wire constants are fixed as listed in the v1 spec, including OpenZL type and transform ID values.
- [x] Canonical unsigned LEBU64 is accepted as the varint format.
- [x] Chunks use the simple native decoding-map structure with OpenZL-like transform IDs and OpenZL-derived runtime limits, rather than exact OpenZL frame-map encoding.
- [x] Top-level frame stores final output metadata: OpenZL type, element width, and element count.
- [x] The implementation supports transform IDs 1 (`delta_int`), 22 (`zstd`), 24 (`field_lz`), and 31 (`transpose_split4`); later transform IDs can be added without changing the outer map.
- [x] Zstd transform behavior is finalized after checking Rust zstd magicless support; target is modern OpenZL magicless payloads with content size present and output element width in the private header. High-level magicless settings are available with the `experimental` feature; content-size validation uses `ZSTD_getFrameHeader_advanced`.
- [x] Direct small-stream store (`byte_size < 10`) is milestone 1.
- [x] Empty input uses `chunk_count = 0`; non-empty chunks must have at least one element.
- [x] Decoder strictly validates reserved token bits, stream length multiples, stream consumption, offsets, chunk totals, output length, map consistency, and trailing bytes.
- [x] V1 match-finder contract is acceptable: `u32`-specialized, lz4_flex-inspired linear parser, pair hash, minimum emitted match length 2, explicit offsets initially, with repeated-offset emission added after benchmarks.
- [x] Benchmarks use `binggan`, not Criterion.
- [x] FSE/Huffman/bitpack side-stream codecs are deferred beyond v1; zstd side streams are sufficient initially.
- [x] Reference side-stream routing is tracked but implemented incrementally through the transform interface.
- [x] Byte-transposed literal routing has landed as transform ID 31; current work is heuristic tuning, not a best-of-N selector.
- [ ] Full binary golden fixtures are deferred. Pinning byte-exact frames now would create churn on every codec/parser change while the format is still evolving. Revisit once the codec set is stable.

## Next step

Implement the native literal dictionary/categorical route for low-cardinality literal streams, then wire it into the explicit route heuristics for `hot_head_dictionary`-like data without adding a best-of-N candidate selector. Keep binary golden fixtures deferred until the codec set is stable.
