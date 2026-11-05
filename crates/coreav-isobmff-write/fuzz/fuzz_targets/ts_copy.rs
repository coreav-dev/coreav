//! Arbitrary bounded ISO input through sealed MPEG-TS preparation.
#![no_main]

use coreav_isobmff::SliceSource;
use coreav_isobmff_write::ts_output::{Limits, prepare};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_input_bytes: 64 << 10,
        max_output_bytes: 64 << 10,
        max_planning_bytes: 1 << 20,
        max_table_bytes: 32 << 10,
        max_metadata_bytes: 16 << 10,
        max_access_unit_bytes: 16 << 10,
        max_samples: 128,
        max_nals_per_sample: 16,
        max_nals: 512,
        max_work: 4096,
    };
    if let Ok(prepared) = prepare(SliceSource::new(data), 0, &limits) {
        let mut first = Vec::new();
        prepared.write_to(&mut first).expect("memory output");
        let mut second = Vec::new();
        prepared.write_to(&mut second).expect("repeat memory output");
        assert_eq!(first, second);
        assert!(!first.is_empty() && first.len() <= limits.max_output_bytes);
        assert_eq!(first.len() % 188, 0);
        assert!(first.chunks_exact(188).all(|packet| packet[0] == 0x47));
    }
});
