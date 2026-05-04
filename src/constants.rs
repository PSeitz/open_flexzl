pub(crate) const MAGIC: &[u8; 4] = b"OFZL";
pub(crate) const VERSION_V1: u8 = 1;
pub(crate) const KIND_U32_FIELD_LZ: u8 = 1;

pub(crate) const OPENZL_TYPE_NUMERIC: u8 = 4;

pub(crate) const STANDARD_TRANSFORM_ID_ZSTD: u64 = 22;
pub(crate) const STANDARD_TRANSFORM_ID_FIELD_LZ: u64 = 24;

pub(crate) const FIELD_LZ_INPUT_COUNT: usize = 5;
pub(crate) const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const U32_WIDTH: usize = 4;
pub(crate) const U16_WIDTH: usize = 2;
pub(crate) const MAX_CHUNK_ELEMENTS_U32: usize = MAX_CHUNK_BYTES / U32_WIDTH;
pub(crate) const MAX_OFFSET_ELEMENTS: usize = MAX_CHUNK_ELEMENTS_U32 - 1;
pub(crate) const DEFAULT_COMPRESSION_LEVEL: i32 = 6;
pub(crate) const DEFAULT_MIN_STREAM_SIZE: usize = 10;

pub(crate) const RUNTIME_TRANSFORM_INPUT_LIMIT: usize = 2_048;
pub(crate) const RUNTIME_TRANSFORM_LIMIT: usize = 20_000;
pub(crate) const RUNTIME_STREAM_LIMIT: usize = 110_000;
pub(crate) const TRANSFORM_OUT_STREAM_LIMIT: usize = 100_000;
