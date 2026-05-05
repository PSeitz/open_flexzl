//! Rust-native fixed-width numeric compression inspired by OpenZL.
//!
//! The v1 public API supports `u32` slices and writes a native `OFZL` frame
//! containing chunk-local transform maps, zstd side-stream transforms, and a
//! FieldLZ transform.

mod constants;
mod delta;
mod error;
mod field_lz;
mod frame;
mod varint;
mod zstd_codec;

pub use error::Error;

/// Compress a slice of unsigned 32-bit integers into an `OFZL` v1 frame.
pub fn compress_u32(input: &[u32]) -> Result<Vec<u8>, Error> {
    frame::compress_u32(input)
}

/// Decompress an `OFZL` v1 frame produced by [`compress_u32`].
pub fn decompress_u32(input: &[u8]) -> Result<Vec<u32>, Error> {
    frame::decompress_u32(input)
}
