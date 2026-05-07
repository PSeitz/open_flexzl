//! Round-trip tests on the checked-in curated test datasets.
//!
//! These tests are silently skipped when the data directory is unavailable so
//! the suite still passes on machines without the dataset checked out (see
//! `tests/common/datasets.rs`).

mod common;

use common::datasets::{load_representative_set, REPRESENTATIVE_SET};
use open_flexzl::{compress_u32, decompress_u32};

#[test]
fn real_world_round_trip() {
    let datasets = load_representative_set();
    if datasets.is_empty() {
        eprintln!(
            "[ofzl] no real-world datasets available; expected at most {} entries",
            REPRESENTATIVE_SET.len()
        );
        return;
    }

    for ds in &datasets {
        eprintln!(
            "[ofzl] {} ({}): {} values",
            ds.label,
            ds.file,
            ds.values.len()
        );
        let frame = compress_u32(&ds.values).expect("ofzl encode");
        let decoded = decompress_u32(&frame).expect("ofzl decode");
        assert_eq!(
            decoded, ds.values,
            "round-trip mismatch on {} ({})",
            ds.label, ds.file
        );
    }
}
