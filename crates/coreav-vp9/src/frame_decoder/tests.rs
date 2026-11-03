#![allow(clippy::indexing_slicing)]

use super::*;
use crate::Samples;

const KEY: [u8; 15] = [
    0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00, 0x01, 0x00,
];

fn key_allocation_bytes() -> u64 {
    (core::mem::size_of::<&[u8]>()
        + core::mem::size_of::<Picture>()
        + 64 * 64
        + 2 * 32 * 32
        + 48 * core::mem::size_of::<ModeInfo>()
        + 8
        + 8
        + 3 * 16
        + 1024
        + 1024 * 8
        + 2 * (64 * 48 + 2 * 32 * 24)) as u64
}

#[test]
fn the_frame_byte_allowance_precedes_reconstruction_and_checks_shown_references() {
    let mut decoder = FrameDecoder::new(Limits::default());
    assert_eq!(
        decoder.decode_with_budget(&KEY, true, 1024, 4607),
        Err(Error::Limit(LimitKind::FrameBytes))
    );
    assert_eq!(decoder.retained_memory_bytes().unwrap(), 0);
    assert_eq!(decoder.decode(&KEY).unwrap().len(), 1);
    assert_eq!(
        decoder.decode_with_budget(&[0x88], true, 8 << 30, 4607),
        Err(Error::Limit(LimitKind::FrameBytes))
    );
    assert_eq!(decoder.decode(&[0x88]).unwrap().len(), 1);
}

#[test]
fn keyframe_only_skips_invalidate_exactly_the_refreshed_slots() {
    for flags in 0u8..=255 {
        let mut full = FrameDecoder::new(Limits::default());
        let expected = full.decode(&KEY).unwrap();
        let mut keys = FrameDecoder::new(Limits::default()).with_keyframes_only();
        assert_eq!(keys.decode(&KEY).unwrap(), expected);
        let skipped = [0x86, flags >> 2, flags << 6];
        assert!(keys.decode(&skipped).unwrap().is_empty());
        for slot in 0..8 {
            let shown = [0x88 | slot];
            let output = keys.decode(&shown).unwrap();
            if flags >> slot & 1 == 1 {
                assert!(output.is_empty());
            } else {
                assert_eq!(output, expected);
            }
        }
    }
}

#[test]
fn keyframe_only_refuses_truncated_skip_prefixes_and_nonzero_profiles() {
    let mut keys = FrameDecoder::new(Limits::default()).with_keyframes_only();
    keys.decode(&KEY).unwrap();
    for packet in [&[0x86][..], &[0x86, 0][..], &[0x80][..], &[0x98][..]] {
        assert!(keys.decode(packet).is_err());
    }
    assert_eq!(keys.decode(&[0x88]).unwrap().len(), 1);
    let mut empty = FrameDecoder::new(Limits::default()).with_keyframes_only();
    assert!(empty.decode(&[0x88]).unwrap().is_empty());
}

#[test]
fn discarded_frames_keep_references_without_allocating_output_sample_copies() {
    let budget = key_allocation_bytes() - (64 * 48 + 2 * 32 * 24);
    let mut d = FrameDecoder::new(Limits::default());
    assert!(
        d.decode_with_budget(&KEY, false, budget, u64::MAX)
            .unwrap()
            .is_empty()
    );
    assert_eq!(d.decode(&[0x88]).unwrap().len(), 1);
    let mut d = FrameDecoder::new(Limits::default());
    assert_eq!(
        d.decode_with_budget(&KEY, true, budget, u64::MAX),
        Err(Error::Limit(LimitKind::MemoryBytes))
    );
}

#[test]
fn the_budget_includes_working_planes_history_and_the_output_copy() {
    let needed = key_allocation_bytes();
    let mut d = FrameDecoder::new(Limits {
        max_memory_bytes: needed - 1,
        ..Limits::default()
    });
    assert_eq!(d.decode(&KEY), Err(Error::Limit(LimitKind::MemoryBytes)));
    assert!(d.previous.is_none());
    assert!(d.references.iter().all(Option::is_none));
    assert!(d.slots.iter().all(Option::is_none));
    d.limits.max_memory_bytes = needed;
    assert_eq!(d.decode(&KEY).unwrap().len(), 1);
    let references = d.references.clone();
    let contexts = d.contexts.clone();
    let history = d.previous.as_ref().unwrap().modes.as_ptr();
    assert_eq!(d.decode(&KEY), Err(Error::Limit(LimitKind::MemoryBytes)));
    assert_eq!(d.references, references);
    assert_eq!(d.contexts, contexts);
    assert_eq!(d.previous.as_ref().unwrap().modes.as_ptr(), history);
    d.limits.max_memory_bytes =
        needed + (64 * 48 + 2 * 32 * 24 + 48 * core::mem::size_of::<ModeInfo>()) as u64;
    assert!(d.decode(&KEY).is_ok());
}

