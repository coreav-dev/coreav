//! End-to-end decoding of streams the test builds itself, so that the
//! decoder is exercised from NAL units to pictures without the conformance
//! suite (which `conformance.rs` needs).

// Test code: the lints that keep production code panic-free are relaxed
// here, as clippy.toml does for unit tests.
#![allow(clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

use coreav_h264::{Decoder, Error, LimitKind, Limits, Mode, annex_b};

#[path = "common/streams.rs"]
mod streams;
use streams::*;

/// A baseline sequence parameter set of `mb_w` by `mb_h` macroblocks.
fn sps(mb_w: u32, mb_h: u32, crop: Option<[u32; 4]>) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, 66).u(8, 0xc0).u(8, 10);
    w.ue(0) // seq_parameter_set_id
        .ue(0) // log2_max_frame_num_minus4
        .ue(0) // pic_order_cnt_type
        .ue(2) // log2_max_pic_order_cnt_lsb_minus4
        .ue(1) // max_num_ref_frames
        .flag(false) // gaps_in_frame_num_value_allowed_flag
        .ue(mb_w - 1)
        .ue(mb_h - 1)
        .flag(true) // frame_mbs_only_flag
        .flag(true); // direct_8x8_inference_flag
    match crop {
        Some([l, r, t, b]) => {
            w.flag(true).ue(l).ue(r).ue(t).ue(b);
        }
        None => {
            w.flag(false);
        }
    }
    w.flag(false); // vui_parameters_present_flag
    w.finish()
}

/// A B slice whose every macroblock is skipped, so each one takes direct
/// prediction (clause 8.4.1.2) and reads the co-located macroblock's
/// motion from the first picture of list 1.
fn skipped_b_slice(
    frame_num: u32,
    poc_lsb: u32,
    macroblocks: usize,
    reference: bool,
    back: u32,
) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(6) // slice_type: B, and every slice of the picture is B
        .ue(0) // pic_parameter_set_id
        .u(4, frame_num)
        .u(6, poc_lsb)
        .flag(true) // direct_spatial_mv_pred_flag
        .flag(false); // num_ref_idx_active_override_flag
    // Both lists are reordered to put the picture `back` ahead of the
    // current one at index 0. A picture invented for a frame_num gap is
    // left out of the initial lists where pic_order_cnt_type is 0
    // (clause 8.2.4.2), so naming it by picNum is the way to reach it.
    for _ in 0..2 {
        w.flag(true) // ref_pic_list_modification_flag_lX
            .ue(0) // modification_of_pic_nums_idc: subtract from picNum
            .ue(back - 1) // abs_diff_pic_num_minus1
            .ue(3); // idc 3: the list is as it should be
    }
    if reference {
        w.flag(false); // adaptive_ref_pic_marking_mode_flag
    }
    w.se(0) // slice_qp_delta
        .ue(macroblocks as u32); // mb_skip_run
    w.finish()
}

#[test]
fn the_macroblocks_a_picture_leaves_uncoded_are_grey_not_the_last_picture() {
    // Decoding no longer clears a picture before it writes one, so a
    // picture whose slices do not cover every macroblock has to be filled
    // where they stopped. Without that fill it would show whatever the
    // pooled buffer held.
    //
    // The test only means anything if the last picture is decoded into a
    // buffer an earlier one wrote, and the decoder reuses a picture only
    // when nothing else holds it (`Arc::strong_count == 1`). So the three
    // full pictures run first to fill the pool, and every picture is
    // dropped as it comes out rather than collected: holding them keeps
    // every pool entry at a count above 1 and the decoder allocates a
    // fresh, already grey picture each time, which passes whether or not
    // the fill happens. Checked by stubbing `Picture::fill_macroblock`:
    // this fails at luma (16, 0) without it.
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    for _ in 0..3 {
        stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    }
    // One macroblock of the four, so three are left uncoded.
    stream.extend(nal(3, 5, idr_pcm_slice(1, pcm_samples)));

    let mut decoder = Decoder::new(Limits::default());
    let mut out = Vec::new();
    for unit in annex_b(&stream) {
        decoder.decode_nal(unit).expect("the stream decodes");
        // Dropped, not kept: a picture the caller still holds cannot be
        // reused, and the point of the test is to decode into a buffer
        // that an earlier picture wrote.
        drop(decoder.take_pictures());
    }
    decoder.flush().expect("flush");
    out.extend(decoder.take_pictures());

    // The last two pictures are still held for output when the stream
    // ends; the partly coded one is the last of them.
    assert_eq!(out.len(), 2);
    let last = out.last().expect("the partly coded picture");
    // Without reuse this test proves nothing: a freshly allocated picture
    // is already grey and would pass whether or not the fill runs.
    assert!(
        decoder.pooled_pictures() > 0,
        "no picture was reused, so the fill is not what this test measured"
    );
    assert_eq!((last.width(), last.height()), (32, 32));
    let luma: Vec<Vec<u8>> = last.luma().rows().map(<[u8]>::to_vec).collect();
    let (cb, cr) = last.chroma();
    let blue: Vec<Vec<u8>> = cb.rows().map(<[u8]>::to_vec).collect();
    let red: Vec<Vec<u8>> = cr.rows().map(<[u8]>::to_vec).collect();

    let coded = pcm_samples(0);
    for (y, row) in luma.iter().enumerate() {
        for (x, &sample) in row.iter().enumerate() {
            let want = if x < 16 && y < 16 {
                // The one macroblock the slice coded keeps its samples.
                coded.get(y * 16 + x).copied().unwrap_or(0)
            } else {
                0
            };
            assert_eq!(sample, want, "luma at ({x}, {y})");
        }
    }
    for (y, (brow, rrow)) in blue.iter().zip(red.iter()).enumerate() {
        for (x, (&b, &r)) in brow.iter().zip(rrow.iter()).enumerate() {
            let (wb, wr) = if x < 8 && y < 8 {
                (
                    coded.get(256 + y * 8 + x).copied().unwrap_or(128),
                    coded.get(320 + y * 8 + x).copied().unwrap_or(128),
                )
            } else {
                (128, 128)
            };
            assert_eq!((b, r), (wb, wr), "chroma at ({x}, {y})");
        }
    }
}

