use thiserror::Error as ThisError;

/// Errors returned by the `open_flexzl` encoder and decoder.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("unexpected end of input")]
    UnexpectedEof,

    #[error("trailing bytes after final chunk")]
    TrailingBytes,

    #[error("invalid frame: {0}")]
    InvalidFrame(&'static str),

    #[error("invalid varint: {0}")]
    InvalidVarint(&'static str),

    #[error("limit exceeded: {0}")]
    LimitExceeded(&'static str),

    #[error("invalid decoding map: {0}")]
    InvalidMap(&'static str),

    #[error("unsupported transform id {0}")]
    UnsupportedTransform(u64),

    #[error("invalid transform: {0}")]
    InvalidTransform(&'static str),

    #[error("invalid FieldLZ stream: {0}")]
    InvalidFieldLz(&'static str),

    #[error("zstd error: {0}")]
    Zstd(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    pub(crate) fn zstd_code(code: usize) -> Self {
        Self::Zstd(zstd::zstd_safe::get_error_name(code).to_owned())
    }

    pub(crate) fn zstd_io(err: std::io::Error) -> Self {
        Self::Zstd(err.to_string())
    }
}
