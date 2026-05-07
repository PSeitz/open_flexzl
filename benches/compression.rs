//! `binggan` benchmark harness for `open_flexzl` against zstd-on-raw bytes
//! and the upstream OpenZL library (via `rust-openzl`).
//!
//! Compression and decompression are run as two separate phases over the full
//! dataset list — easier to read than alternating compress/decompress per
//! dataset, and the runner's section name (`compression` / `decompression`)
//! makes the split obvious in the output. The OutputValue column shows the
//! compressed size for compress benches and the decoded byte count for
//! decompress benches, so per-dataset ratios are visible without additional
//! prints.

use binggan::{black_box, BenchRunner};
use open_flexzl::{compress_u32, decompress_u32};

#[path = "../tests/common/datasets.rs"]
mod real_world;

const ZSTD_LEVEL: i32 = 6;

fn openzl_compress(data: &[u32]) -> Vec<u8> {
    rust_openzl::compress_numeric(data).expect("openzl encode")
}

fn openzl_decompress(frame: &[u8]) -> Vec<u32> {
    rust_openzl::decompress_numeric::<u32>(frame).expect("openzl decode")
}

struct Prepared {
    name: &'static str,
    data: Vec<u32>,
    raw_bytes: Vec<u8>,
    raw_size: usize,
    ofzl_frame: Vec<u8>,
    zstd_frame: Vec<u8>,
    openzl_frame: Vec<u8>,
}

fn main() {
    let mut datasets = Vec::new();
    //let mut datasets = build_synthetic_datasets();
    for ds in real_world::load_representative_set() {
        datasets.push((ds.label, ds.values));
    }

    let prepared: Vec<Prepared> = datasets
        .into_iter()
        .map(|(name, data)| {
            let raw_bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
            let raw_size = raw_bytes.len();
            let ofzl_frame = compress_u32(&data).expect("ofzl encode");
            let zstd_frame = zstd::bulk::compress(&raw_bytes, ZSTD_LEVEL).expect("zstd encode");
            let openzl_frame = openzl_compress(&data);
            Prepared {
                name,
                data,
                raw_bytes,
                raw_size,
                ofzl_frame,
                zstd_frame,
                openzl_frame,
            }
        })
        .collect();

    let mut runner = BenchRunner::new();

    runner.set_name("compression");
    for p in &prepared {
        let mut group = runner.new_group();
        group.set_name(p.name);
        group.set_input_size(p.raw_size);
        group.register_with_input("ofzl", &p.data, |input| {
            black_box(compress_u32(input).expect("ofzl encode")).len() as u64
        });
        group.register_with_input("zstd_on_raw", &p.raw_bytes, |bytes| {
            black_box(zstd::bulk::compress(bytes, ZSTD_LEVEL).expect("zstd encode")).len() as u64
        });
        group.register_with_input("openzl", &p.data, |input| {
            black_box(openzl_compress(input)).len() as u64
        });
        group.run();
    }

    runner.set_name("decompression");
    for p in &prepared {
        let raw_size = p.raw_size;
        let mut group = runner.new_group();
        group.set_name(p.name);
        group.set_input_size(p.raw_size);
        group.register_with_input("ofzl", &p.ofzl_frame, |frame| {
            // Return decoded byte count (not element count) so the OutputValue
            // column is comparable to the other benches in this group.
            let decoded = black_box(decompress_u32(frame).expect("ofzl decode"));
            (decoded.len() * std::mem::size_of::<u32>()) as u64
        });
        group.register_with_input("zstd_on_raw", &p.zstd_frame, |frame| {
            black_box(zstd::bulk::decompress(frame, raw_size).expect("zstd decode")).len() as u64
        });
        group.register_with_input("openzl", &p.openzl_frame, |frame| {
            let decoded = black_box(openzl_decompress(frame));
            (decoded.len() * std::mem::size_of::<u32>()) as u64
        });
        group.run();
    }
}

const ELEMENTS_PER_DATASET: usize = 1 << 16; // 64 KiB elements = 256 KiB raw

fn build_synthetic_datasets() -> Vec<(&'static str, Vec<u32>)> {
    let n = ELEMENTS_PER_DATASET;

    // 32-element block repeated to fill the dataset.
    let block: Vec<u32> = (0..32u32).collect();
    let repeated_blocks: Vec<u32> = block.iter().copied().cycle().take(n).collect();

    // Strictly monotonic.
    let monotonic: Vec<u32> = (0..n as u32).collect();

    // Low cardinality (7 distinct values).
    let low_cardinality: Vec<u32> = (0..n as u32).map(|i| i % 7).collect();

    // Small-range values (high three bytes always zero).
    let mut state = 0xdead_beefu64;
    let sparse_small_values: Vec<u32> = (0..n)
        .map(|_| {
            state = lcg(state);
            ((state >> 33) as u32) & 0x0000_ffff
        })
        .collect();

    // Full-range pseudo-random.
    let mut state = 0xfeed_faceu64;
    let random: Vec<u32> = (0..n)
        .map(|_| {
            state = lcg(state);
            (state >> 32) as u32
        })
        .collect();

    // Synthetic traces: short records repeated a few times each.
    let mut state = 0xcafe_babeu64;
    let mut synthetic_traces: Vec<u32> = Vec::with_capacity(n);
    while synthetic_traces.len() < n {
        state = lcg(state);
        let record_len = ((state >> 33) as usize % 8) + 1;
        let mut record = Vec::with_capacity(record_len);
        for _ in 0..record_len {
            state = lcg(state);
            record.push(((state >> 33) as u32) & 0x0000_00ff);
        }
        let repeats = ((state >> 40) as usize % 16) + 1;
        for _ in 0..repeats {
            if synthetic_traces.len() >= n {
                break;
            }
            for &value in &record {
                if synthetic_traces.len() >= n {
                    break;
                }
                synthetic_traces.push(value);
            }
        }
    }
    synthetic_traces.truncate(n);

    vec![
        ("repeated_blocks", repeated_blocks),
        ("monotonic", monotonic),
        ("low_cardinality", low_cardinality),
        ("sparse_small_values", sparse_small_values),
        ("random", random),
        ("synthetic_traces", synthetic_traces),
    ]
}

fn lcg(state: u64) -> u64 {
    state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}
