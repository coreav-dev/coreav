//! The H.264 byte stream reader's `Limits`: the default values, and the
//! count of NAL units in an access unit at its boundary.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_transcode::annexb::{AnnexBReader, Error, Limits};

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const FILLER_DATA: [u8; 2] = [0x0C, 0xFF];

fn fillers_then_start_code(count: usize) -> Vec<u8> {
    let mut stream = Vec::new();
    for _ in 0..count {
        stream.extend_from_slice(&START_CODE);
        stream.extend_from_slice(&FILLER_DATA);
    }
    stream.extend_from_slice(&START_CODE);
    stream
}

#[test]
fn defaults_match_the_documented_values() {
    assert_eq!(
        Limits::default(),
        Limits {
            max_nal_unit_bytes: 128 << 20,
            max_access_unit_bytes: 128 << 20,
            max_nal_units_per_access_unit: 1024,
            max_period_pictures: 65_536,
            max_period_bytes: 1 << 30,
            max_pictures: u64::MAX,
        }
    );
}

#[test]
fn exactly_max_nal_units_in_an_access_unit_are_read_and_one_more_is_refused() {
    let limits = Limits {
        max_nal_units_per_access_unit: 3,
        ..Limits::default()
    };
    let mut at_the_limit = AnnexBReader::new(limits);
    assert_eq!(at_the_limit.push(&fillers_then_start_code(3)), Ok(()));
    let mut past_it = AnnexBReader::new(limits);
    assert_eq!(
        past_it.push(&fillers_then_start_code(4)),
        Err(Error::Limit("NAL units in an access unit"))
    );
    assert_eq!(past_it.push(&START_CODE), Err(Error::Closed));
}
