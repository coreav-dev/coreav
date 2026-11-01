#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

/// Writes bits the way `f(n)` reads them, most significant first.
#[derive(Default)]
struct W(Vec<bool>);

impl W {
    fn f(&mut self, n: u32, value: u32) -> &mut Self {
        for i in (0..n).rev() {
            self.0.push(value >> i & 1 == 1);
        }
        self
    }

    fn s(&mut self, n: u32, value: i32) -> &mut Self {
        self.f(n, value.unsigned_abs()).f(1, u32::from(value < 0))
    }

    /// The bytes, padded with zero bits.
    fn bytes(&self) -> Vec<u8> {
        self.0
            .chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |b, (i, &bit)| b | u8::from(bit) << (7 - i))
            })
            .collect()
    }

    /// Everything after `frame_context_idx` with every value 0 or off, for
    /// a frame 64 samples wide, then a compressed header of one byte.
    fn plain_tail(&mut self) -> &mut Self {
        self.tail(1)
    }

    /// As [`W::plain_tail`], with `tile_bits` bits of `tile_info()`: one
    /// for the tile rows, and one more where the width allows several tile
    /// columns.
    fn tail(&mut self, tile_bits: u32) -> &mut Self {
        self.f(6, 0).f(3, 0).f(1, 0); // loop filter: level, sharpness, no deltas
        self.f(8, 60).f(3, 0); // base_q_idx and no delta
        self.f(1, 0); // no segmentation
        self.f(tile_bits, 0); // one tile column and one tile row
        self.f(16, 1).compressed_byte()
    }

    /// The trailing bits, then a compressed header of one 0 byte.
    fn compressed_byte(&mut self) -> &mut Self {
        while self.0.len() % 8 != 0 {
            self.0.push(false);
        }
        self.f(8, 0)
    }
}

fn key_frame(width: u32, height: u32) -> W {
    let mut w = W::default();
    w.f(2, 2).f(1, 0).f(1, 0).f(1, 0); // marker, profile 0, not show-existing
    w.f(1, 0).f(1, 1).f(1, 0); // key frame, shown, not error resilient
    w.f(24, 0x49_83_42).f(3, 2).f(1, 0); // sync code, BT.709, studio range
    w.f(16, width - 1).f(16, height - 1).f(1, 0);
    w.f(1, 1).f(1, 0).f(2, 3); // refresh_frame_context, not parallel, context 3
    w
}

fn parse_bytes(bytes: &[u8], slots: &[Option<Slot>; 8], carried: &Carried) -> Result<Header> {
    parse(bytes, slots, carried, &Contexts::default()).map(|p| p.header)
}

fn frame(header: Result<Header>) -> FrameHeader {
    match header.unwrap() {
        Header::Frame { header, .. } => *header,
        other => panic!("not a frame: {other:?}"),
    }
}

fn after_key(width: u32, height: u32) -> ([Option<Slot>; 8], Carried) {
    let key = frame(parse_bytes(
        &key_frame(width, height).plain_tail().bytes(),
        &[None; 8],
        &Carried::default(),
    ));
    ([Some(key.slot()); 8], key.carried())
}

#[test]
fn a_key_frame_states_its_size_and_format_and_refreshes_every_slot() {
    let h = frame(parse_bytes(
        &key_frame(64, 48).plain_tail().bytes(),
        &[None; 8],
        &Carried::default(),
    ));
    assert!(h.key_frame && h.show_frame && h.is_intra());
    assert_eq!((h.width, h.height, h.render_size), (64, 48, (64, 48)));
    assert_eq!(h.color.color_space, ColorSpace::Bt709);
    assert_eq!(
        (
            h.color.bit_depth,
            h.color.subsampling_x,
            h.color.subsampling_y
        ),
        (8, true, true)
    );
    assert_eq!(h.refresh_frame_flags, 0xFF);
    assert_eq!((h.frame_context_idx, h.coded_frame_context_idx), (0, 3));
    assert_eq!(h.quantization.base_q_idx, 60);
    assert_eq!(h.compressed_header_size, 1);
    assert!(h.trailing_bits_zero);
    assert_eq!(h.uncompressed_size, 14);
}

#[test]
fn an_inter_frame_reads_its_render_size_before_its_motion_vector_precision() {
    let (slots, carried) = after_key(64, 48);
    let mut w = W::default();
    w.f(2, 2).f(2, 0).f(1, 0).f(1, 1).f(1, 1).f(1, 0); // inter, shown, not error resilient
    w.f(2, 0).f(8, 0b10); // reset_frame_context, refresh slot 1
    for i in 0..3 {
        w.f(3, i).f(1, u32::from(i == 2)); // slots 0, 1, 2; altref's sign bias 1
    }
    w.f(1, 1); // found_ref: the size of slot 0
    w.f(1, 1).f(16, 31).f(16, 23); // render size 32x24
    w.f(1, 1).f(1, 1); // allow_high_precision_mv, switchable filter
    w.f(1, 0).f(1, 0).f(2, 2); // context 2
    w.plain_tail();
    let h = frame(parse_bytes(&w.bytes(), &slots, &carried));
    assert!(!h.is_intra());
    assert_eq!((h.width, h.height, h.render_size), (64, 48, (32, 24)));
    assert!(h.allow_high_precision_mv);
    assert_eq!(h.interpolation_filter, InterpolationFilter::Switchable);
    assert_eq!(h.ref_frame_idx, [0, 1, 2]);
    assert_eq!(h.ref_frame_sign_bias, [false, false, false, true]);
    assert_eq!(h.frame_context_idx, 2);
    assert_eq!(h.color, carried.color.unwrap());
}

