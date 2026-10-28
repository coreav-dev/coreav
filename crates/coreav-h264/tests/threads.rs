//! Threaded decoding gives the pictures one thread gives, sample for
//! sample and in the same order (`docs/design/h264.md`, "Bit-exact whatever
//! the thread count"), on streams the test builds: pictures cut into many
//! bands, motion vectors that reach across them, intra macroblocks among
//! inter ones, several slices a picture and deblocking across their
//! boundaries.

// Test code: the lints that keep production code panic-free are relaxed
// here, as clippy.toml does for unit tests.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
#![forbid(unsafe_code)]

use std::sync::Arc;

use coreav_h264::{Decoder, Limits, Picture, annex_b};

#[path = "common/streams.rs"]
mod streams;
use streams::*;

/// The picture: 4 macroblocks wide and 20 high, which threaded decoding
/// cuts into ten bands of two macroblock rows.
const MB_W: usize = 4;
const MB_H: usize = 20;
const MBS: usize = MB_W * MB_H;

/// A small deterministic generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn between(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u64) as i32
    }
}

/// A baseline sequence parameter set with `refs` reference frames.
fn sps_with_refs(refs: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, 66).u(8, 0xc0).u(8, 30);
    w.ue(0) // seq_parameter_set_id
        .ue(0) // log2_max_frame_num_minus4
        .ue(0) // pic_order_cnt_type
        .ue(2) // log2_max_pic_order_cnt_lsb_minus4
        .ue(refs) // max_num_ref_frames
        .flag(false) // gaps_in_frame_num_value_allowed_flag
        .ue(MB_W as u32 - 1)
        .ue(MB_H as u32 - 1)
        .flag(true) // frame_mbs_only_flag
        .flag(true) // direct_8x8_inference_flag
        .flag(false) // frame_cropping_flag
        .flag(false); // vui_parameters_present_flag
    w.finish()
}

/// Picture parameter set 1: as `pps()`, with the deblocking control in
/// the slice headers.
fn pps_with_filter_control() -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(1) // pic_parameter_set_id
        .ue(0) // seq_parameter_set_id
        .flag(false) // entropy_coding_mode_flag
        .flag(false) // bottom_field_pic_order_in_frame_present_flag
        .ue(0) // num_slice_groups_minus1
        .ue(0) // num_ref_idx_l0_default_active_minus1
        .ue(0) // num_ref_idx_l1_default_active_minus1
        .flag(false) // weighted_pred_flag
        .u(2, 0) // weighted_bipred_idc
        .se(0) // pic_init_qp_minus26
        .se(0) // pic_init_qs_minus26
        .se(2) // chroma_qp_index_offset
        .flag(true) // deblocking_filter_control_present_flag
        .flag(false) // constrained_intra_pred_flag
        .flag(false); // redundant_pic_cnt_present_flag
    w.finish()
}

/// The references a P slice predicts from: how many its list holds, and
/// the vertical vector differences, in quarter samples, it draws.
#[derive(Clone, Copy)]
struct Refs {
    active: u32,
    mvd_y: i32,
}

