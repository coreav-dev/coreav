#![allow(clippy::indexing_slicing)]

use super::*;
use crate::Samples;

const KEY: [u8; 15] = [
    0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00, 0x01, 0x00,
];

#[test]
fn refreshed_slots_share_private_pictures_and_outputs_do_not_alias_them() {
    let mut d = FrameDecoder::new(Limits::default());
    let expected = d.decode(&KEY).unwrap().pop().unwrap();
    let first = d.references[0].as_ref().unwrap();
    for reference in d.references.iter().flatten() {
        assert!(Arc::ptr_eq(first, reference));
    }
    let mut handed_out = d.decode(&[0x88]).unwrap().pop().unwrap();
    if let Samples::Eight(samples) = &mut handed_out.planes[0] {
        samples.fill(3);
    }
    assert_ne!(handed_out, expected);
    assert_eq!(d.decode(&[0x88]).unwrap(), vec![expected]);
}

#[test]
fn hidden_frames_refresh_references_without_show_existing_advancing_history() {
    let mut d = FrameDecoder::new(Limits::default());
    let mut hidden = KEY;
    hidden[0] &= !2;
    assert!(d.decode(&hidden).unwrap().is_empty());
    let p = d.previous.as_ref().unwrap();
    assert!(!p.shown && p.key);
    let old_modes = p.modes.as_ptr();
    let old_contexts = d.contexts.clone();
    let stored = d.references[0].as_ref().unwrap().as_ref().clone();
    assert_eq!(d.decode(&[0x88]).unwrap(), vec![stored]);
    assert_eq!(d.previous.as_ref().unwrap().modes.as_ptr(), old_modes);
    assert!(!d.previous.as_ref().unwrap().shown);
    assert_eq!(d.contexts, old_contexts);
}

#[test]
fn errors_leave_slots_contexts_carried_values_and_history_unchanged() {
    let mut d = FrameDecoder::new(Limits::default());
    d.decode(&KEY).unwrap();
    let slots = d.slots;
    let contexts = d.contexts.clone();
    let carried = d.carried;
    let references = d.references.clone();
    let modes = d.previous.as_ref().unwrap().modes.as_ptr();
    d.limits.max_width = 1;
    assert_eq!(d.decode(&KEY), Err(Error::Limit(LimitKind::Width)));
    assert!(d.decode(&[]).is_err());
    assert_eq!(d.slots, slots);
    assert_eq!(d.contexts, contexts);
    assert_eq!(d.carried, carried);
    assert_eq!(d.previous.as_ref().unwrap().modes.as_ptr(), modes);
    for (a, b) in d.references.iter().zip(&references) {
        assert!(Arc::ptr_eq(a.as_ref().unwrap(), b.as_ref().unwrap()));
    }
}

#[test]
fn show_existing_requires_a_decoded_reference() {
    let mut d = FrameDecoder::new(Limits::default());
    assert!(d.decode(&[0x88]).is_err());
    assert!(d.references.iter().all(Option::is_none));
    assert!(d.previous.is_none());
}

#[test]
fn a_failed_superframe_member_keeps_earlier_successful_members_committed() {
    let mut d = FrameDecoder::new(Limits::default());
    let mut packet = KEY.to_vec();
    packet.extend_from_slice(&[0x80, 0xc1, 15, 1, 0xc1]);
    assert!(d.decode(&packet).is_err());
    let shown = d.decode(&[0x88]).unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!((shown[0].width, shown[0].height), (64, 48));
}
