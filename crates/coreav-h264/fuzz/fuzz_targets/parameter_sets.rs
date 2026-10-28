//! Arbitrary bytes as a sequence or picture parameter set, then as the
//! slices that activate them. The parameter sets are the part of the
//! syntax a stream can repeat cheaply, and the one whose values size
//! everything else.
#![no_main]

use coreav_h264::{Decoder, Limits};
use libfuzzer_sys::fuzz_target;

/// A NAL unit with its header.
fn nal(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(3 << 5) | kind];
    out.extend_from_slice(payload);
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&split, rest)) = data.split_first() else {
        return;
    };
    // The first part is a sequence parameter set, the second a picture
    // parameter set, and the rest a slice of each kind.
    let at = usize::from(split).min(rest.len());
    let (sps, tail) = rest.split_at(at);
    let at = (tail.len() / 2).min(tail.len());
    let (pps, slice) = tail.split_at(at);
    let mut decoder = Decoder::new(Limits {
        max_mbs: 64 * 64,
        ..Limits::default()
    });
    let _ = decoder.decode_nal(&nal(7, sps));
    let _ = decoder.decode_nal(&nal(8, pps));
    let _ = decoder.decode_nal(&nal(5, slice));
    let _ = decoder.decode_nal(&nal(1, slice));
    let _ = decoder.flush();
    let _ = decoder.take_pictures();
});