#[test]
fn decodes_a_picture_of_i_pcm_macroblocks() {
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));

    let mut decoder = Decoder::new(Limits::default());
    for unit in annex_b(&stream) {
        decoder.decode_nal(unit).expect("the stream decodes");
    }
    decoder.flush().expect("flush");
    let pictures = decoder.take_pictures();
    assert_eq!(pictures.len(), 1);
    let picture = pictures.first().expect("one picture");
    assert_eq!((picture.width(), picture.height()), (32, 32));
    assert!(decoder.warnings().is_empty());

    // Every sample is the one the stream carried: macroblock 1 is the
    // second 16 columns of the first 16 rows.
    let luma: Vec<&[u8]> = picture.luma().rows().collect();
    let want = pcm_samples(1);
    for y in 0..16 {
        let row = luma.get(y).expect("row");
        let got = &row[16..32];
        assert_eq!(got, &want[y * 16..y * 16 + 16], "row {y}");
    }
    let (cb, cr) = picture.chroma();
    assert_eq!((cb.width(), cb.height()), (16, 16));
    assert!(cb.rows().next().expect("row")[..8].iter().all(|&s| s == 40));
    assert!(
        cr.rows().next().expect("row")[..8]
            .iter()
            .all(|&s| s == 200)
    );
}

#[test]
fn cropping_shrinks_the_picture() {
    let mut stream = Vec::new();
    // 4:2:0 frame cropping counts in chroma samples: CropUnitX and
    // CropUnitY are both 2, so this crops 2 columns and 4 rows.
    stream.extend(nal(3, 7, sps(2, 2, Some([1, 0, 0, 2]))));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    let mut decoder = Decoder::new(Limits::default());
    for unit in annex_b(&stream) {
        decoder.decode_nal(unit).expect("the stream decodes");
    }
    decoder.flush().expect("flush");
    let pictures = decoder.take_pictures();
    let picture = pictures.first().expect("one picture");
    assert_eq!((picture.width(), picture.height()), (30, 28));
    let (cb, _) = picture.chroma();
    assert_eq!((cb.width(), cb.height()), (15, 14));
    // The first sample is the third of the first macroblock's first row.
    let first = picture
        .luma()
        .rows()
        .next()
        .and_then(|r| r.first())
        .copied();
    assert_eq!(first, pcm_samples(0).get(2).copied());
}

#[test]
fn a_picture_larger_than_the_limits_is_refused() {
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    let limits = Limits::for_size(16, 16);
    let mut decoder = Decoder::new(limits);
    let mut last = Ok(());
    for unit in annex_b(&stream) {
        last = decoder.decode_nal(unit);
    }
    assert!(matches!(last, Err(Error::Limit(_))), "{last:?}");
    decoder.flush().expect("flush");
    assert!(decoder.take_pictures().is_empty());
}

#[test]
fn streams_outside_the_scope_are_refused_rather_than_decoded() {
    // A High 4:2:2 sequence parameter set: profile_idc 122, so the chroma
    // format is coded, and it is 4:2:2, which this decoder does not decode.
    let mut w = Writer::default();
    w.u(8, 122).u(8, 0).u(8, 40);
    w.ue(0) // seq_parameter_set_id
        .ue(2) // chroma_format_idc: 4:2:2
        .ue(0) // bit_depth_luma_minus8
        .ue(0) // bit_depth_chroma_minus8
        .flag(false) // qpprime_y_zero_transform_bypass_flag
        .flag(false) // seq_scaling_matrix_present_flag
        .ue(0)
        .ue(0)
        .ue(2)
        .ue(1)
        .flag(false)
        .ue(1)
        .ue(1)
        .flag(true)
        .flag(true)
        .flag(false)
        .flag(false);
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, w.finish()));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(1, pcm_samples)));
    let mut decoder = Decoder::new(Limits::default());
    let mut results = Vec::new();
    for unit in annex_b(&stream) {
        results.push(decoder.decode_nal(unit));
    }
    assert!(
        matches!(results.last(), Some(Err(Error::Unsupported(_)))),
        "{results:?}"
    );
}

#[test]
fn hostile_input_gives_errors_and_never_panics() {
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    // Every one-byte change of the stream, and every truncation of it, is
    // either decoded or refused; none may panic or hang.
    let mut state = 12345u32;
    for _ in 0..2000 {
        let mut broken = stream.clone();
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let at = (state >> 8) as usize % broken.len();
        let value = (state >> 3) as u8;
        if let Some(byte) = broken.get_mut(at) {
            *byte ^= value | 1;
        }
        if state % 3 == 0 {
            broken.truncate(at.max(1));
        }
        let mut decoder = Decoder::new(Limits::default());
        for unit in annex_b(&broken) {
            let _ = decoder.decode_nal(unit);
        }
        decoder.flush().expect("flush");
        let _ = decoder.take_pictures();
    }
}

/// A sequence parameter set with a video usability information that says
/// the stream does not reorder and holds one frame.
fn sps_without_reordering(mb_w: u32, mb_h: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, 66).u(8, 0xc0).u(8, 10);
    w.ue(0)
        .ue(0)
        .ue(0)
        .ue(2)
        .ue(1)
        .flag(false)
        .ue(mb_w - 1)
        .ue(mb_h - 1)
        .flag(true)
        .flag(true)
        .flag(false) // frame_cropping_flag
        .flag(true); // vui_parameters_present_flag
    w.flag(false) // aspect_ratio_info_present_flag
        .flag(false) // overscan_info_present_flag
        .flag(false) // video_signal_type_present_flag
        .flag(false) // chroma_loc_info_present_flag
        .flag(false) // timing_info_present_flag
        .flag(false) // nal_hrd_parameters_present_flag
        .flag(false) // vcl_hrd_parameters_present_flag
        .u(1, 0) // pic_struct_present_flag
        .flag(true) // bitstream_restriction_flag
        .u(1, 1) // motion_vectors_over_pic_boundaries_flag
        .ue(0) // max_bytes_per_pic_denom
        .ue(0) // max_bits_per_mb_denom
        .ue(0) // log2_max_mv_length_horizontal
        .ue(0) // log2_max_mv_length_vertical
        .ue(0) // max_num_reorder_frames
        .ue(1); // max_dec_frame_buffering
    w.finish()
}

