//! The decoder's x86-64-v3 instance of the hot paths
//! (`src/fast/`) and its baseline twin decode the same bytes to the same
//! pictures, sample for sample and in the same order, with the same error
//! from every call (`docs/design/h264.md`, "Kernel round: the hot paths
//! compiled twice"). On a CPU without the v3 features both decoders run
//! the baseline instance, and the target compares it with itself.
//! The v3 instance decodes each regular CABAC bin with the site `decision`
//! and the baseline with its twin, `Engine::decision`, so the target also
//! compares those two ("Entropy round: DecodeDecision in assembly").
//!
//! The first byte picks the mode and whether the caller ends access units
//! itself, as for the `decode` target.
#![no_main]

use coreav_h264::{Decoder, Error, Limits, Mode, annex_b};
use libfuzzer_sys::fuzz_target;

/// Macroblocks per picture, as for the `decode` target.
const MAX_MBS: usize = 32 * 32;

/// NAL units decoded from one input.
const MAX_UNITS: usize = 512;

/// What one call returned or one picture held, in order.
#[derive(Debug, PartialEq)]
enum Event {
    Call(Result<(), Error>),
    Picture(u64, u64),
}

fn decode(data: &[u8], choice: u8, v3: bool) -> Vec<Event> {
    let limits = Limits {
        max_mbs: MAX_MBS,
        ..Limits::default()
    };
    let mode = if choice % 2 == 0 {
        Mode::EveryPicture
    } else {
        Mode::KeyframesOnly
    };
    let mut decoder = Decoder::new(limits).with_mode(mode).with_x86_64_v3(v3);
    let mut out = Vec::new();
    let take = |decoder: &mut Decoder, out: &mut Vec<Event>| {
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
            out.push(Event::Picture(picture.mark(), hash));
        }
    };
    for (index, nal) in annex_b(data).take(MAX_UNITS).enumerate() {
        decoder.set_mark(index as u64);
        out.push(Event::Call(decoder.decode_nal(nal)));
        if choice % 4 == 3 && index % 3 == 2 {
            out.push(Event::Call(decoder.end_access_unit()));
        }
        take(&mut decoder, &mut out);
    }
    out.push(Event::Call(decoder.flush()));
    take(&mut decoder, &mut out);
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&choice, data)) = data.split_first() else {
        return;
    };
    assert_eq!(decode(data, choice, true), decode(data, choice, false));
});
