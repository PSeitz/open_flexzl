use std::io::Write;
use std::mem::MaybeUninit;

use crate::Error;

pub(crate) fn encode_magicless(src: &[u8], level: i32) -> Result<Vec<u8>, Error> {
    let pledged_size = u64::try_from(src.len())
        .map_err(|_| Error::LimitExceeded("zstd source length does not fit u64"))?;
    let mut encoder =
        zstd::stream::write::Encoder::new(Vec::new(), level).map_err(Error::zstd_io)?;
    encoder.include_magicbytes(false).map_err(Error::zstd_io)?;
    encoder.include_contentsize(true).map_err(Error::zstd_io)?;
    encoder.include_checksum(false).map_err(Error::zstd_io)?;
    encoder.include_dictid(false).map_err(Error::zstd_io)?;
    encoder
        .set_pledged_src_size(Some(pledged_size))
        .map_err(Error::zstd_io)?;
    encoder.write_all(src).map_err(Error::zstd_io)?;
    encoder.finish().map_err(Error::zstd_io)
}

pub(crate) fn decode_magicless(src: &[u8], output_elt_width: usize) -> Result<Vec<u8>, Error> {
    if src.is_empty() {
        return Err(Error::InvalidTransform(
            "zstd transform input payload must not be empty",
        ));
    }
    if output_elt_width == 0 {
        return Err(Error::InvalidTransform(
            "zstd output element width must be non-zero",
        ));
    }

    let content_size = magicless_content_size(src)?;
    let output_len = usize::try_from(content_size)
        .map_err(|_| Error::LimitExceeded("zstd content size does not fit usize"))?;
    if output_len % output_elt_width != 0 {
        return Err(Error::InvalidTransform(
            "zstd content size is not a multiple of output element width",
        ));
    }

    let mut output = vec![0u8; output_len];
    let mut dctx = zstd::zstd_safe::DCtx::create();
    dctx.set_parameter(zstd::zstd_safe::DParameter::Format(
        zstd::zstd_safe::FrameFormat::Magicless,
    ))
    .map_err(Error::zstd_code)?;
    let written = dctx
        .decompress(&mut output[..], src)
        .map_err(Error::zstd_code)?;
    if written != output_len {
        return Err(Error::InvalidTransform(
            "zstd decompressed size did not match frame content size",
        ));
    }
    Ok(output)
}

pub(crate) fn magicless_content_size(src: &[u8]) -> Result<u64, Error> {
    let mut header = MaybeUninit::<zstd::zstd_safe::zstd_sys::ZSTD_FrameHeader>::uninit();
    let code = unsafe {
        zstd::zstd_safe::zstd_sys::ZSTD_getFrameHeader_advanced(
            header.as_mut_ptr(),
            src.as_ptr().cast(),
            src.len(),
            zstd::zstd_safe::zstd_sys::ZSTD_format_e::ZSTD_f_zstd1_magicless,
        )
    };

    if unsafe { zstd::zstd_safe::zstd_sys::ZSTD_isError(code) } != 0 {
        return Err(Error::zstd_code(code));
    }
    if code != 0 {
        return Err(Error::UnexpectedEof);
    }

    let header = unsafe { header.assume_init() };
    let content_size = header.frameContentSize;
    if content_size == zstd::zstd_safe::CONTENTSIZE_UNKNOWN {
        return Err(Error::InvalidTransform(
            "zstd frame content size is not present",
        ));
    }
    if content_size == zstd::zstd_safe::CONTENTSIZE_ERROR {
        return Err(Error::InvalidTransform(
            "zstd frame content size is invalid",
        ));
    }
    Ok(content_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magicless_round_trip_with_content_size() {
        for input in [&b""[..], &b"hello hello hello hello"[..]] {
            let compressed = encode_magicless(input, 6).unwrap();
            assert!(!compressed.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]));
            assert_eq!(
                magicless_content_size(&compressed).unwrap(),
                input.len() as u64
            );
            assert_eq!(decode_magicless(&compressed, 1).unwrap(), input);
        }
    }
}