/// A coded slice of a picture that is not an IDR picture, with
/// `frame_num`, made of I_PCM macroblocks.
fn non_idr_pcm_slice(frame_num: u32, poc_lsb: u32, macroblocks: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(7) // slice_type: I
        .ue(0) // pic_parameter_set_id
        .u(4, frame_num)
        .u(6, poc_lsb)
        .flag(false) // adaptive_ref_pic_marking_mode_flag
        .se(0); // slice_qp_delta
    for addr in 0..macroblocks {
        w.ue(25);
        w.align_zero();
        w.bytes(&pcm_samples(addr));
    }
    w.finish()
}

#[test]
fn a_stream_that_does_not_reorder_gives_its_pictures_at_once() {
    fn feed(decoder: &mut Decoder, payload: Vec<u8>, kind: u8) {
        for unit in annex_b(&nal(3, kind, payload)) {
            decoder.decode_nal(unit).expect("the stream decodes");
        }
    }
    let mut decoder = Decoder::new(Limits::default());
    feed(&mut decoder, sps_without_reordering(2, 2), 7);
    feed(&mut decoder, pps(), 8);
    feed(&mut decoder, idr_pcm_slice(4, pcm_samples), 5);
    // The first picture is still being decoded.
    assert!(decoder.take_pictures().is_empty());
    feed(&mut decoder, non_idr_pcm_slice(1, 2, 4), 1);
    // The second picture ended the first one, which the stream says needs
    // no reordering, so it is out without waiting for the buffer to fill.
    assert_eq!(decoder.take_pictures().len(), 1);
    decoder.flush().expect("flush");
    assert_eq!(decoder.take_pictures().len(), 1);
}

#[test]
fn a_stream_that_needs_more_references_than_the_limit_is_refused() {
    let mut w = Writer::default();
    w.u(8, 66).u(8, 0xc0).u(8, 10);
    w.ue(0)
        .ue(0)
        .ue(0)
        .ue(2)
        .ue(8) // max_num_ref_frames
        .flag(false)
        .ue(1)
        .ue(1)
        .flag(true)
        .flag(true)
        .flag(false)
        .flag(false);
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, w.finish()));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(1, pcm_samples)));
    let limits = Limits {
        max_dpb_frames: 4,
        ..Limits::default()
    };
    let mut decoder = Decoder::new(limits);
    let mut last = Ok(());
    for unit in annex_b(&stream) {
        last = decoder.decode_nal(unit);
    }
    assert_eq!(last, Err(Error::Limit(LimitKind::DpbFrames)));
}

#[test]
fn parameter_sets_with_extreme_values_are_refused_not_wrapped() {
    // pic_init_qp_minus26 far outside its range.
    let mut w = Writer::default();
    w.ue(0).ue(0).flag(false).flag(false).ue(0).ue(0).ue(0);
    w.flag(false).u(2, 0).se(i32::MAX / 2);
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, w.finish()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    let mut decoder = Decoder::new(Limits::default());
    let mut results = Vec::new();
    for unit in annex_b(&stream) {
        results.push(decoder.decode_nal(unit));
    }
    assert!(results.iter().any(Result::is_err), "{results:?}");

    // A slice_qp_delta that would overflow the sum with pic_init_qp,
    // which a fuzz target found.
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(7) // slice_type
        .ue(0) // pic_parameter_set_id
        .u(4, 0)
        .ue(0) // idr_pic_id
        .u(6, 0)
        .flag(false)
        .flag(false)
        .se(i32::MAX); // slice_qp_delta
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, w.finish()));
    let mut decoder = Decoder::new(Limits::default());
    let mut results = Vec::new();
    for unit in annex_b(&stream) {
        results.push(decoder.decode_nal(unit));
    }
    assert!(
        matches!(results.last(), Some(Err(Error::Invalid(_)))),
        "{results:?}"
    );

    // A picture order count cycle whose offsets cannot be summed.
    let mut w = Writer::default();
    w.u(8, 66).u(8, 0xc0).u(8, 10);
    w.ue(0).ue(0).ue(1); // pic_order_cnt_type 1
    w.flag(false) // delta_pic_order_always_zero_flag
        .se(0) // offset_for_non_ref_pic
        .se(0) // offset_for_top_to_bottom_field
        .ue(4); // num_ref_frames_in_pic_order_cnt_cycle
    for _ in 0..4 {
        w.se(i32::MAX);
    }
    w.ue(1)
        .flag(false)
        .ue(1)
        .ue(1)
        .flag(true)
        .flag(true)
        .flag(false)
        .flag(false);
    let mut decoder = Decoder::new(Limits::default());
    let unit = nal(3, 7, w.finish());
    let result = annex_b(&unit)
        .map(|u| decoder.decode_nal(u))
        .last()
        .expect("one unit");
    assert!(result.is_err(), "{result:?}");
}

/// A P slice whose macroblocks are all skipped: it predicts the whole
/// picture from the one before, so it is not a keyframe.
fn skipped_p_slice(frame_num: u32, poc_lsb: u32, macroblocks: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(5) // slice_type: P, and every slice of the picture is P
        .ue(0) // pic_parameter_set_id
        .u(4, frame_num)
        .u(6, poc_lsb)
        .flag(false) // num_ref_idx_active_override_flag
        .flag(false) // ref_pic_list_modification_flag_l0
        .flag(false) // adaptive_ref_pic_marking_mode_flag
        .se(0) // slice_qp_delta
        .ue(macroblocks as u32); // mb_skip_run
    w.finish()
}

