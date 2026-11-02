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
    d.segment_ids = vec![3; 48];
    let segments = d.segment_ids.clone();
    let carried = d.carried;
    let references = d.references.clone();
    let modes = d.previous.as_ref().unwrap().modes.as_ptr();
    d.limits.max_width = 1;
    assert_eq!(d.decode(&KEY), Err(Error::Limit(LimitKind::Width)));
    assert!(d.decode(&[]).is_err());
    assert_eq!(d.slots, slots);
    assert_eq!(d.contexts, contexts);
    assert_eq!(d.segment_ids, segments);
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

fn segment_packet(
    shown: bool,
    resilient: bool,
    width: u32,
    enabled: bool,
    update_map: bool,
) -> Vec<u8> {
    let mut bits = Vec::new();
    let mut put = |n, value: u32| {
        for i in (0..n).rev() {
            bits.push(value >> i & 1 != 0);
        }
    };
    put(2, 2);
    put(2, 0);
    put(1, 0);
    put(1, 1);
    put(1, u32::from(shown));
    put(1, u32::from(resilient));
    if !shown {
        put(1, 0);
    }
    if !resilient {
        put(2, 0);
    }
    put(8, 0);
    for _ in 0..3 {
        put(3, 0);
        put(1, 0);
    }
    put(3, 0);
    put(16, width - 1);
    put(16, 47);
    put(1, 0);
    put(1, 0);
    put(1, 1);
    if !resilient {
        put(1, 0);
        put(1, 1);
    }
    put(2, 0);
    put(6, 0);
    put(3, 0);
    put(1, 0);
    put(8, 60);
    put(3, 0);
    put(1, u32::from(enabled));
    if enabled {
        put(1, u32::from(update_map));
        if update_map {
            for _ in 0..7 {
                put(1, 0);
            }
            put(1, 0);
        }
        put(1, 0);
    }
    put(1, 0);
    put(16, 1);
    let mut data: Vec<u8> = bits
        .chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0, |b, (i, v)| b | u8::from(*v) << (7 - i))
        })
        .collect();
    data.extend_from_slice(&[0, 0]);
    data
}

#[test]
fn a_hidden_map_update_commits_only_after_the_frame_succeeds() {
    let mut d = decoder_with_segment_history();
    let packet = segment_packet(false, false, 64, true, true);
    d.limits.max_width = 1;
    assert!(d.decode(&packet).is_err());
    assert_eq!(d.segment_ids, vec![3; 48]);
    d.limits = Limits::default();
    assert!(d.decode(&packet).unwrap().is_empty());
    assert_eq!(d.segment_ids, vec![0; 48]);
    assert!(!d.previous.as_ref().unwrap().shown);
    d.decode(&[0x88]).unwrap();
    assert_eq!(d.segment_ids, vec![0; 48]);
}

fn decoder_with_segment_history() -> FrameDecoder {
    let mut d = FrameDecoder::new(Limits::default());
    d.decode(&KEY).unwrap();
    d.segment_ids = vec![3; 48];
    for feature in &mut d.carried.features {
        feature[2] = Some(0);
        feature[3] = Some(0);
    }
    d
}

#[test]
fn hidden_and_disabled_frames_keep_the_last_updated_segment_map() {
    let mut d = decoder_with_segment_history();
    assert!(
        d.decode(&segment_packet(false, false, 64, true, false))
            .unwrap()
            .is_empty()
    );
    assert!(
        d.previous
            .as_ref()
            .unwrap()
            .modes
            .iter()
            .all(|m| m.segment_id == 3)
    );
    let history = d.segment_ids.clone();
    d.decode(&[0x88]).unwrap();
    assert_eq!(d.segment_ids, history);
    d.decode(&segment_packet(true, false, 64, false, false))
        .unwrap();
    assert!(
        d.previous
            .as_ref()
            .unwrap()
            .modes
            .iter()
            .all(|m| m.segment_id == 0)
    );
    assert_eq!(d.segment_ids, history);
    d.decode(&segment_packet(true, false, 64, true, false))
        .unwrap();
    assert!(
        d.previous
            .as_ref()
            .unwrap()
            .modes
            .iter()
            .all(|m| m.segment_id == 3)
    );
}

#[test]
fn resized_and_independent_frames_clear_segment_history_even_with_no_map_update() {
    for (resilient, width) in [(false, 63), (true, 64)] {
        let mut d = decoder_with_segment_history();
        d.decode(&segment_packet(true, resilient, width, true, false))
            .unwrap();
        assert!(d.segment_ids.is_empty());
        assert!(
            d.previous
                .as_ref()
                .unwrap()
                .modes
                .iter()
                .all(|m| m.segment_id == 0)
        );
    }
    let mut d = decoder_with_segment_history();
    d.decode(&KEY).unwrap();
    assert!(d.segment_ids.is_empty());
}

#[test]
fn segment_reference_and_skip_features_replace_their_entropy_syntax() {
    let mut d = decoder_with_segment_history();
    d.carried.features[3][2] = Some(3);
    let packet = segment_packet(true, false, 64, true, false);
    let parsed = header::parse(&packet, &d.slots, &d.carried, &d.contexts).unwrap();
    let Header::Frame {
        header,
        mut compressed,
        ..
    } = parsed.header
    else {
        panic!()
    };
    compressed.reference_mode = crate::ReferenceMode::Compound;
    let (_, probs) = *parsed.probs.unwrap();
    let (_, tiles) = decode::<u8>(
        &[0],
        &header,
        &probs,
        compressed,
        TileContext {
            references: &d.references,
            previous: None,
            previous_segments: &d.segment_ids,
            reference_mode: crate::ReferenceMode::Compound,
        },
    )
    .unwrap();
    assert!(
        tiles
            .modes
            .iter()
            .all(|m| m.ref_frames == [3, -1] && m.mode == 12 && m.skip)
    );
    assert!(tiles.counts.skip.iter().flatten().all(|&v| v == 0));
    assert!(tiles.counts.is_inter.iter().flatten().all(|&v| v == 0));
    assert!(tiles.counts.comp_mode.iter().flatten().all(|&v| v == 0));
    assert!(tiles.counts.comp_ref.iter().flatten().all(|&v| v == 0));
    assert!(
        tiles
            .counts
            .single_ref
            .iter()
            .flatten()
            .flatten()
            .all(|&v| v == 0)
    );
    assert!(
        tiles
            .counts
            .inter_mode
            .iter()
            .flatten()
            .flatten()
            .all(|&v| v == 0)
    );
}
