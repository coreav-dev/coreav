//! Every field of `Limits` is pinned at its boundary (`docs/design/h264.md`,
//! "Limits"): the value the limit allows decodes, one more is refused with
//! the error that names the limit, and the defaults are the documented
//! values. Streams are built bit by bit, so no encoder is needed.
#![allow(clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

use coreav_h264::{AnnexB, Auto, Decoder, Error, LimitKind, Limits, annex_b};

#[path = "common/streams.rs"]
mod streams;
use streams::*;

/// An IDR slice of `macroblocks` I_PCM macroblocks starting at `first_mb`.
fn idr_slice(first_mb: u32, macroblocks: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(first_mb)
        .ue(7) // slice_type: I, and every slice of the picture is I
        .ue(0) // pic_parameter_set_id
        .u(4, 0) // frame_num
        .ue(0) // idr_pic_id
        .u(6, 0) // pic_order_cnt_lsb
        .flag(false) // no_output_of_prior_pics_flag
        .flag(false) // long_term_reference_flag
        .se(0); // slice_qp_delta
    for addr in first_mb as usize..first_mb as usize + macroblocks {
        w.ue(25); // mb_type: I_PCM
        w.align_zero();
        w.bytes(&pcm_samples(addr));
    }
    w.finish()
}

/// Parameter sets and one IDR picture of `mb_w` by `mb_h` macroblocks in
/// `slices` slices, which must divide the macroblock count.
fn stream(mb_w: u32, mb_h: u32, max_num_ref_frames: u32, slices: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(nal(3, 7, sps_with_max_refs(mb_w, mb_h, max_num_ref_frames)));
    out.extend(nal(3, 8, pps()));
    let per_slice = (mb_w * mb_h) as usize / slices;
    for slice in 0..slices {
        out.extend(nal(3, 5, idr_slice((slice * per_slice) as u32, per_slice)));
    }
    out
}

/// What each NAL unit of `stream` returns; the decoder is flushed after.
fn decode(decoder: &mut Decoder<Auto>, stream: &[u8]) -> Vec<Result<(), Error>> {
    let results = annex_b(stream)
        .map(|unit| decoder.decode_nal(unit))
        .collect();
    let _ = decoder.flush();
    results
}

#[test]
fn the_default_limits_are_the_documented_values() {
    assert_eq!(
        Limits::default(),
        Limits {
            max_mbs: 139_264,
            max_dpb_frames: 16,
            max_slices_per_picture: 139_264,
            max_nal_bytes: 64 << 20,
            max_threads: 16,
            max_memory_bytes: 8 << 30,
        }
    );
}

#[test]
fn limits_for_a_size_count_whole_macroblocks_and_keep_the_other_defaults() {
    let hd = Limits::for_size(1920, 1080);
    assert_eq!(hd.max_mbs, 120 * 68);
    assert_eq!(Limits::for_size(17, 17).max_mbs, 4);
    assert_eq!(Limits::for_size(0, 0).max_mbs, 1);
    let default = Limits::default();
    assert_eq!(
        Limits {
            max_mbs: default.max_mbs,
            ..hd
        },
        default
    );
}

#[test]
fn a_picture_of_max_mbs_macroblocks_decodes_and_one_more_is_refused() {
    let stream = stream(3, 2, 1, 1);
    let mut decoder = Decoder::new(Limits {
        max_mbs: 6,
        ..Limits::default()
    });
    assert!(decode(&mut decoder, &stream).iter().all(Result::is_ok));
    assert_eq!(decoder.parameter_sets_within_limits(), Ok(()));
    assert_eq!(decoder.take_pictures().len(), 1);

    let mut decoder = Decoder::new(Limits {
        max_mbs: 5,
        ..Limits::default()
    });
    let results = decode(&mut decoder, &stream);
    assert_eq!(
        results.last(),
        Some(&Err(Error::Limit(LimitKind::PictureSize)))
    );
    assert_eq!(
        decoder.parameter_sets_within_limits(),
        Err(Error::Limit(LimitKind::PictureSize))
    );
    assert!(decoder.take_pictures().is_empty());
}

#[test]
fn a_stream_of_max_dpb_frames_references_decodes_and_one_more_is_refused() {
    let limits = Limits {
        max_dpb_frames: 4,
        ..Limits::default()
    };
    let mut decoder = Decoder::new(limits);
    let results = decode(&mut decoder, &stream(2, 2, 4, 1));
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(decoder.parameter_sets_within_limits(), Ok(()));
    assert_eq!(decoder.take_pictures().len(), 1);

    let mut decoder = Decoder::new(limits);
    let results = decode(&mut decoder, &stream(2, 2, 5, 1));
    assert_eq!(
        results.last(),
        Some(&Err(Error::Limit(LimitKind::DpbFrames)))
    );
    assert_eq!(
        decoder.parameter_sets_within_limits(),
        Err(Error::Limit(LimitKind::DpbFrames))
    );
    assert!(decoder.take_pictures().is_empty());
}