#[test]
fn the_keyframe_mode_returns_the_pictures_full_decoding_returns() {
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    stream.extend(nal(2, 1, skipped_p_slice(1, 2, 4)));
    stream.extend(nal(3, 1, non_idr_pcm_slice(2, 4, 4)));

    let decode = |mode| {
        let mut decoder = Decoder::new(Limits::default()).with_mode(mode);
        let mut out: Vec<Vec<u8>> = Vec::new();
        let mut take = |decoder: &mut Decoder| {
            for picture in decoder.take_pictures() {
                out.push(picture.luma().rows().flatten().copied().collect());
            }
        };
        for unit in annex_b(&stream) {
            decoder.decode_nal(unit).expect("the stream decodes");
            take(&mut decoder);
        }
        decoder.flush().expect("flush");
        take(&mut decoder);
        out
    };
    let every = decode(Mode::EveryPicture);
    let keyframes = decode(Mode::KeyframesOnly);
    // Three pictures in full, two of them keyframes.
    assert_eq!(every.len(), 3);
    assert_eq!(keyframes.len(), 2);
    // The keyframes are the same samples either way: the first picture
    // and the third.
    assert_eq!(keyframes.first(), every.first());
    assert_eq!(keyframes.get(1), every.get(2));
    // The skipped picture repeats the first one, which is why it is not a
    // keyframe.
    assert_eq!(every.get(1), every.first());
}

/// An IDR slice with control over `idr_pic_id`, the picture order count
/// and `no_output_of_prior_pics_flag`.
fn idr_slice(idr_pic_id: u32, poc_lsb: u32, no_output: bool, macroblocks: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(7) // slice_type: I
        .ue(0) // pic_parameter_set_id
        .u(4, 0) // frame_num
        .ue(idr_pic_id)
        .u(6, poc_lsb)
        .flag(no_output)
        .flag(false) // long_term_reference_flag
        .se(0);
    for addr in 0..macroblocks {
        w.ue(25);
        w.align_zero();
        w.bytes(&pcm_samples(addr));
    }
    w.finish()
}

/// A sequence parameter set that allows gaps in `frame_num`, with an
/// 8-bit `frame_num`.
fn sps_with_frame_num_gaps(mb_w: u32, mb_h: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, 66).u(8, 0xc0).u(8, 10);
    w.ue(0)
        .ue(4) // log2_max_frame_num_minus4: frame_num is 8 bits
        .ue(0)
        .ue(2)
        .ue(1)
        .flag(true) // gaps_in_frame_num_value_allowed_flag
        .ue(mb_w - 1)
        .ue(mb_h - 1)
        .flag(true)
        .flag(true)
        .flag(false)
        .flag(false);
    w.finish()
}

/// The slices of the streams with an 8-bit `frame_num`.
fn skipped_p_slice8(frame_num: u32, poc_lsb: u32, macroblocks: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0)
        .ue(5)
        .ue(0)
        .u(8, frame_num)
        .u(6, poc_lsb)
        .flag(false)
        .flag(false)
        .flag(false)
        .se(0)
        .ue(macroblocks as u32);
    w.finish()
}

fn idr_pcm_slice8(macroblocks: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0)
        .ue(7)
        .ue(0)
        .u(8, 0)
        .ue(0)
        .u(6, 0)
        .flag(false)
        .flag(false)
        .se(0);
    for addr in 0..macroblocks {
        w.ue(25);
        w.align_zero();
        w.bytes(&pcm_samples(addr));
    }
    w.finish()
}

fn non_idr_pcm_slice8(frame_num: u32, poc_lsb: u32, macroblocks: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0)
        .ue(7)
        .ue(0)
        .u(8, frame_num)
        .u(6, poc_lsb)
        .flag(false)
        .se(0);
    for addr in 0..macroblocks {
        w.ue(25);
        w.align_zero();
        w.bytes(&pcm_samples(addr));
    }
    w.finish()
}

/// Feeds one NAL unit and returns what the decoder made of it.
fn feed(decoder: &mut Decoder, ref_idc: u8, kind: u8, payload: Vec<u8>) -> Result<(), Error> {
    let unit = nal(ref_idc, kind, payload);
    let mut last = Ok(());
    for u in annex_b(&unit) {
        last = decoder.decode_nal(u);
    }
    last
}

/// The pictures a decoder has ready, as their luma samples.
fn pictures(decoder: &mut Decoder) -> Vec<Vec<u8>> {
    decoder
        .take_pictures()
        .iter()
        .map(|p| p.luma().rows().flatten().copied().collect())
        .collect()
}

#[test]
fn an_access_unit_delimiter_or_sei_ends_the_picture_before_it() {
    // Two IDR pictures with identical slice headers, which only the
    // access unit boundary between them tells apart. Clause 7.4.1.2.4
    // compares consecutive slices; the SEI says the access unit ended, so
    // the slice after it starts a picture whatever its header holds.
    let mut decoder = Decoder::new(Limits::default());
    feed(&mut decoder, 3, 7, sps(2, 2, None)).expect("sps");
    feed(&mut decoder, 3, 8, pps()).expect("pps");
    feed(&mut decoder, 3, 5, idr_pcm_slice(4, pcm_samples)).expect("the first picture");
    feed(&mut decoder, 0, 6, vec![0x80]).expect("an SEI");
    feed(&mut decoder, 3, 5, idr_pcm_slice(4, pcm_samples)).expect("the second picture");
    decoder.flush().expect("flush");
    assert_eq!(pictures(&mut decoder).len(), 2);

    // The same of `flush`, after which a decoder is usable again.
    let mut decoder = Decoder::new(Limits::default());
    feed(&mut decoder, 3, 7, sps(2, 2, None)).expect("sps");
    feed(&mut decoder, 3, 8, pps()).expect("pps");
    feed(&mut decoder, 3, 5, idr_pcm_slice(4, pcm_samples)).expect("the first picture");
    decoder.flush().expect("flush");
    assert_eq!(pictures(&mut decoder).len(), 1);
    feed(&mut decoder, 3, 5, idr_pcm_slice(4, pcm_samples)).expect("a picture after the flush");
    decoder.flush().expect("flush");
    assert_eq!(pictures(&mut decoder).len(), 1);
}

#[test]
fn the_caller_can_end_an_access_unit_itself() {
    // A caller whose packets are access units (the decode boundary's)
    // ends each one, and the picture comes out without waiting for the
    // packet after it.
    let mut decoder = Decoder::new(Limits::default());
    feed(&mut decoder, 3, 7, sps_without_reordering(2, 2)).expect("sps");
    feed(&mut decoder, 3, 8, pps()).expect("pps");
    feed(&mut decoder, 3, 5, idr_pcm_slice(4, pcm_samples)).expect("the picture");
    decoder.end_access_unit().expect("the access unit ends");
    assert_eq!(pictures(&mut decoder).len(), 1);
}

