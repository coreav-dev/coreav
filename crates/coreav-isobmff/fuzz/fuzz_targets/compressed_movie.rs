//! Arbitrary bytes through QuickTime compressed movie boxes
//! (docs/design/compressed-movie.md): as the payload of the movie box a
//! compressed movie holds, compressed with zlib here so the inflated walk
//! is reached; or as the payload of the compressed movie box itself (its
//! `dcom` and `cmvd`, zlib data included). Each file is probed under tight
//! movie limits and its packets listed; every result is a movie or an
//! error, never a panic.
#![no_main]

use coreav_isobmff::{Allowlist, Budget, Limits, SliceSource, probe};
use libfuzzer_sys::fuzz_target;

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let cmov = if mode % 2 == 0 {
        let inner = bx(b"moov", rest);
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&inner, 1);
        let mut cmvd = (inner.len() as u32).to_be_bytes().to_vec();
        cmvd.extend_from_slice(&packed);
        [bx(b"dcom", b"zlib"), bx(b"cmvd", &cmvd)].concat()
    } else {
        rest.to_vec()
    };
    let file = [
        bx(b"ftyp", b"qt  \0\0\0\0qt  "),
        bx(b"moov", &bx(b"cmov", &cmov)),
    ]
    .concat();
    let limits = Limits {
        max_movie_bytes: 1 << 20,
        max_sample_work: 1 << 20,
        ..Limits::default()
    };
    let Ok(movie) = probe(SliceSource::new(&file), &limits, &Allowlist::default()) else {
        return;
    };
    assert!(movie.compressed, "a movie read through cmov says so");
    let mut budget = Budget::new(&limits);
    for track in &movie.tracks {
        if let Ok(packets) =
            movie.packets(track.index, SliceSource::new(&file), &limits, &mut budget)
        {
            for packet in packets {
                if packet.is_err() {
                    break;
                }
            }
        }
    }
});
