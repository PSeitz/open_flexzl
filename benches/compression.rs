//! `binggan` benchmark harness for `open_flexzl` against zstd-on-raw bytes.
//!
//! For each synthetic dataset we report:
//!
//! - compression throughput (input_size = raw u32 bytes)
//! - decompression throughput (input_size = original raw u32 bytes, the
//!   conventional measure for decompression)
//! - the compressed size each function produced (printed as the OutputValue)
//!
//! Compression ratios are also printed once per dataset before the benches run,
//! so a single `cargo bench` invocation reports throughput + ratio together.

use binggan::{black_box, BenchRunner};
use open_flexzl::{compress_u32, decompress_u32};

const ZSTD_LEVEL: i32 = 6;

fn main() {
    let datasets = build_datasets();
    let mut runner = BenchRunner::new();

    for (name, data) in &datasets {
        let raw_bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        let raw_size = raw_bytes.len();

        let ofzl_frame = compress_u32(data).expect("ofzl encode");
        let zstd_frame = zstd::bulk::compress(&raw_bytes, ZSTD_LEVEL).expect("zstd encode");

        eprintln!(
            "[{name}] raw={raw_size}B  ofzl={}B ({:.2}x)  zstd_on_raw={}B ({:.2}x)",
            ofzl_frame.len(),
            raw_size as f64 / ofzl_frame.len() as f64,
            zstd_frame.len(),
            raw_size as f64 / zstd_frame.len() as f64,
        );

        {
            let mut group = runner.new_group();
            group.set_name(format!("{name} compress"));
            group.set_input_size(raw_size);
            group.register_with_input("ofzl", data, |input| {
                black_box(compress_u32(input).expect("ofzl encode")).len() as u64
            });
            group.register_with_input("zstd_on_raw", &raw_bytes, |bytes| {
                black_box(zstd::bulk::compress(bytes, ZSTD_LEVEL).expect("zstd encode")).len() as u64
            });
            group.run();
        }

        {
            let mut group = runner.new_group();
            group.set_name(format!("{name} decompress"));
            group.set_input_size(raw_size);
            group.register_with_input("ofzl", &ofzl_frame, |frame| {
                // Return decoded byte count (not element count) so the OutputValue
                // column is comparable to the zstd-on-raw bench below.
                let decoded = black_box(decompress_u32(frame).expect("ofzl decode"));
                (decoded.len() * std::mem::size_of::<u32>()) as u64
            });
            group.register_with_input("zstd_on_raw", &zstd_frame, |frame| {
                black_box(zstd::bulk::decompress(frame, raw_size).expect("zstd decode")).len() as u64
            });
            group.run();
        }
    }
}

const ELEMENTS_PER_DATASET: usize = 1 << 16; // 64 KiB elements = 256 KiB raw

fn build_datasets() -> Vec<(&'static str, Vec<u32>)> {
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
