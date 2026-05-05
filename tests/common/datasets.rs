//! Loader for the `num_flex/test_data` representative `.uncomp` files.
//!
//! Each `.uncomp` file is a UTF-8 text stream with one decimal `u32` per line.
//! The six datasets below come from `num_flex`'s `representative_sets.json` and
//! span a useful range of distribution shapes (single-symbol floods, small
//! cycles, hot-head dictionaries, wide tails, all-unique).
//!
//! Resolution order for the data directory:
//!
//! 1. `OFZL_TEST_DATA_DIR` environment variable, if set.
//! 2. `$CARGO_MANIFEST_DIR/../num_flex/test_data`.
//!
//! Missing files are skipped with a warning so the suite still passes on
//! machines that don't have the dataset checked out.

#![allow(dead_code)]

use std::path::PathBuf;

pub struct RealWorldDataset {
    pub label: &'static str,
    pub file: &'static str,
    pub values: Vec<u32>,
}

/// `(label, file)` for the curated representative set, in the same order as
/// `num_flex/test_data/representative_sets.json`.
pub const REPRESENTATIVE_SET: &[(&str, &str)] = &[
    ("single_symbol_floods", "TemplateId(40).col1.uncomp"),
    ("ten_value_cycle", "TemplateId(42).col0.uncomp"),
    ("hot_head_dictionary", "TemplateId(61).col2.uncomp"),
    ("bursty_mid_dictionary", "TemplateId(43).col3.uncomp"),
    ("wide_tail", "TemplateId(17).col1.uncomp"),
    ("all_unique", "TemplateId(13).col0.uncomp"),
];

pub fn data_dir() -> PathBuf {
    if let Ok(env) = std::env::var("OFZL_TEST_DATA_DIR") {
        return PathBuf::from(env);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|parent| parent.join("num_flex/test_data"))
        .unwrap_or_else(|| PathBuf::from("../num_flex/test_data"))
}

pub fn load_representative_set() -> Vec<RealWorldDataset> {
    let dir = data_dir();
    let mut out = Vec::with_capacity(REPRESENTATIVE_SET.len());
    for &(label, file) in REPRESENTATIVE_SET {
        let path = dir.join(file);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let values: Vec<u32> = text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| {
                        line.parse::<u32>().unwrap_or_else(|_| {
                            panic!("non-u32 line in {}: {:?}", path.display(), line)
                        })
                    })
                    .collect();
                out.push(RealWorldDataset {
                    label,
                    file,
                    values,
                });
            }
            Err(err) => {
                eprintln!(
                    "[ofzl] skipping {}: {} (set OFZL_TEST_DATA_DIR to override)",
                    path.display(),
                    err
                );
            }
        }
    }
    out
}