#[test]
fn the_keyframe_mode_decodes_a_keyframe_as_if_the_stream_started_at_it() {
    // An open group of pictures: a stream that starts at an intra picture
    // that is not an IDR picture, then predicted pictures, then another
    // intra picture whose pic_order_cnt_lsb has wrapped. Deriving its
    // picture order count from the skipped pictures' state would put it
    // before the random access point and drop it.
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 1, non_idr_pcm_slice(0, 12, 4)));
    stream.extend(nal(2, 1, skipped_p_slice(1, 40, 4)));
    stream.extend(nal(2, 1, skipped_p_slice(2, 62, 4)));
    stream.extend(nal(3, 1, non_idr_pcm_slice(3, 4, 4)));

    let decode = |mode| {
        let mut decoder = Decoder::new(Limits::default()).with_mode(mode);
        let mut out = Vec::new();
        for unit in annex_b(&stream) {
            decoder.decode_nal(unit).expect("the stream decodes");
            out.extend(pictures(&mut decoder));
        }
        decoder.flush().expect("flush");
        out.extend(pictures(&mut decoder));
        (out, decoder.warnings().len())
    };
    let (every, _) = decode(Mode::EveryPicture);
    let (keys, warnings) = decode(Mode::KeyframesOnly);
    assert_eq!(every.len(), 4);
    assert_eq!(keys.len(), 2, "both intra pictures are keyframes");
    assert_eq!(keys.first(), every.first());
    assert_eq!(keys.get(1), every.get(3));
    assert_eq!(warnings, 0);
}

#[test]
fn the_keyframe_mode_infers_no_gap_from_the_pictures_it_skipped() {
    // The skipped pictures move frame_num far from the keyframe's. A
    // decoder that carried their state would see a gap, and where the
    // stream allows gaps it would decode an inferred picture for each of
    // them: the work the mode exists to avoid.
    // The units go in one at a time: a slice of this stream may hold the
    // bytes of a start code, which only an encoder's emulation
    // prevention would keep out of a byte stream.
    let decode = |mode| {
        let mut decoder = Decoder::new(Limits::default()).with_mode(mode);
        let mut out = Vec::new();
        let unit = |decoder: &mut Decoder, ref_idc, kind, payload, out: &mut Vec<Vec<u8>>| {
            feed(decoder, ref_idc, kind, payload).expect("the stream decodes");
            out.extend(pictures(decoder));
        };
        unit(&mut decoder, 3, 7, sps_with_frame_num_gaps(2, 2), &mut out);
        unit(&mut decoder, 3, 8, pps(), &mut out);
        unit(&mut decoder, 3, 5, idr_pcm_slice8(4), &mut out);
        for i in 1..200u32 {
            unit(
                &mut decoder,
                2,
                1,
                skipped_p_slice8(i, (2 * i) % 64, 4),
                &mut out,
            );
        }
        unit(&mut decoder, 3, 1, non_idr_pcm_slice8(200, 10, 4), &mut out);
        decoder.flush().expect("flush");
        out.extend(pictures(&mut decoder));
        (out, decoder.warnings().len())
    };
    let (every, every_warnings) = decode(Mode::EveryPicture);
    let (keys, warnings) = decode(Mode::KeyframesOnly);
    assert_eq!(every.len(), 201);
    assert_eq!(every_warnings, 0);
    assert_eq!(keys.len(), 2);
    assert_eq!(keys.first(), every.first());
    assert_eq!(keys.get(1), every.last());
    assert_eq!(warnings, 0, "no inferred gap, so no picture to fill it");
}

#[test]
fn the_keyframe_mode_returns_a_picture_a_later_access_unit_discards() {
    // What the mode guarantees (`docs/design/h264.md`): the picture full
    // decoding *decodes* for the access unit, not the one it outputs. An
    // IDR picture with no_output_of_prior_pics_flag tells a decoder to
    // drop what it holds; a keyframe-only decoder has returned those
    // pictures already, which is what a caller that asked for the frame
    // at that access unit wants.
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_slice(0, 0, false, 4)));
    stream.extend(nal(3, 1, non_idr_pcm_slice(1, 8, 4)));
    stream.extend(nal(3, 5, idr_slice(1, 0, true, 4)));

    let decode = |mode| {
        let mut decoder = Decoder::new(Limits::default()).with_mode(mode);
        let mut out = Vec::new();
        for unit in annex_b(&stream) {
            decoder.decode_nal(unit).expect("the stream decodes");
            out.extend(pictures(&mut decoder));
        }
        decoder.flush().expect("flush");
        out.extend(pictures(&mut decoder));
        out
    };
    assert_eq!(decode(Mode::EveryPicture).len(), 1);
    assert_eq!(decode(Mode::KeyframesOnly).len(), 3);
}

#[test]
fn the_slices_of_a_skipped_picture_count_against_the_limit() {
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, None)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    // A predicted picture of many one-macroblock slices, which the
    // keyframe mode skips.
    for first_mb in 0..4u32 {
        let mut w = Writer::default();
        w.ue(first_mb)
            .ue(0) // slice_type: P
            .ue(0)
            .u(4, 1)
            .u(6, 2)
            .flag(false)
            .flag(false)
            .flag(false)
            .se(0)
            .ue(1);
        stream.extend(nal(2, 1, w.finish()));
    }
    let limits = Limits {
        max_slices_per_picture: 2,
        ..Limits::default()
    };
    let mut decoder = Decoder::new(limits).with_mode(Mode::KeyframesOnly);
    let mut last = Ok(());
    for unit in annex_b(&stream) {
        last = decoder.decode_nal(unit);
    }
    assert_eq!(last, Err(Error::Limit(LimitKind::SlicesPerPicture)));
}

/// A non-IDR intra slice whose marking resets every reference picture
/// (memory_management_control_operation 5, clause 7.4.3.3).
fn mmco5_pcm_slice(frame_num: u32, poc_lsb: u32, macroblocks: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(7) // slice_type: I
        .ue(0) // pic_parameter_set_id
        .u(4, frame_num)
        .u(6, poc_lsb)
        .flag(true) // adaptive_ref_pic_marking_mode_flag
        .ue(5) // memory_management_control_operation: reset
        .ue(0) // end of the operations
        .se(0); // slice_qp_delta
    for addr in 0..macroblocks {
        w.ue(25);
        w.align_zero();
        w.bytes(&pcm_samples(addr));
    }
    w.finish()
}

