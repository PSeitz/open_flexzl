//! `delta_int` transform (OpenZL Standard Transform ID 1). Stored as the
//! first value followed by per-element wrapping deltas; decoder undoes via
//! prefix sum. Lossless on every `u32` input regardless of overflow.

use crate::constants::U32_WIDTH;
use crate::Error;

pub(crate) fn apply_u32(input: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(input.len());
    let mut prev = 0u32;
    for &value in input {
        out.push(value.wrapping_sub(prev));
        prev = value;
    }
    out
}

pub(crate) fn undo_bytes(deltas: &[u8]) -> Result<Vec<u8>, Error> {
    if !deltas.len().is_multiple_of(U32_WIDTH) {
        return Err(Error::InvalidTransform(
            "delta_int input length is not a multiple of 4",
        ));
    }
    let mut out = Vec::with_capacity(deltas.len());
    let mut acc: u32 = 0;
    for chunk in deltas.chunks_exact(U32_WIDTH) {
        let delta = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        acc = acc.wrapping_add(delta);
        out.extend_from_slice(&acc.to_le_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cases: &[Vec<u32>] = &[
            Vec::new(),
            vec![0x1122_3344],
            (0..1024u32).collect(),
            vec![0, u32::MAX, 0, u32::MAX],
        ];
        for input in cases {
            let bytes: Vec<u8> = apply_u32(input).iter().flat_map(|d| d.to_le_bytes()).collect();
            let recovered: Vec<u32> = undo_bytes(&bytes)
                .unwrap()
                .chunks_exact(U32_WIDTH)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            assert_eq!(&recovered, input);
        }
    }
}
