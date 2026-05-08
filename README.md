# open_flexzl

Rust-native FieldLZ for `u32`, inspired by OpenZL's `le-u32` path. This crate
intentionally does **not** target OpenZL frame compatibility.

## Status

Initial v1 implementation.

The crate exposes `compress_u32()` / `decompress_u32()` for a native `OFZL` v1
frame with chunk-local transform maps, FieldLZ side streams, direct tiny-stream
store, and magicless zstd side-stream transforms.

```rust
let compressed = open_flexzl::compress_u32(&values)?;
let decoded = open_flexzl::decompress_u32(&compressed)?;
```

See [`docs/compressor.md`](docs/compressor.md) for a current-vs-planned
explanation of how compression works, and [`plan.md`](plan.md) for the v1 frame,
transform, match-finder, and implementation roadmap.

### Benchmarks

`cargo bench`

To select a specific benchmark, use `cargo bench hot_head` or `cargo bench decompress`.