/// One P slice of `count` macroblocks from `first`: runs of skipped
/// macroblocks, P_L0_16x16 macroblocks with a random reference of the
/// `refs.active` and random vector differences, and now and then an I_PCM
/// one.
#[allow(clippy::too_many_arguments)]
fn p_slice(
    rng: &mut Rng,
    frame_num: u32,
    poc_lsb: u32,
    pps_id: u32,
    first: usize,
    count: usize,
    reference: bool,
    refs: Refs,
) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(first as u32) // first_mb_in_slice
        .ue(0) // slice_type: P
        .ue(pps_id)
        .u(4, frame_num)
        .u(6, poc_lsb);
    if refs.active > 1 {
        w.flag(true) // num_ref_idx_active_override_flag
            .ue(refs.active - 1); // num_ref_idx_l0_active_minus1
    } else {
        w.flag(false); // num_ref_idx_active_override_flag
    }
    w.flag(false); // ref_pic_list_modification_flag_l0
    if reference {
        w.flag(false); // adaptive_ref_pic_marking_mode_flag
    }
    w.se(rng.between(-12, 12)); // slice_qp_delta
    if pps_id == 1 {
        let idc = rng.below(3) as u32;
        w.ue(idc); // disable_deblocking_filter_idc
        if idc != 1 {
            w.se(rng.between(-3, 3)).se(rng.between(-3, 3));
        }
    }
    let mut left = count;
    while left > 0 {
        let run = (rng.below(4) as usize).min(left);
        w.ue(run as u32); // mb_skip_run
        left -= run;
        if left == 0 {
            break;
        }
        if rng.below(8) == 0 {
            w.ue(30); // mb_type: I_PCM in a P slice
            w.align_zero();
            w.bytes(&pcm_samples(
                (first + count - left + 17 * frame_num as usize) % 150,
            ));
        } else {
            w.ue(0); // mb_type: P_L0_16x16
            match refs.active {
                1 => {}
                // te(v) with a range of 1 is one inverted bit (clause 9.1).
                2 => {
                    w.flag(rng.below(2) == 0); // ref_idx_l0
                }
                n => {
                    w.ue(rng.below(u64::from(n)) as u32); // ref_idx_l0
                }
            }
            w.se(rng.between(-24, 24)) // mvd_l0, x
                .se(rng.between(-refs.mvd_y, refs.mvd_y)) // mvd_l0, y
                .ue(0); // coded_block_pattern: none
        }
        left -= 1;
    }
    w.finish()
}

