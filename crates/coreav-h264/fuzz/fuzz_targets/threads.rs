//! Bar G1 of step 7 (`docs/design/h264.md`): the same bytes decoded with
//! one thread and with three give the same pictures, sample for sample
//! and in the same order, for a caller that carries on after errors.
//!
//! The first byte picks the mode and whether the caller ends access units
//! itself, as for the `decode` target, and the band height: the
//! three-thread decoder cuts its pictures into bands of one or two
//! macroblock rows whatever their height, so that small pictures are
//! banded too.
#![no_main]

use coreav_h264::{Decoder, Limits, Mode, annex_b};
use libfuzzer_sys::fuzz_target;

/// Macroblocks per picture, as for the `decode` target.
const MAX_MBS: usize = 32 * 32;

/// NAL units decoded from one input.
const MAX_UNITS: usize = 512;

/// Each picture's mark and a hash of its samples, in order.
fn decode(data: &[u8], choice: u8, threads: usize) -> Vec<(u64, u64)> {
    let limits = Limits {
        max_mbs: MAX_MBS,
        ..Limits::default()
    };
    let mode = if choice % 2 == 0 {
        Mode::EveryPicture
    } else {
        Mode::KeyframesOnly
    };
    let mut decoder = Decoder::new(limits).with_mode(mode).with_threads(threads);
    if threads > 1 {
        decoder = decoder.with_band_height(if choice & 4 == 0 { 2 } else { 1 });
    }
    let mut out = Vec::new();
    let mut take = |decoder: &mut Decoder| {
        for picture in decoder.take_pictures() {
            let mut hash = 0xcbf2_9ce4_8422_2325u64;
            let (cb, cr) = picture.chroma();
            for rows in [picture.luma().rows(), cb.rows(), cr.rows()] {
                for row in rows {
                    for &s in row {
                        hash = (hash ^ u64::from(s)).wrapping_mul(0x0100_0000_01b3);
                    }
                }
            }
            out.push((picture.mark(), hash));
        }
    };
    for (index, nal) in annex_b(data).take(MAX_UNITS).enumerate() {
        decoder.set_mark(index as u64);
        let _ = decoder.decode_nal(nal);
        if choice % 4 == 3 && index % 3 == 2 {
            let _ = decoder.end_access_unit();
        }
        take(&mut decoder);
    }
    let _ = decoder.flush();
    take(&mut decoder);
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&choice, data)) = data.split_first() else {
        return;
    };
    assert_eq!(decode(data, choice, 1), decode(data, choice, 3));
});