#[test]
fn eight_distinct_reference_capacities_are_charged_before_an_output() {
    let mut d = FrameDecoder::new(Limits::default());
    d.decode(&KEY).unwrap();
    for slot in 0..8 {
        let mut packet = segment_packet(true, false, 64, false, false);
        let flags = 1u8 << slot;
        packet[1] |= flags >> 2;
        packet[2] |= flags << 6;
        d.decode(&packet).unwrap();
    }
    assert_eq!(d.references.iter().flatten().count(), 8);
    let mut retained = Memory::new(u64::MAX);
    for picture in d.references.iter().flatten() {
        picture.charge(&mut retained).unwrap();
    }
    let picture_bytes = 64 * 48 + 2 * 32 * 24;
    let history_bytes = 48 * core::mem::size_of::<ModeInfo>();
    d.limits.max_memory_bytes = (8 * picture_bytes + history_bytes - 1) as u64;
    assert_eq!(d.decode(&[0x88]), Err(Error::Limit(LimitKind::MemoryBytes)));
    d.limits.max_memory_bytes = (8 * picture_bytes
        + history_bytes
        + core::mem::size_of::<&[u8]>()
        + core::mem::size_of::<Picture>()
        + picture_bytes) as u64;
    assert!(d.decode(&[0x88]).is_ok());
}

#[test]
fn superframe_output_copies_share_one_packet_budget() {
    let mut d = FrameDecoder::new(Limits::default());
    d.decode(&KEY).unwrap();
    let packet = [
        0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0xc7, 1, 1, 1, 1, 1, 1, 1, 1, 0xc7,
    ];
    let bytes = 64 * 48 + 2 * 32 * 24;
    let needed = (bytes
        + 48 * core::mem::size_of::<ModeInfo>()
        + 8 * (core::mem::size_of::<&[u8]>() + core::mem::size_of::<Picture>() + bytes))
        as u64;
    d.limits.max_memory_bytes = needed - 1;
    assert_eq!(d.decode(&packet), Err(Error::Limit(LimitKind::MemoryBytes)));
    d.limits.max_memory_bytes = needed;
    assert_eq!(d.decode(&packet).unwrap().len(), 8);
}

#[test]
fn memory_failure_in_a_later_member_keeps_the_earlier_frame_committed() {
    let mut packet = KEY.to_vec();
    packet.extend_from_slice(&KEY);
    packet.extend_from_slice(&[0xc1, 15, 15, 0xc1]);
    let first_member = key_allocation_bytes()
        + (core::mem::size_of::<&[u8]>() + core::mem::size_of::<Picture>()) as u64;
    let mut d = FrameDecoder::new(Limits {
        max_memory_bytes: first_member,
        ..Limits::default()
    });
    assert_eq!(d.decode(&packet), Err(Error::Limit(LimitKind::MemoryBytes)));
    assert_eq!(d.references.iter().flatten().count(), 1);
    assert!(d.previous.as_ref().unwrap().key);
    d.limits.max_memory_bytes = Limits::default().max_memory_bytes;
    assert_eq!(d.decode(&[0x88]).unwrap().len(), 1);
}

#[test]
fn resolution_changes_keep_old_reference_and_history_charges_until_commit() {
    let mut d = decoder_with_segment_history();
    let packet = segment_packet(true, false, 80, true, true);
    let references = d.references.clone();
    let slots = d.reference_slots;
    let modes = d.previous.as_ref().unwrap().modes.as_ptr();
    let segments = d.segment_ids.clone();
    d.limits.max_memory_bytes = key_allocation_bytes();
    assert_eq!(d.decode(&packet), Err(Error::Limit(LimitKind::MemoryBytes)));
    assert_eq!(d.references, references);
    assert_eq!(d.reference_slots, slots);
    assert_eq!(d.segment_ids, segments);
    assert_eq!(d.previous.as_ref().unwrap().modes.as_ptr(), modes);
    d.limits.max_memory_bytes = Limits::default().max_memory_bytes;
    assert!(d.decode(&packet).is_ok());
}

#[test]
fn refreshed_slots_share_private_pictures_and_outputs_do_not_alias_them() {
    let mut d = FrameDecoder::new(Limits::default());
    let expected = d.decode(&KEY).unwrap().pop().unwrap();
    assert_eq!(d.references.iter().flatten().count(), 1);
    assert!(
        d.reference_slots
            .iter()
            .all(|slot| *slot == d.reference_slots[0])
    );
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
    let stored = d.references[0].as_ref().unwrap().clone();
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
        assert_eq!(a, b);
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
    let (_, probs) = parsed.probs.unwrap();
    let (_, tiles) = decode::<u8>(
        &[0],
        &header,
        &probs,
        compressed,
        TileContext {
            references: &core::array::from_fn::<_, 8, _>(|i| {
                d.reference_slots[i].and_then(|index| d.references[index].as_ref())
            }),
            previous: None,
            previous_segments: &d.segment_ids,
            reference_mode: crate::ReferenceMode::Compound,
        },
        &mut Memory::new(u64::MAX),
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