#[test]
fn a_picture_whose_marking_resets_the_count_is_still_cropped() {
    // Operation 5 sets the picture order count of its picture to 0
    // (clause 8.2.1) and nothing else: the cropping rectangle is the
    // sequence parameter set's, as it is for every other picture.
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps(2, 2, Some([1, 0, 0, 2]))));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    stream.extend(nal(3, 1, mmco5_pcm_slice(1, 2, 4)));

    let mut decoder = Decoder::new(Limits::default());
    for unit in annex_b(&stream) {
        decoder.decode_nal(unit).expect("the stream decodes");
    }
    decoder.flush().expect("flush");
    let pictures = decoder.take_pictures();
    assert_eq!(pictures.len(), 2);
    for picture in &pictures {
        assert_eq!((picture.width(), picture.height()), (30, 28));
    }
    assert_eq!(pictures.get(1).map(|p| p.poc()), Some(0));
}

#[test]
fn a_picture_invented_for_a_frame_num_gap_is_grey_not_the_pool() {
    // Clause 8.2.5.2 lets the decoder choose the samples of a frame it
    // invents for a frame_num gap, and this decoder conceals from the
    // previous reference picture. Where there is none it fills with grey:
    // the picture comes from the pool, which decoding does not clear, and
    // it is stored as a reference a later picture may predict from.
    //
    // Reaching that path needs a gap with no reference before it, so the
    // stream opens with non-reference pictures — they are decoded, they
    // leave their samples in the pool as they are dropped, and they never
    // become `last_reference`. The picture that opens the gap is also a
    // non-reference one, so it does not join the reference list and the
    // invented frame stays the most recent reference. The P picture then
    // skips every macroblock, which copies that reference, putting the
    // invented samples where this test can see them.
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps_with_gaps(2, 2)));
    stream.extend(nal(3, 8, pps()));
    for poc in 0..3 {
        stream.extend(nal(0, 1, non_idr_pcm_slice_ref(0, poc * 2, 4, false)));
    }
    // frame_num jumps from 0 to 2, so frame 1 is invented.
    stream.extend(nal(0, 1, non_idr_pcm_slice_ref(2, 8, 4, false)));
    // Skipping every macroblock copies reference 0, the invented frame.
    stream.extend(nal(1, 1, skipped_p_slice(2, 10, 4)));

    // A small buffer so each picture is output, and so released, while
    // the stream is still being decoded: a picture the decoder still
    // holds for output cannot be reused either.
    let limits = Limits {
        max_dpb_frames: 1,
        ..Limits::default()
    };
    let mut decoder = Decoder::new(limits);
    let mut out = Vec::new();
    for unit in annex_b(&stream) {
        decoder.decode_nal(unit).expect("the stream decodes");
        // Dropped as they come: a picture the caller holds cannot be
        // reused, and this test needs the invented frame to land in a
        // buffer an earlier picture wrote.
        drop(decoder.take_pictures());
    }
    decoder.flush().expect("flush");
    out.extend(decoder.take_pictures());

    let last = out.last().expect("the skipped picture");
    // As above: the invented frame has to land in a buffer an earlier
    // picture wrote, or the test passes for the wrong reason.
    assert!(
        decoder.pooled_pictures() > 0,
        "no picture was reused, so the fill is not what this test measured"
    );
    let luma: Vec<u8> = last.luma().rows().flatten().copied().collect();
    let (cb, cr) = last.chroma();
    let blue: Vec<u8> = cb.rows().flatten().copied().collect();
    let red: Vec<u8> = cr.rows().flatten().copied().collect();
    assert!(
        luma.iter().all(|&s| s == 0),
        "the invented frame leaked samples into the prediction: {:?}",
        luma.get(..16)
    );
    assert!(blue.iter().chain(red.iter()).all(|&s| s == 128));
}

#[test]
fn a_frame_num_gap_picture_is_reached_by_direct_prediction() {
    // Exercises the path, and does NOT guard the motion clear on it.
    //
    // A picture invented for a frame_num gap (clause 8.2.5.2) is covered
    // by no slice and never reaches the end-of-picture fill, so neither
    // half of the invariant that lets a reused picture keep its motion
    // applies to it: nothing writes it and nothing clears it. It is kept
    // as a reference, and a later B picture's direct prediction reads the
    // motion of its co-located macroblock, so stale vectors there would
    // decide this stream's samples.
    //
    // This test reaches that reader -- the gap, the reordering that puts
    // the invented picture in both lists where pic_order_cnt_type is 0,
    // and the direct prediction -- but it cannot tell the clear from its
    // absence: `P_Skip` with no neighbours derives a zero vector, so the
    // stale motion it leaves is ref_idx 0 with a zero vector, and spatial
    // direct reaches a zero vector from that and from cleared motion
    // alike. Removing `clear_all_motion` leaves this test passing; it is
    // kept for the path coverage and named for what it does.
    //
    // The clear itself is guarded by
    // `decoder::tests::a_frame_invented_for_a_frame_num_gap_carries_no_motion`,
    // which looks at the stored frame's motion: 64 of its 64 blocks are
    // stale without the clear.
    //
    // The pool is dirtied with real motion first: skipped P pictures
    // write ref_idx 0 into every block, and are dropped so their buffers
    // are free. The invented picture then takes one of those buffers.
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, sps_with_gaps(2, 2)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    for poc in 1..4 {
        stream.extend(nal(0, 1, skipped_p_slice_ref(0, poc * 2, 4, false)));
    }
    // frame_num jumps from 0 to 2 on a non-reference picture, so frame 1
    // is invented and stays the most recent reference.
    stream.extend(nal(0, 1, non_idr_pcm_slice_ref(2, 10, 4, false)));
    // Every macroblock skipped in a B picture is direct prediction.
    // picNum 2 - 1 = 1: the invented frame.
    stream.extend(nal(0, 1, skipped_b_slice(2, 12, 4, false, 1)));

    let limits = Limits {
        max_dpb_frames: 1,
        ..Limits::default()
    };
    let mut decoder = Decoder::new(limits);
    let mut out = Vec::new();
    for unit in annex_b(&stream) {
        decoder.decode_nal(unit).expect("the stream decodes");
        drop(decoder.take_pictures());
    }
    decoder.flush().expect("flush");
    out.extend(decoder.take_pictures());

    assert!(
        decoder.pooled_pictures() > 0,
        "no picture was reused, so the invented frame did not come from a dirty buffer"
    );
    // The invented frame carries no motion, so direct prediction gives
    // every block a zero vector into it, and the B picture is a copy of
    // it. It was concealed from the IDR, so that is what comes out.
    let last = out.last().expect("the B picture");
    let luma: Vec<u8> = last.luma().rows().flatten().copied().collect();
    let expected = pcm_samples(0);
    assert_eq!(
        luma.get(..16).map(<[u8]>::to_vec),
        expected.get(..16).map(<[u8]>::to_vec),
        "the B picture is the invented frame, concealed from the IDR"
    );
}

