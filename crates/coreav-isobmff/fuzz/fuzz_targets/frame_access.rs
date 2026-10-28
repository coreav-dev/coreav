//! Arbitrary bytes through the random access readers
//! (docs/design/random-access.md, docs/design/cut-points.md): as a file,
//! probed and every H.264 track's packets read with `Packets::frame_access`;
//! as one frame of length-prefixed NAL units through
//! `access::read_frame_access` and `access::read_starts_decoding`; or as
//! one AV1 sample through `access::read_av1_sync` and
//! `access::read_av1_frame_start` (and as an `av1C` payload); each with a work limit.
//! Every result is an answer or `None`, never a panic.
#![no_main]

use coreav_isobmff::access::{self, AccessFormat};
use coreav_isobmff::{Allowlist, Budget, Limits, SliceSource, probe};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let len = rest.len() as u64;
    let mut work = 0u32;
    let mut charge = || {
        work += 1;
        if work > 10_000 {
            Err(coreav_isobmff::Error::Overflow { offset: 0 })
        } else {
            Ok(())
        }
    };
    if mode % 4 != 0 {
        let format = AccessFormat::H264 {
            length_size: [1, 2, 4][usize::from(mode % 4) - 1],
        };
        let source = &mut SliceSource::new(rest);
        if mode & 0x80 == 0 {
            let _ = access::read_frame_access(format, source, 0, len, &mut charge);
        } else {
            let refs = Some(u32::from(mode >> 4 & 0x07));
            let _ = access::read_starts_decoding(format, source, 0, len, refs, &mut charge);
        }
        return;
    }
    if mode & 0x80 != 0 {
        let _ = access::read_av1_sync(&mut SliceSource::new(rest), 0, len, &mut charge);
        let mut reduced = (mode & 0x40 != 0).then_some(mode & 0x20 != 0);
        let _ = access::read_av1_frame_start(&mut SliceSource::new(rest), 0, len, &mut reduced, &mut charge);
        let _ = access::av1_config_reduced(rest);
        return;
    }
    let limits = Limits {
        max_sample_work: 1 << 20,
        ..Limits::default()
    };
    let Ok(movie) = probe(SliceSource::new(rest), &limits, &Allowlist::default()) else {
        return;
    };
    let mut budget = Budget::new(&limits);
    for track in &movie.tracks {
        let Some(format) = movie.access_format(track.index) else {
            continue;
        };
        let Ok(mut packets) =
            movie.packets(track.index, SliceSource::new(rest), &limits, &mut budget)
        else {
            continue;
        };
        while let Some(Ok(packet)) = packets.next() {
            if packets.frame_access(format, &packet).is_err() {
                break;
            }
        }
    }
});