#[test]
fn a_picture_of_max_slices_per_picture_slices_decodes_and_one_more_is_refused() {
    let stream = stream(2, 2, 1, 4);
    let mut decoder = Decoder::new(Limits {
        max_slices_per_picture: 4,
        ..Limits::default()
    });
    assert!(decode(&mut decoder, &stream).iter().all(Result::is_ok));
    assert_eq!(decoder.take_pictures().len(), 1);

    let mut decoder = Decoder::new(Limits {
        max_slices_per_picture: 3,
        ..Limits::default()
    });
    let results = decode(&mut decoder, &stream);
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 5);
    assert_eq!(
        results.last(),
        Some(&Err(Error::Limit(LimitKind::SlicesPerPicture)))
    );
}

#[test]
fn a_nal_unit_of_max_nal_bytes_passes_the_splitter_and_one_more_byte_is_refused() {
    let limits = Limits {
        max_nal_bytes: 16,
        ..Limits::default()
    };
    for (len, fits) in [(16, true), (17, false)] {
        let mut stream = vec![0, 0, 1, 0x65];
        stream.extend(std::iter::repeat_n(0x42, len - 1));
        let expected = if fits { vec![len] } else { Vec::new() };

        let mut seen = Vec::new();
        let mut splitter = AnnexB::new(&limits);
        let ended_by_a_start_code =
            splitter.push(&[stream.as_slice(), &[0, 0, 1]].concat(), |nal| {
                seen.push(nal.len());
                Ok(())
            });
        assert_eq!(seen, expected, "{len} bytes ended by a start code");
        assert_eq!(
            ended_by_a_start_code.err(),
            (!fits).then_some(Error::Limit(LimitKind::NalBytes))
        );

        let mut seen = Vec::new();
        let mut collect = |nal: &[u8]| {
            seen.push(nal.len());
            Ok(())
        };
        let mut splitter = AnnexB::new(&limits);
        let (head, tail) = stream.split_at(stream.len() / 2);
        let grown = splitter
            .push(head, &mut collect)
            .and_then(|()| splitter.push(tail, &mut collect));
        assert_eq!(
            grown.err(),
            (!fits).then_some(Error::Limit(LimitKind::NalBytes)),
            "{len} bytes growing across chunks"
        );
        if fits {
            splitter.finish(&mut collect).expect("finish");
        }
        assert_eq!(seen, expected, "{len} bytes ended by the stream");
    }
}

#[test]
fn a_picture_that_fits_max_memory_bytes_decodes_and_one_byte_less_is_refused() {
    let stream = stream(2, 2, 1, 1);
    let limits = Limits::default();
    let needed = Decoder::<Auto>::memory_needed(2, 2, 1, &limits);

    let mut decoder = Decoder::new(Limits {
        max_memory_bytes: needed,
        ..limits
    });
    assert!(decode(&mut decoder, &stream).iter().all(Result::is_ok));
    assert_eq!(decoder.take_pictures().len(), 1);

    let mut decoder = Decoder::new(Limits {
        max_memory_bytes: needed - 1,
        ..limits
    });
    let results = decode(&mut decoder, &stream);
    assert_eq!(results.last(), Some(&Err(Error::Limit(LimitKind::Memory))));
    assert!(decoder.take_pictures().is_empty());
}

#[test]
fn the_thread_count_is_lowered_until_the_pictures_fit_max_memory_bytes() {
    let stream = stream(2, 2, 1, 1);
    let limits = Limits::default();
    let two = Decoder::<Auto>::memory_needed(2, 2, 2, &limits);
    assert!(Decoder::<Auto>::memory_needed(2, 2, 1, &limits) < two);

    let mut roomy = Decoder::new(Limits {
        max_memory_bytes: two,
        ..limits
    })
    .with_threads(2);
    assert!(decode(&mut roomy, &stream).iter().all(Result::is_ok));
    assert_eq!(roomy.threads(), 2);

    let mut tight = Decoder::new(Limits {
        max_memory_bytes: two - 1,
        ..limits
    })
    .with_threads(2);
    assert!(decode(&mut tight, &stream).iter().all(Result::is_ok));
    assert_eq!(tight.threads(), 1);
    assert_eq!(tight.take_pictures().len(), 1);
}

#[test]
fn a_small_picture_keeps_its_memory_bound_when_eight_threads_are_lowered() {
    let stream = stream(2, 30, 1, 1);
    let limits = Limits::default();
    let needed = Decoder::<Auto>::memory_needed(2, 30, 8, &limits);
    let mut decoder = Decoder::new(Limits {
        max_memory_bytes: needed,
        ..limits
    })
    .with_threads(8);
    assert!(decode(&mut decoder, &stream).iter().all(Result::is_ok));
    assert_eq!(decoder.threads(), 8);
    let before = decoder.take_pictures();
    assert_eq!(before.len(), 1);
    decoder.limit_threads(7);
    assert!(decode(&mut decoder, &stream).iter().all(Result::is_ok));
    assert_eq!(decoder.threads(), 7);
    let after = decoder.take_pictures();
    assert_eq!(after.len(), 1);
    for (old, new) in before.iter().zip(&after) {
        let (old_cb, old_cr) = old.chroma();
        let (new_cb, new_cr) = new.chroma();
        for (old, new) in [old.luma(), old_cb, old_cr]
            .into_iter()
            .zip([new.luma(), new_cb, new_cr])
        {
            assert_eq!(
                old.rows().collect::<Vec<_>>(),
                new.rows().collect::<Vec<_>>()
            );
        }
    }
}