#[test]
fn a_frame_size_outside_every_reference_ratio_is_refused() {
    let (slots, carried) = after_key(64, 48);
    let mut w = W::default();
    w.f(2, 2)
        .f(2, 0)
        .f(1, 0)
        .f(1, 1)
        .f(1, 1)
        .f(1, 0)
        .f(2, 0)
        .f(8, 0);
    for i in 0..3 {
        w.f(3, i).f(1, 0);
    }
    w.f(3, 0).f(16, 1023).f(16, 767); // 1024x768 is 16 times 64x48
    w.f(1, 0).f(1, 0).f(1, 0).f(2, 0).f(1, 0).f(1, 0).f(2, 0);
    w.tail(2); // 1024 samples wide allows several tile columns
    assert!(matches!(
        parse_bytes(&w.bytes(), &slots, &carried),
        Ok(Header::Frame { .. })
    ));
    let mut w = W::default();
    w.f(2, 2)
        .f(2, 0)
        .f(1, 0)
        .f(1, 1)
        .f(1, 1)
        .f(1, 0)
        .f(2, 0)
        .f(8, 0);
    for i in 0..3 {
        w.f(3, i).f(1, 0);
    }
    w.f(3, 0).f(16, 1024).f(16, 767);
    w.f(1, 0).f(1, 0).f(1, 0).f(2, 0).f(1, 0).f(1, 0).f(2, 0);
    w.plain_tail();
    assert_eq!(
        parse_bytes(&w.bytes(), &slots, &carried),
        Err(Error::Invalid(
            "no reference frame within the scaling ratios"
        ))
    );
}

#[test]
fn an_inter_frame_needs_its_reference_slots() {
    let mut w = W::default();
    w.f(2, 2)
        .f(2, 0)
        .f(1, 0)
        .f(1, 1)
        .f(1, 1)
        .f(1, 0)
        .f(2, 0)
        .f(8, 0);
    for i in 0..3 {
        w.f(3, i).f(1, 0);
    }
    let carried = Carried {
        color: Some(after_key(64, 48).1.color.unwrap()),
        ..Carried::default()
    };
    assert_eq!(
        parse_bytes(&w.bytes(), &[None; 8], &carried),
        Err(Error::Invalid(
            "ref_frame_idx names an empty reference slot"
        ))
    );
}

#[test]
fn show_existing_frame_shows_a_slot_of_its_size() {
    let (mut slots, carried) = after_key(64, 48);
    let mut w = W::default();
    w.f(2, 2).f(2, 0).f(1, 1).f(3, 5);
    assert_eq!(
        parse_bytes(&w.bytes(), &slots, &carried),
        Ok(Header::ShowExisting {
            profile: 0,
            slot: 5,
            width: 64,
            height: 48
        })
    );
    slots[5] = None;
    assert_eq!(
        parse_bytes(&w.bytes(), &slots, &carried),
        Err(Error::Invalid(
            "show_existing_frame of an empty reference slot"
        ))
    );
}

#[test]
fn loop_filter_deltas_carry_over_until_an_intra_frame_resets_them() {
    let (slots, carried) = after_key(64, 48);
    let mut w = W::default();
    w.f(2, 2)
        .f(2, 0)
        .f(1, 0)
        .f(1, 1)
        .f(1, 1)
        .f(1, 0)
        .f(2, 0)
        .f(8, 0);
    for i in 0..3 {
        w.f(3, i).f(1, 0);
    }
    w.f(1, 1)
        .f(1, 0)
        .f(1, 0)
        .f(1, 0)
        .f(2, 0)
        .f(1, 0)
        .f(1, 0)
        .f(2, 0);
    w.f(6, 10).f(3, 0).f(1, 1).f(1, 1); // level 10, deltas enabled and updated
    w.f(1, 0).f(1, 1).s(6, -7).f(1, 0).f(1, 0); // last frame's delta -7
    w.f(1, 1).s(6, 3).f(1, 0); // mode delta 0 is 3
    w.f(8, 0).f(3, 0).f(1, 0).f(1, 0).f(16, 1).compressed_byte();
    let h = frame(parse_bytes(&w.bytes(), &slots, &carried));
    assert_eq!(h.loop_filter.ref_deltas, [1, -7, -1, -1]);
    assert_eq!(h.loop_filter.mode_deltas, [3, 0]);
    let kept = h.carried();
    let mut w = W::default();
    w.f(2, 2)
        .f(2, 0)
        .f(1, 0)
        .f(1, 1)
        .f(1, 1)
        .f(1, 0)
        .f(2, 0)
        .f(8, 0);
    for i in 0..3 {
        w.f(3, i).f(1, 0);
    }
    w.f(1, 1)
        .f(1, 0)
        .f(1, 0)
        .f(1, 0)
        .f(2, 0)
        .f(1, 0)
        .f(1, 0)
        .f(2, 0);
    w.f(6, 10).f(3, 0).f(1, 1).f(1, 0); // deltas enabled, not updated
    w.f(8, 0).f(3, 0).f(1, 0).f(1, 0).f(16, 1).compressed_byte();
    let h = frame(parse_bytes(&w.bytes(), &slots, &kept));
    assert_eq!(h.loop_filter.ref_deltas, [1, -7, -1, -1]);
    let h = frame(parse_bytes(
        &key_frame(64, 48).plain_tail().bytes(),
        &slots,
        &kept,
    ));
    assert_eq!(h.loop_filter.ref_deltas, [1, 0, -1, -1]);
    assert_eq!(h.loop_filter.mode_deltas, [0, 0]);
}

