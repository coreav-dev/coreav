//! Complete bounded AVC transport-header admission, independently of decoding.

#![allow(clippy::expect_used)]

use coreav_isobmff::avc_copy_facts;

const RECORD: &[u8] = &[
    0x01, 0x64, 0x00, 0x0a, 0xff, 0xe1, 0x00, 0x18, 0x67, 0x64, 0x00, 0x0a, 0xac, 0xd9, 0x44, 0x7b,
    0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0x00, 0xf1, 0x22, 0x59, 0x60,
    0x01, 0x00, 0x06, 0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0, 0xfd, 0xf8, 0xf8, 0x00,
];

#[test]
fn complete_sps_retains_explicit_transport_facts() {
    let facts = avc_copy_facts(RECORD).expect("complete SPS");
    assert_eq!(facts.profile_idc, 100);
    assert_eq!(facts.constraint_flags, 0);
    assert_eq!(facts.level_idc, 10);
    assert_eq!(facts.sps_id, 0);
    assert_eq!(facts.chroma_format_idc, 1);
    assert_eq!(facts.bit_depth_luma_minus8, 0);
    assert_eq!(facts.bit_depth_chroma_minus8, 0);
    assert!(facts.frame_mbs_only);
    assert_eq!(facts.num_units_in_tick, 1);
    assert_eq!(facts.time_scale, 48);
    assert!(!facts.fixed_frame_rate);
    assert_eq!(facts.max_num_reorder_frames, Some(2));
    assert_eq!(facts.max_dec_frame_buffering, Some(4));
}

#[test]
fn copy_facts_refuse_incomplete_or_noncanonical_sps_bits() {
    for end in 0..32 {
        assert!(avc_copy_facts(RECORD.get(..end).unwrap_or_default()).is_none());
    }
    for (offset, byte) in [(8, 0xe7), (8, 0x07), (10, 1), (20, 4), (31, 0x61)] {
        let mut changed = RECORD.to_vec();
        if let Some(target) = changed.get_mut(offset) {
            *target = byte;
        }
        assert!(avc_copy_facts(&changed).is_none(), "offset {offset}");
    }
    let mut extended = RECORD.to_vec();
    extended.insert(32, 0);
    if let Some(length) = extended.get_mut(7) {
        *length += 1;
    }
    assert!(avc_copy_facts(&extended).is_none());
}
