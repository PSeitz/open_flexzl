//! Rust-native fixed-width numeric compression inspired by OpenZL.
//!
//! Implementation status: intentionally reset to stubs.
//!
//! The previous code in this crate was a scratch/prototype implementation that
//! diverged from the approved direction in `open_flexzl/plan.md`. Do not revive
//! it wholesale. Implement the crate from the plan after the implementation
//! approval checklist is explicitly approved.

mod error;

pub use error::Error;

/// Compress a slice of unsigned 32-bit integers.
///
/// Planned API shape only. See `open_flexzl/plan.md` for the approved format
/// before implementing this function.
pub fn compress_u32(_input: &[u32]) -> Result<Vec<u8>, Error> {
    Err(Error::NotImplemented)
}

/// Decompress a frame produced by [`compress_u32`].
///
/// Planned API shape only. See `open_flexzl/plan.md` for the approved format
/// before implementing this function.
pub fn decompress_u32(_input: &[u8]) -> Result<Vec<u32>, Error> {
    Err(Error::NotImplemented)
}