/// Adds the emulation prevention bytes a NAL unit payload needs
/// (clause 7.4.1).
fn escape(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    let mut zeros = 0;
    for &b in payload {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// A NAL unit with its payload escaped.
fn unit(ref_idc: u8, kind: u8, payload: &[u8]) -> Vec<u8> {
    nal(ref_idc, kind, escape(payload))
}

/// The stream, one access unit per entry: the parameter sets and an IDR
/// picture of I_PCM macroblocks, then `pictures` P pictures of one to
/// three slices each, every fourth of which is not a reference.
fn stream(seed: u64, pictures: u32) -> Vec<Vec<u8>> {
    stream_with_refs(seed, pictures, 1, 80)
}

/// [`stream`] with up to `refs` reference frames, each slice predicting
/// from as many as have been decoded, with vertical vector differences
/// up to `mvd_y` quarter samples.
fn stream_with_refs(seed: u64, pictures: u32, refs: u32, mvd_y: i32) -> Vec<Vec<u8>> {
    let mut rng = Rng(seed);
    let mut units = Vec::new();
    let mut first = unit(3, 7, &sps_with_refs(refs));
    first.extend(unit(3, 8, &pps()));
    first.extend(unit(3, 8, &pps_with_filter_control()));
    first.extend(unit(3, 5, &idr_pcm_slice(MBS, pcm_samples)));
    units.push(first);
    let mut frame_num = 1;
    // The IDR picture is the first reference.
    let mut decoded_refs = 1;
    for n in 1..=pictures {
        // A picture that is not a reference is cut into one band.
        let reference = n % 4 != 2;
        let pps_id = u32::from(n % 3 == 0);
        let slices = 1 + rng.below(3) as usize;
        let mut cuts: Vec<usize> = (1..slices)
            .map(|_| 1 + rng.below(MBS as u64 - 1) as usize)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut bounds = vec![0];
        bounds.extend(cuts);
        bounds.push(MBS);
        let mut au = Vec::new();
        for pair in bounds.windows(2) {
            let slice = p_slice(
                &mut rng,
                frame_num % 16,
                (2 * n) % 64,
                pps_id,
                pair[0],
                pair[1] - pair[0],
                reference,
                Refs {
                    active: decoded_refs.min(refs),
                    mvd_y,
                },
            );
            au.extend(unit(if reference { 2 } else { 0 }, 1, &slice));
        }
        units.push(au);
        // frame_num counts reference pictures (clause 7.4.3).
        if reference {
            frame_num += 1;
            decoded_refs += 1;
        }
    }
    units
}

/// A hash of every sample of a picture.
fn digest(picture: &Picture) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let (cb, cr) = picture.chroma();
    for rows in [picture.luma().rows(), cb.rows(), cr.rows()] {
        for row in rows {
            for &s in row {
                hash = (hash ^ u64::from(s)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    hash
}

/// How a decoder is set up.
#[derive(Clone, Copy, Debug)]
struct Setup {
    threads: usize,
    band_height: Option<usize>,
    /// End each access unit explicitly, as the decode-boundary backend
    /// does, rather than letting the next one end it.
    end_access_units: bool,
}

/// What a caller that carries on after errors receives: each picture's
/// mark and digest, in order, and how many calls failed.
fn decode(units: &[Vec<u8>], setup: Setup) -> (Vec<(u64, u64)>, usize) {
    let mut decoder = Decoder::new(Limits::default()).with_threads(setup.threads);
    if let Some(rows) = setup.band_height {
        decoder = decoder.with_band_height(rows);
    }
    let mut out = Vec::new();
    let mut errors = 0;
    let take = |decoder: &mut Decoder, out: &mut Vec<(u64, u64)>| {
        for picture in decoder.take_pictures() {
            out.push((picture.mark(), digest(&picture)));
        }
    };
    for (n, au) in units.iter().enumerate() {
        decoder.set_mark(n as u64);
        for nal in annex_b(au) {
            if decoder.decode_nal(nal).is_err() {
                errors += 1;
            }
            take(&mut decoder, &mut out);
        }
        if setup.end_access_units && decoder.end_access_unit().is_err() {
            errors += 1;
        }
        take(&mut decoder, &mut out);
    }
    if decoder.flush().is_err() {
        errors += 1;
    }
    take(&mut decoder, &mut out);
    (out, errors)
}

const SETUPS: [Setup; 9] = [
    Setup {
        threads: 1,
        band_height: Some(1),
        end_access_units: false,
    },
    Setup {
        threads: 1,
        band_height: Some(2),
        end_access_units: false,
    },
    Setup {
        threads: 1,
        band_height: Some(3),
        end_access_units: true,
    },
    Setup {
        threads: 2,
        band_height: None,
        end_access_units: false,
    },
    Setup {
        threads: 3,
        band_height: None,
        end_access_units: true,
    },
    Setup {
        threads: 3,
        band_height: Some(4),
        end_access_units: false,
    },
    Setup {
        threads: 8,
        band_height: None,
        end_access_units: false,
    },
    Setup {
        threads: 8,
        band_height: Some(2),
        end_access_units: true,
    },
    Setup {
        threads: 8,
        band_height: Some(1),
        end_access_units: false,
    },
];

const ONE: Setup = Setup {
    threads: 1,
    band_height: None,
    end_access_units: false,
};

#[test]
fn threads_give_the_pictures_one_thread_gives() {
    for seed in [1u64, 2, 3] {
        let units = stream(seed, 24);
        let (want, errors) = decode(&units, ONE);
        assert_eq!(errors, 0, "seed {seed}: the stream decodes");
        assert_eq!(
            want.len(),
            units.len(),
            "seed {seed}: a picture an access unit"
        );
        // The pictures differ from one another: the stream moves.
        let distinct: std::collections::HashSet<u64> = want.iter().map(|p| p.1).collect();
        assert!(
            distinct.len() > units.len() / 2,
            "seed {seed}: the pictures change"
        );
        for setup in SETUPS {
            let (got, errors) = decode(&units, setup);
            assert_eq!(errors, 0, "seed {seed}, {setup:?}");
            assert_eq!(got, want, "seed {seed}, {setup:?}");
        }
    }
}

/// Slices that predict from eight references, with vectors that reach
/// many bands up or down: blocks read bands of references that share a
/// slot of the slice's kept band views (`docs/design/h264.md`, "Threading
/// round: band views kept for the slice"), references four apart and
/// bands four apart, so a view kept for one must never serve the other.
#[test]
fn many_references_give_the_pictures_one_thread_gives() {
    for seed in [11u64, 12] {
        let units = stream_with_refs(seed, 20, 8, 160);
        let (want, errors) = decode(&units, ONE);
        assert_eq!(errors, 0, "seed {seed}: the stream decodes");
        assert_eq!(
            want.len(),
            units.len(),
            "seed {seed}: a picture an access unit"
        );
        for setup in SETUPS {
            let (got, errors) = decode(&units, setup);
            assert_eq!(errors, 0, "seed {seed}, {setup:?}");
            assert_eq!(got, want, "seed {seed}, {setup:?}");
        }
    }
}

#[test]
fn a_broken_stream_gives_the_same_pictures_with_threads() {
    // A caller that carries on after errors receives the same pictures in
    // the same order, whatever the thread count; the errors may come from
    // later calls.
    let mut failed = 0;
    for seed in 1..=12u64 {
        let mut rng = Rng(seed * 7919);
        let mut units = stream(seed, 16);
        for au in units.iter_mut().skip(1) {
            match rng.below(5) {
                // Flip a byte past the slice header's first bytes.
                0 | 1 if au.len() > 12 => {
                    let at = 8 + rng.below(au.len() as u64 - 8) as usize;
                    au[at] ^= 1 << rng.below(8);
                }
                // Cut the access unit short.
                2 if au.len() > 12 => {
                    let keep = 6 + rng.below(au.len() as u64 - 6) as usize;
                    au.truncate(keep);
                }
                // Lose it.
                3 => au.clear(),
                _ => {}
            }
        }
        let (want, errors) = decode(&units, ONE);
        failed += errors;
        for setup in [
            Setup {
                threads: 3,
                band_height: None,
                end_access_units: false,
            },
            Setup {
                threads: 3,
                band_height: None,
                end_access_units: true,
            },
            Setup {
                threads: 2,
                band_height: Some(2),
                end_access_units: false,
            },
            Setup {
                threads: 3,
                band_height: Some(1),
                end_access_units: true,
            },
        ] {
            let (got, _) = decode(&units, setup);
            assert_eq!(got, want, "seed {seed}, {setup:?}");
        }
    }
    assert!(
        failed > 0,
        "no corruption was noticed: the test tests nothing"
    );
}

/// What a caller that stops at the first error receives: the pictures
/// taken before the first call that failed, and whether one did.
fn decode_until_error(units: &[Vec<u8>], setup: Setup) -> (Vec<(u64, u64)>, bool) {
    let mut decoder = Decoder::new(Limits::default()).with_threads(setup.threads);
    let mut out = Vec::new();
    for (n, au) in units.iter().enumerate() {
        decoder.set_mark(n as u64);
        for nal in annex_b(au) {
            if decoder.decode_nal(nal).is_err() {
                return (out, true);
            }
            for picture in decoder.take_pictures() {
                out.push((picture.mark(), digest(&picture)));
            }
        }
        if setup.end_access_units && decoder.end_access_unit().is_err() {
            return (out, true);
        }
        for picture in decoder.take_pictures() {
            out.push((picture.mark(), digest(&picture)));
        }
    }
    if decoder.flush().is_err() {
        return (out, true);
    }
    for picture in decoder.take_pictures() {
        out.push((picture.mark(), digest(&picture)));
    }
    (out, false)
}

#[test]
fn a_caller_stopping_at_the_first_error_gets_no_picture_one_thread_would_not() {
    // With threads an error is reported by a later call than with one, so
    // a caller that stops at the first error may receive fewer pictures,
    // but never one that one thread would not have given it before its
    // own first error: the threaded pictures are a prefix of those.
    let mut stopped = 0;
    for seed in 1..=16u64 {
        let mut rng = Rng(seed * 104_729);
        let mut units = stream(seed, 16);
        for au in units.iter_mut().skip(1) {
            if rng.below(4) == 0 && au.len() > 12 {
                let at = 8 + rng.below(au.len() as u64 - 8) as usize;
                au[at] ^= 1 << rng.below(8);
            }
        }
        for end_access_units in [false, true] {
            let one = Setup {
                end_access_units,
                ..ONE
            };
            let (want, failed) = decode_until_error(&units, one);
            stopped += usize::from(failed);
            for threads in [2, 3, 8] {
                let (got, _) = decode_until_error(&units, Setup { threads, ..one });
                assert!(
                    want.starts_with(&got),
                    "seed {seed}, {threads} threads: {} pictures, not a prefix of the {} \
                     one thread gives",
                    got.len(),
                    want.len()
                );
            }
        }
    }
    assert!(
        stopped > 0,
        "no corruption was noticed: the test tests nothing"
    );
}

#[test]
fn a_decoder_dropped_mid_stream_stops_its_threads() {
    // Dropping a decoder with pictures in flight, one of them unfinished,
    // returns: every worker is stopped and joined, and a worker waiting
    // for a band of a picture another one holds is released.
    let units = stream(5, 12);
    for cut in [1, 3, 7] {
        let mut decoder = Decoder::new(Limits::default()).with_threads(4);
        for au in &units[..cut] {
            for nal in annex_b(au) {
                decoder.decode_nal(nal).expect("decodes");
            }
        }
        // Half of the next picture's slices.
        if let Some(nal) = annex_b(&units[cut]).next() {
            decoder.decode_nal(nal).expect("decodes");
        }
        let held: Vec<Arc<Picture>> = decoder.take_pictures();
        drop(decoder);
        drop(held);
    }
}

#[test]
fn lowering_the_thread_count_mid_stream_changes_no_picture() {
    // A decoder lowered to one thread starts cutting its pictures into one
    // band, and a picture invented for a frame_num gap is then concealed
    // from a reference cut into several.
    let units = stream(9, 16);
    let (want, _) = decode(&units, ONE);
    for (at, to) in [(3, 1), (5, 2), (1, 1)] {
        let mut decoder = Decoder::new(Limits::default()).with_threads(4);
        let mut got = Vec::new();
        for (n, au) in units.iter().enumerate() {
            if n == at {
                decoder.limit_threads(to);
                assert_eq!(decoder.threads(), to);
            }
            decoder.set_mark(n as u64);
            for nal in annex_b(au) {
                decoder.decode_nal(nal).expect("decodes");
            }
            got.extend(
                decoder
                    .take_pictures()
                    .iter()
                    .map(|p| (p.mark(), digest(p))),
            );
        }
        decoder.flush().expect("flush");
        got.extend(
            decoder
                .take_pictures()
                .iter()
                .map(|p| (p.mark(), digest(p))),
        );
        assert_eq!(got, want, "lowered to {to} at access unit {at}");
    }

    let mut gap = Vec::new();
    gap.extend(nal(3, 7, sps_with_gaps(4, 8)));
    gap.extend(nal(3, 8, pps()));
    gap.extend(nal(3, 5, idr_pcm_slice(32, pcm_samples)));
    gap.extend(nal(2, 1, non_idr_pcm_slice_ref(1, 2, 32, true)));
    // frame_num 2 is missing: it is invented, concealed from frame 1, and
    // frame 3 is predicted from it.
    let skipped = |frame_num, poc| nal(2, 1, skipped_p_slice_ref(frame_num, poc, 32, true));
    let tail: Vec<Vec<u8>> = vec![skipped(3, 6), skipped(4, 8)];
    let run = |lower: bool| {
        let mut decoder = Decoder::new(Limits::default()).with_threads(3);
        let mut out = Vec::new();
        for nal in annex_b(&gap) {
            decoder.decode_nal(nal).expect("decodes");
        }
        if lower {
            decoder.limit_threads(1);
        }
        for unit in &tail {
            for nal in annex_b(unit) {
                decoder.decode_nal(nal).expect("decodes");
            }
        }
        decoder.flush().expect("flush");
        out.extend(decoder.take_pictures().iter().map(|p| digest(p)));
        out
    };
    let mut one = Decoder::new(Limits::default());
    for nal in annex_b(&gap).chain(tail.iter().flat_map(|u| annex_b(u))) {
        one.decode_nal(nal).expect("decodes");
    }
    one.flush().expect("flush");
    let want: Vec<u64> = one.take_pictures().iter().map(|p| digest(p)).collect();
    assert_eq!(want.len(), 4);
    assert_eq!(run(false), want);
    assert_eq!(run(true), want);
}

#[test]
fn the_thread_count_is_bounded_by_the_limits() {
    let limits = Limits {
        max_threads: 3,
        ..Limits::default()
    };
    assert_eq!(Decoder::new(limits).with_threads(64).threads(), 3);
    assert_eq!(Decoder::new(limits).with_threads(0).threads(), 1);
    assert_eq!(Decoder::new(Limits::default()).threads(), 1);
}
