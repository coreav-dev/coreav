//! The metadata walk (`coreav_isobmff::metadata::read`): any input reads
//! without an error (a slice never fails to read) or a panic, and what it
//! keeps stays within the limits.
#![no_main]

use coreav_isobmff::metadata::{ItemValue, ListKey, read};
use coreav_isobmff::{Limits, SliceSource};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_retained_bytes: 1 << 16,
        ..Limits::default()
    };
    let meta = read(SliceSource::new(data), &limits).expect("a slice never fails to read");
    let mut kept = 0usize;
    for item in &meta.items {
        kept += match &item.value {
            ItemValue::UserData { payload, .. } => payload.len(),
            ItemValue::ListItem { key, data, .. } => {
                let key = match key {
                    ListKey::Atom(_) => 0,
                    ListKey::Key { name, .. } => name.len(),
                    ListKey::Free { mean, name } => {
                        mean.as_ref().map_or(0, Vec::len) + name.as_ref().map_or(0, Vec::len)
                    }
                };
                key + data.iter().map(|d| d.value.len()).sum::<usize>()
            }
        };
    }
    for track in &meta.tracks {
        kept += track.handler_name.len();
        if let Some(entry) = &track.sample_entry {
            kept += entry
                .children
                .iter()
                .chain(entry.wave.iter())
                .map(|c| c.payload.len())
                .sum::<usize>();
        }
    }
    assert!(kept as u64 <= limits.max_retained_bytes);
});
