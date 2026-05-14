//! Loader for the checked-in `test_data` representative `.uncomp` files.
//!
//! Each `.uncomp` file is a UTF-8 text stream with one decimal `u32` per line.
//! The datasets below span a useful range of distribution shapes
//! (single-symbol floods, small cycles, hot-head dictionaries, wide tails,
//! all-unique, and mostly-unique clustered streams).
//!
//! Resolution order for the data directory:
//!
//! 1. `OFZL_TEST_DATA_DIR` environment variable, if set.
//! 2. `$CARGO_MANIFEST_DIR/test_data`.
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

/// `(label, file)` for the curated representative set.
pub const REPRESENTATIVE_SET: &[(&str, &str)] = &[
    ("single_symbol_floods", "single_symbol_floods.uncomp"), // TemplateId(40).col1.uncomp
    ("ten_value_cycle", "ten_value_cycle.uncomp"),           // TemplateId(42).col0.uncomp
    ("hot_head_dictionary", "hot_head_dictionary.uncomp"),   // TemplateId(61).col2.uncomp
    ("bursty_mid_dictionary", "bursty_mid_dictionary.uncomp"), // TemplateId(43).col3.uncomp
    ("wide_tail", "wide_tail.uncomp"),                       // TemplateId(17).col1.uncomp
    ("all_unique", "all_unique.uncomp"),                     // TemplateId(13).col0.uncomp
    ("clustered_wide_tail", "clustered_wide_tail.uncomp"),   // TemplateId(15).col0.uncomp
    ("mostly_unique_wide_tail", "mostly_unique_wide_tail.uncomp"), // TemplateId(39).col6.uncomp
    ("android_v2_template_25", "android_v2_template_25.uncomp"), // representative 16 MiB chunk from moshiki Android_v2 TemplateId(25).col
    ("android_v2_template_29", "android_v2_template_29.uncomp"), // representative 16 MiB chunk from moshiki Android_v2 TemplateId(29).col
];

pub fn data_dir() -> PathBuf {
    if let Ok(env) = std::env::var("OFZL_TEST_DATA_DIR") {
        return PathBuf::from(env);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data")
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