/// A High profile sequence parameter set of `mb_w` by `mb_h` macroblocks
/// whose pictures are monochrome: chroma_format_idc 0.
fn monochrome_sps(mb_w: u32, mb_h: u32, bit_depth_chroma_minus8: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, 100).u(8, 0).u(8, 40);
    w.ue(0) // seq_parameter_set_id
        .ue(0) // chroma_format_idc: 4:0:0
        .ue(0) // bit_depth_luma_minus8
        .ue(bit_depth_chroma_minus8)
        .flag(false) // qpprime_y_zero_transform_bypass_flag
        .flag(false) // seq_scaling_matrix_present_flag
        .ue(0) // log2_max_frame_num_minus4
        .ue(0) // pic_order_cnt_type
        .ue(2) // log2_max_pic_order_cnt_lsb_minus4
        .ue(1) // max_num_ref_frames
        .flag(false) // gaps_in_frame_num_value_allowed_flag
        .ue(mb_w - 1)
        .ue(mb_h - 1)
        .flag(true) // frame_mbs_only_flag
        .flag(true) // direct_8x8_inference_flag
        .flag(false) // frame_cropping_flag
        .flag(false); // vui_parameters_present_flag
    w.finish()
}

/// An IDR slice of a picture whose slices are all I, `data` writing its
/// slice_data().
fn idr_i_slice(data: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(7) // slice_type: I, and every slice of the picture is I
        .ue(0) // pic_parameter_set_id
        .u(4, 0) // frame_num
        .ue(0) // idr_pic_id
        .u(6, 0) // pic_order_cnt_lsb
        .flag(false) // no_output_of_prior_pics_flag
        .flag(false) // long_term_reference_flag
        .se(0); // slice_qp_delta
    data(&mut w);
    w.finish()
}

/// Decodes `stream` whole: its one picture, or the first error.
fn decode_one(stream: &[u8]) -> Result<std::sync::Arc<coreav_h264::Picture>, Error> {
    let mut decoder = Decoder::new(Limits::default());
    for unit in annex_b(stream) {
        decoder.decode_nal(unit)?;
    }
    decoder.flush()?;
    assert!(decoder.warnings().is_empty());
    let mut pictures = decoder.take_pictures();
    assert_eq!(pictures.len(), 1);
    Ok(pictures.remove(0))
}

#[test]
fn decodes_a_monochrome_picture_of_i_pcm_macroblocks() {
    // A monochrome I_PCM macroblock carries 256 luma samples and no chroma
    // (clause 7.3.5), so each macroblock here starts 256 bytes after the
    // one before. Read as 4:2:0, the second would start 128 bytes late.
    // The chroma bit depth is not used either (the NOTE in clause
    // 7.4.2.1.1), so 10 bits there is no reason to refuse the stream.
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, monochrome_sps(2, 2, 2)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(
        3,
        5,
        idr_i_slice(|w| {
            for addr in 0..4 {
                w.ue(25).align_zero().bytes(&pcm_samples(addr)[..256]);
            }
        }),
    ));
    let picture = decode_one(&stream).expect("the stream decodes");
    assert_eq!((picture.width(), picture.height()), (32, 32));
    let luma: Vec<&[u8]> = picture.luma().rows().collect();
    for addr in 0..4 {
        let want = pcm_samples(addr);
        let (x, y) = ((addr % 2) * 16, (addr / 2) * 16);
        for row in 0..16 {
            assert_eq!(
                &luma[y + row][x..x + 16],
                &want[row * 16..row * 16 + 16],
                "macroblock {addr} row {row}"
            );
        }
    }
    // The chroma planes are neutral, 128, which a caller that needs 4:2:0
    // shows as grey (docs/design/h264.md, "Monochrome").
    let (cb, cr) = picture.chroma();
    assert_eq!((cb.width(), cb.height()), (16, 16));
    assert!(cb.rows().chain(cr.rows()).flatten().all(|&s| s == 128));
}

/// The arithmetic encoder of clause 9.3.4.2, enough to write the bins of
/// an I_PCM macroblock and end_of_slice_flag.
struct Arithmetic<'w> {
    w: &'w mut Writer,
    low: u32,
    range: u32,
    first_bit: bool,
    outstanding: u32,
}

impl<'w> Arithmetic<'w> {
    /// InitEncoder (clause 9.3.4.1).
    fn new(w: &'w mut Writer) -> Self {
        Self {
            w,
            low: 0,
            range: 510,
            first_bit: true,
            outstanding: 0,
        }
    }

    /// PutBit (clause 9.3.4.2).
    fn put(&mut self, b: u32) {
        if self.first_bit {
            self.first_bit = false;
        } else {
            self.w.u(1, b);
        }
        for _ in 0..self.outstanding {
            self.w.u(1, 1 - b);
        }
        self.outstanding = 0;
    }

