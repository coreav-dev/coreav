//! Arbitrary bytes through the upload policy check (docs/design/policy-check.md):
//! a policy document and a media file, both untrusted. Reading the policy
//! either succeeds or fails with an error; checking the file always gives a
//! verdict that is consistent (accepted exactly when it has no reasons; one
//! `unreadable` reason when the probe rejects the file), serializes to the
//! same bytes every time, and is the evaluation of the probe document (the
//! Matroska one, with the block scan, for a Matroska file).
#![no_main]

use coreav_cli::{check, matroska, native, probe};
use coreav_isobmff::{Allowlist, Limits, SliceSource};
use coreav_policy::{Code, Policy, evaluate};
use libfuzzer_sys::fuzz_target;

/// Policies used when the input does not start with its own.
const POLICIES: [&str; 3] = [
    r#"{"schema": "coreav.policy/1", "streams": {"video": {}, "audio": {}, "subtitle": {},
        "timecode": {}, "data": {}, "unidentified": "ignore", "invalid_tables": "ignore",
        "external_data": "ignore", "multiple_descriptions": "ignore"}}"#,
    r#"{"schema": "coreav.policy/1", "file": {"max_size": 100000, "max_bit_rate": 1000000},
        "container": {"formats": ["mp4"], "fragmented": false},
        "duration": {"max_seconds": 1.5},
        "streams": {"max_count": 2,
          "video": {"codecs": ["h264"], "min_count": 1, "max_count": 1, "max_width": 320,
                    "max_height": 240, "max_pixels": 50000, "max_bit_rate": 100000,
                    "rotations": [0]},
          "audio": {"codecs": ["aac"], "max_count": 1, "max_bit_rate": 64000,
                    "max_channels": 2, "max_sample_rate": 48000}}}"#,
    r#"{"schema": "coreav.policy/1", "streams": {}}"#,
];

fuzz_target!(|data: &[u8]| {
    // `{...}\0media` carries its own policy; otherwise the first byte picks one.
    let (policy, media) = match data.iter().position(|b| *b == 0) {
        Some(split) if data.first() == Some(&b'{') => {
            let Ok(policy) = Policy::from_json(&data[..split]) else {
                return;
            };
            (policy, &data[split + 1..])
        }
        _ => {
            let Some((&pick, media)) = data.split_first() else {
                return;
            };
            let text = POLICIES[usize::from(pick) % POLICIES.len()];
            (Policy::from_json(text.as_bytes()).expect("built-in policy"), media)
        }
    };
    let size = media.len() as u64;
    // Sample work and table bytes as the other targets bound them: the
    // library defaults allow seconds of work by design, more than the
    // fuzzer's timeout under the sanitizer.
    let limits = Limits {
        max_sample_work: 1 << 20,
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    let Ok(verdict) = check::run_source_limited(SliceSource::new(media), size, &policy, &limits) else {
        panic!("a slice never fails to read");
    };
    assert_eq!(verdict.accepted(), verdict.reasons.is_empty());
    let json = verdict.to_json();
    assert_eq!(serde_json::to_vec(&json).unwrap(), serde_json::to_vec(&verdict.to_json()).unwrap());
    // Scanning whatever the policy needs gives the same reasons: a rule that
    // does not need a scanned value does not read one.
    let document = if coreav_mkv::sniff(media) {
        matroska::run_source_with(SliceSource::new(media), size, &coreav_mkv::Allowlist::all(), true)
            .map(|probed| matroska::document(&probed))
    } else {
        probe::run_source_limited(SliceSource::new(media), size, &Allowlist::default(), true, true, &limits)
            .map(|probed| native::document(&probed))
    };
    match document {
        Ok(doc) => {
            assert_eq!(evaluate(&policy, &doc), verdict);
            assert!(verdict.reasons.iter().all(|r| r.code != Code::Unreadable));
        }
        Err(_) => {
            assert_eq!(verdict.reasons.len(), 1);
            assert_eq!(verdict.reasons[0].code, Code::Unreadable);
        }
    }
});
