//! Arbitrary bytes as an Annex B stream through the whole decoder. Every
//! NAL unit is decoded, every picture taken, and the stream flushed: the
//! result is pictures or errors, never a panic, an unbounded allocation
//! or an endless loop.
//!
//! The first byte picks the mode, so that the keyframe-only path — which
//! skips pictures, restarts the decoder at each keyframe and ends access
//! units of its own — is fuzzed beside the full one.
#![no_main]

use coreav_h264::{Decoder, Limits, Mode, annex_b};
use libfuzzer_sys::fuzz_target;

/// Macroblocks per picture: 1,024 is a 512x512 picture, or any other
/// shape of the same area. It bounds the work of one picture, which the
/// 60 seconds CI gives a target has to fit many of.
const MAX_MBS: usize = 32 * 32;

/// NAL units decoded from one input, which bounds the work of the whole
/// input the way `MAX_MBS` bounds one picture's.
const MAX_UNITS: usize = 512;

fuzz_target!(|data: &[u8]| {
    let Some((&choice, data)) = data.split_first() else {
        return;
    };
    let limits = Limits {
        max_mbs: MAX_MBS,
        ..Limits::default()
    };
    let mode = if choice % 2 == 0 {
        Mode::EveryPicture
    } else {
        Mode::KeyframesOnly
    };
    let mut decoder = Decoder::new(limits).with_mode(mode);
    let mut pictures = 0usize;
    for (index, nal) in annex_b(data).take(MAX_UNITS).enumerate() {
        let _ = decoder.decode_nal(nal);
        // The caller's own access unit boundaries, which a decoder behind
        // the decode boundary uses, every few units.
        if choice % 4 == 3 && index % 3 == 2 {
            let _ = decoder.end_access_unit();
        }
        for picture in decoder.take_pictures() {
            // No picture is larger than the limit it was decoded under,
            // whatever shape it has.
            let macroblocks = picture.width().div_ceil(16) * picture.height().div_ceil(16);
            assert!(macroblocks <= MAX_MBS, "{} macroblocks", macroblocks);
            // Every picture is readable to its last sample.
            let (cb, cr) = picture.chroma();
            assert_eq!(picture.luma().rows().count(), picture.height());
            assert_eq!(cb.rows().count(), cr.rows().count());
            pictures += 1;
        }
        // A stream of this size cannot make more pictures than it has
        // bytes.
        assert!(pictures <= data.len() + 1);
    }
    let _ = decoder.flush();
    let _ = decoder.take_pictures();
    let _ = decoder.warnings();
});