#[test]
fn srgb_is_refused_in_profile_0() {
    let mut w = W::default();
    w.f(2, 2)
        .f(2, 0)
        .f(1, 0)
        .f(1, 0)
        .f(1, 1)
        .f(1, 0)
        .f(24, 0x49_83_42)
        .f(3, 7);
    assert_eq!(
        parse_bytes(&w.bytes(), &[None; 8], &Carried::default()),
        Err(Error::Invalid("sRGB in profile 0 or 2"))
    );
}

#[test]
fn a_truncated_header_is_refused_as_truncated() {
    let bytes = key_frame(64, 48).plain_tail().bytes();
    for len in 0..bytes.len() - 2 {
        assert!(
            matches!(
                parse_bytes(&bytes[..len], &[None; 8], &Carried::default()),
                Err(Error::Truncated(_))
            ),
            "{len}"
        );
    }
}

/// The saved contexts after the headers of `bytes`, each with its first
/// skip probability, starting from contexts whose skip probabilities are 1,
/// 2, 3 and 4.
fn contexts_after(bytes: &[u8], slots: &[Option<Slot>; 8], carried: &Carried) -> [u8; 4] {
    let mut contexts = Contexts::default();
    for ctx in 0..4u8 {
        let mut marked = Probs::DEFAULT;
        marked.skip[0] = ctx + 1;
        contexts.save(ctx, marked);
    }
    let parsed = parse(bytes, slots, carried, &contexts).unwrap();
    let (after, _) = *parsed.probs.unwrap();
    [0, 1, 2, 3].map(|ctx| after.load(ctx).skip[0])
}

#[test]
fn intra_and_error_resilient_frames_reset_the_contexts_clause_7_2_names() {
    let (slots, carried) = after_key(64, 48);
    let default = Probs::DEFAULT.skip[0];
    let key = key_frame(64, 48).plain_tail().bytes();
    assert_eq!(contexts_after(&key, &slots, &carried), [default; 4]);
    let intra_only = |reset: u32| {
        let mut w = W::default();
        w.f(2, 2).f(2, 0).f(1, 0); // marker, profile 0, not show-existing
        w.f(1, 1).f(1, 0).f(1, 0); // not a key frame, hidden, not error resilient
        w.f(1, 1).f(2, reset); // intra_only, reset_frame_context
        w.f(24, 0x49_83_42).f(8, 0x01); // sync code, refresh slot 0
        w.f(16, 63).f(16, 47).f(1, 0); // 64x48
        w.f(1, 1).f(1, 0).f(2, 2); // refresh_frame_context, context 2 coded
        w.plain_tail();
        w.bytes()
    };
    assert_eq!(
        contexts_after(&intra_only(0), &slots, &carried),
        [1, 2, 3, 4]
    );
    assert_eq!(
        contexts_after(&intra_only(2), &slots, &carried),
        [1, 2, default, 4]
    );
    assert_eq!(
        contexts_after(&intra_only(3), &slots, &carried),
        [default; 4]
    );
    let mut inter = W::default();
    inter.f(2, 2).f(2, 0).f(1, 0).f(1, 1).f(1, 1).f(1, 0); // inter, shown, not error resilient
    inter.f(2, 3).f(8, 0); // reset_frame_context 3, which an inter frame ignores
    for i in 0..3 {
        inter.f(3, i).f(1, 0);
    }
    inter.f(1, 1).f(1, 0).f(1, 0).f(1, 0).f(2, 0); // size of slot 0, no hp, filter 0
    inter.f(1, 0).f(1, 0).f(2, 1); // context 1
    inter.plain_tail();
    assert_eq!(
        contexts_after(&inter.bytes(), &slots, &carried),
        [1, 2, 3, 4]
    );
}