    /// RenormE (clause 9.3.4.2).
    fn renormalize(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put(1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }

    /// EncodeDecision of the least probable symbol, whose range is
    /// `lps` (Table 9-44).
    fn lps(&mut self, lps: u32) {
        self.low += self.range - lps;
        self.range = lps;
        self.renormalize();
    }

    /// EncodeTerminate (clause 9.3.4.5); a 1 ends with EncodeFlush,
    /// short of its last bit, which is 1: the caller writes it, or the
    /// RBSP's stop bit is it.
    fn terminate(&mut self, bin: bool) {
        self.range -= 2;
        if !bin {
            self.renormalize();
            return;
        }
        self.low += self.range;
        self.range = 2;
        self.renormalize();
        self.put((self.low >> 9) & 1);
        self.w.u(1, (self.low >> 8) & 1);
    }
}

#[test]
fn decodes_a_monochrome_cabac_picture_of_i_pcm_macroblocks() {
    // As decodes_a_monochrome_picture_of_i_pcm_macroblocks, in CABAC. At
    // SliceQPY 26, mb_type's first bin uses ctxIdx 3 (no neighbour),
    // pStateIdx 46 and valMPS 0, then ctxIdx 4 (an I_PCM neighbour on the
    // left, clause 9.3.3.1.1.3), pStateIdx 6 and valMPS 0 (clause 9.3.1.1,
    // Table 9-12), so a 1 is the least probable symbol, of range 22 and
    // 175 (Table 9-44). The second bin is DecodeTerminate's 1.
    let mut pps = Writer::default();
    pps.ue(0) // pic_parameter_set_id
        .ue(0) // seq_parameter_set_id
        .flag(true) // entropy_coding_mode_flag
        .flag(false) // bottom_field_pic_order_in_frame_present_flag
        .ue(0) // num_slice_groups_minus1
        .ue(0) // num_ref_idx_l0_default_active_minus1
        .ue(0) // num_ref_idx_l1_default_active_minus1
        .flag(false) // weighted_pred_flag
        .u(2, 0) // weighted_bipred_idc
        .se(0) // pic_init_qp_minus26
        .se(0) // pic_init_qs_minus26
        .se(0) // chroma_qp_index_offset
        .flag(false) // deblocking_filter_control_present_flag
        .flag(false) // constrained_intra_pred_flag
        .flag(false); // redundant_pic_cnt_present_flag
    let slice = idr_i_slice(|w| {
        w.align_one(); // cabac_alignment_one_bit
        let mut first = Arithmetic::new(w);
        first.lps(22);
        first.terminate(true);
        w.u(1, 1).align_zero().bytes(&pcm_samples(0)[..256]);
        let mut second = Arithmetic::new(w);
        second.terminate(false); // end_of_slice_flag
        second.lps(175);
        second.terminate(true);
        w.u(1, 1).align_zero().bytes(&pcm_samples(1)[..256]);
        Arithmetic::new(w).terminate(true); // end_of_slice_flag
    });
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, monochrome_sps(2, 1, 0)));
    stream.extend(nal(3, 8, pps.finish()));
    stream.extend(nal(3, 5, slice));
    let picture = decode_one(&stream).expect("the stream decodes");
    assert_eq!((picture.width(), picture.height()), (32, 16));
    let luma: Vec<&[u8]> = picture.luma().rows().collect();
    for addr in 0..2 {
        let want = pcm_samples(addr);
        for row in 0..16 {
            assert_eq!(
                &luma[row][addr * 16..addr * 16 + 16],
                &want[row * 16..row * 16 + 16],
                "macroblock {addr} row {row}"
            );
        }
    }
    let (cb, cr) = picture.chroma();
    assert!(cb.rows().chain(cr.rows()).flatten().all(|&s| s == 128));
}

#[test]
fn monochrome_macroblocks_code_no_chroma_syntax() {
    // Two intra macroblocks of a CAVLC slice, written as clause 7.3.5 has
    // them for ChromaArrayType 0: no intra_chroma_pred_mode, the
    // coded_block_pattern of Table 9-4 (b), and no chroma residual. A
    // reader that expects chroma reads the wrong bits from the first
    // element it adds.
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, monochrome_sps(2, 1, 0)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(
        3,
        5,
        idr_i_slice(|w| {
            // I_NxN, every Intra4x4PredMode predicted: DC throughout.
            w.ue(0);
            for _ in 0..16 {
                w.flag(true); // prev_intra4x4_pred_mode_flag
            }
            w.ue(0) // coded_block_pattern: codeNum 0, 15 for intra in (b)
                .se(0); // mb_qp_delta
            for _ in 0..16 {
                w.u(1, 1); // coeff_token: no coefficients, nC 0
            }
            // I_16x16_2_0_0: DC prediction, no AC and no chroma.
            w.ue(3)
                .se(0) // mb_qp_delta
                .u(2, 0b01) // Intra16x16DCLevel coeff_token: one trailing one
                .u(1, 0) // trailing_ones_sign_flag: +1
                .u(1, 1); // total_zeros: 0
        }),
    ));
    let picture = decode_one(&stream).expect("the stream decodes");
    // The first macroblock predicts 128 and adds nothing. The second adds
    // a DC level of 1 at QP 26, which is 1 in every sample (clause
    // 8.5.10). The columns near the edge between them are deblocked.
    for row in picture.luma().rows() {
        assert!(row[..12].iter().all(|&s| s == 128), "{row:?}");
        assert!(row[20..].iter().all(|&s| s == 129), "{row:?}");
    }
    let (cb, cr) = picture.chroma();
    assert!(cb.rows().chain(cr.rows()).flatten().all(|&s| s == 128));
}

#[test]
fn a_monochrome_macroblock_with_coded_chroma_is_refused() {
    // I_16x16_2_1_0 gives CodedBlockPatternChroma 1, which a monochrome
    // stream may not (clause 7.4.5).
    let mut stream = Vec::new();
    stream.extend(nal(3, 7, monochrome_sps(1, 1, 0)));
    stream.extend(nal(3, 8, pps()));
    stream.extend(nal(
        3,
        5,
        idr_i_slice(|w| {
            w.ue(7).se(0).u(1, 1);
        }),
    ));
    let mut decoder = Decoder::new(Limits::default());
    let results: Vec<_> = annex_b(&stream).map(|u| decoder.decode_nal(u)).collect();
    assert!(
        matches!(results.last(), Some(Err(Error::Invalid("mb_type")))),
        "{results:?}"
    );
}
