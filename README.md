# open_flexzl

Rust-native FieldLZ for `u32`, inspired by OpenZL's `le-u32` path. This crate
intentionally does **not** target OpenZL frame compatibility.

## Status

Planning / implementation reset.

The previous Rust prototype was removed because it diverged from the current
plan. The public API shape is reserved, but the functions currently return
`Error::NotImplemented` until `plan.md` is explicitly approved for
implementation.

```rust
let compressed = open_flexzl::compress_u32(&values)?;
let decoded = open_flexzl::decompress_u32(&compressed)?;
```

See [`plan.md`](plan.md) for the v1 frame, transform, match-finder, and
implementation roadmap.
