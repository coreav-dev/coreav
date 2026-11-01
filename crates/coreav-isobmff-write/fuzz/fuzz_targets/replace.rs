//! Arbitrary replacements through the replaced-stream remux
//! (`docs/design/remux.md`, "Replaced streams"), over small
//! fixed sources: an MP4 movie with B-frame video and AAC (with an edit
//! list, or none), and a Matroska file with H.264 and AAC. The remux
//! either fails with an error or writes an output that the MP4 parser
//! accepts, whose replaced stream holds the replacement's samples (bytes,
//! sync flags, composition offsets) presented at the times the plain
//! remux presents the source's samples of the same times, and whose other
//! streams are the plain remux's.
#![no_main]

#[path = "../../../coreav-isobmff/tests/common/mod.rs"]
#[allow(dead_code)]
mod common;
#[path = "../../../coreav-mkv/tests/common/mod.rs"]
#[allow(dead_code)]
mod mkv;
#[path = "../../tests/movies/mod.rs"]
mod movies;

use coreav_isobmff::{Allowlist, Budget, Limits, Movie, Packet, SliceSource, Source, probe};
use coreav_isobmff_write::mux::avc1_sample_entry;
use coreav_isobmff_write::{
    MatroskaRemuxOptions, RemuxOptions, Replacement, ReplacementSample, Replacements,
    remux_matroska,
    remux_matroska_replacing, remux_mp4, remux_mp4_replacing,
};
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

const AVCC: [u8; 7] = [1, 100, 0, 30, 0xFF, 0xE0, 0];

/// A source and its plain remux.
struct Fixed {
    source: Vec<u8>,
    plain: Vec<u8>,
    mp4: bool,
}

fn sources() -> &'static [Fixed; 3] {
    static SOURCES: OnceLock<[Fixed; 3]> = OnceLock::new();
    SOURCES.get_or_init(|| {
        let with_edits = movies::av_movie();
        let without = movies::movie(&[
            movies::TrackSpec {
                edits: Vec::new(),
                ..movies::ibbp_video(30, 10)
            },
            movies::aac_audio(60, Vec::new()),
        ]);
        let matroska = matroska();
        let plain_mp4 = |source: &[u8]| {
            let mut out = Vec::new();
            remux_mp4(
                SliceSource::new(source),
                &Limits::default(),
                &Allowlist::default(),
                &RemuxOptions::default(),
                &mut out,
            )
            .expect("the plain remux");
            out
        };
        let mut plain_mkv = Vec::new();
        remux_matroska(
            SliceSource::new(&matroska),
            &coreav_mkv::Limits::default(),
            &Limits::default(),
            &coreav_mkv::Allowlist::default(),
            &MatroskaRemuxOptions::default(),
            &mut plain_mkv,
        )
        .expect("the plain Matroska remux");
        [
            Fixed {
                plain: plain_mp4(&with_edits),
                source: with_edits,
                mp4: true,
            },
            Fixed {
                plain: plain_mp4(&without),
                source: without,
                mp4: true,
            },
            Fixed {
                source: matroska,
                plain: plain_mkv,
                mp4: false,
            },
        ]
    })
}

/// H.264 and AAC in Matroska, starting 1 s into the segment.
fn matroska() -> Vec<u8> {
    use mkv::{cat, cluster, el, file, info, simple_block, uint};
    let mut blocks = Vec::new();
    for (n, t) in [0i16, 120, 40, 80, 240, 160, 200, 280]
        .into_iter()
        .enumerate()
    {
        blocks.push(simple_block(1, t, n == 0, &[n as u8; 20]));
        blocks.push(simple_block(2, n as i16 * 21, true, &[0x21; 9]));
    }
    let video = mkv::video_track(
        1,
        "V_MPEG4/ISO/AVC",
        64,
        48,
        &cat(&[el(0x63A2, &AVCC), uint(0x23_E383, 40_000_000)]),
    );
    let audio = mkv::audio_track(2, "A_AAC", 48_000.0, 2, &el(0x63A2, &[0x11, 0x90]));
    file(
        "matroska",
        &[
            info(None),
            el(mkv::TRACKS, &cat(&[video, audio])),
            cluster(1000, &blocks),
        ],
    )
}

fn packets(data: &[u8], movie: &Movie, index: usize) -> Vec<Packet> {
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    movie
        .packets(index, SliceSource::new(data), &limits, &mut budget)
        .expect("packets")
        .map(|p| p.expect("every packet is readable"))
        .collect()
}

fn bytes(data: &[u8], packet: &Packet) -> Vec<u8> {
    let mut buf = vec![0; packet.size as usize];
    SliceSource::new(data)
        .read_exact_at(packet.offset, &mut buf)
        .expect("packet bytes");
    buf
}

/// `pts - time` (media ticks), as a fraction (numerator, denominator).
fn lead(pts: coreav_isobmff::Rational, time: i64) -> (i128, i128) {
    (
        pts.num - i128::from(time) * i128::from(pts.den),
        i128::from(pts.den),
    )
}

fn same_fraction(a: (i128, i128), b: (i128, i128)) -> bool {
    a.0 * b.1 == b.0 * a.1
}

fuzz_target!(|data: &[u8]| {
    let Some((&choice, rest)) = data.split_first() else {
        return;
    };
    let fixed = &sources()[usize::from(choice % 3)];
    let stream_index = usize::from((choice >> 2) & 1);
    // Six bytes a sample: size, flags, decode step (two bytes),
    // composition offset (two bytes, signed).
    let mut samples = Vec::new();
    let mut time: i64 = i64::from(i8::from_be_bytes([choice])) * 97;
    for chunk in rest.chunks_exact(6).take(200) {
        let step = i64::from(u16::from_be_bytes([chunk[2], chunk[3]]));
        // Mostly increasing decode times; a step of 0 or "back" (high bit)
        // tests the refusals.
        time += if step & 0x8000 != 0 {
            -(step & 0xFF)
        } else {
            step
        };
        let offset = i64::from(i16::from_be_bytes([chunk[4], chunk[5]]));
        samples.push(ReplacementSample {
            size: u32::from(chunk[0]),
            sync: chunk[1] & 1 != 0,
            decode_time: time,
            presentation_time: time + offset,
        });
    }
    let total: usize = samples.iter().map(|s| s.size as usize).sum();
    // The data: each sample's bytes are its number; sometimes one byte short.
    let mut spool = Vec::with_capacity(total);
    for (n, s) in samples.iter().enumerate() {
        spool.extend(std::iter::repeat_n(n as u8, s.size as usize));
    }
    if choice & 0x80 != 0 {
        spool.pop();
    }
    let entry = avc1_sample_entry(64, 48, &AVCC, None, None).expect("entry");
    let replacement = Replacements {
        video: Some(Replacement {
            stream_index,
            sample_entry: &entry,
            width: 64 << 16,
            height: 48 << 16,
            samples: &samples,
            last_duration: u32::from(choice & 0x30) * 13,
            data: SliceSource::new(&spool),
        }),
        audio: None,
    };
    let limits = Limits {
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    let mut out = Vec::new();
    let replaced = if fixed.mp4 {
        remux_mp4_replacing(
            SliceSource::new(&fixed.source),
            &limits,
            &Allowlist::default(),
            &RemuxOptions::default(),
            replacement,
            &mut out,
        )
        .map(|s| (s.bytes_written, s.tracks.iter().position(|t| t.replaced)))
        .map_err(drop)
    } else {
        remux_matroska_replacing(
            SliceSource::new(&fixed.source),
            &coreav_mkv::Limits::default(),
            &limits,
            &coreav_mkv::Allowlist::default(),
            &MatroskaRemuxOptions::default(),
            replacement,
            &mut out,
        )
        .map(|s| (s.bytes_written, s.tracks.iter().position(|t| t.replaced)))
        .map_err(drop)
    };
    let Ok((written, Some(index))) = replaced else {
        return;
    };
    assert_eq!(written, out.len() as u64);
    assert_eq!(&out[4..8], b"ftyp");
    // Everything an accepted replacement must be.
    assert!(!samples.is_empty());
    assert!(spool.len() >= total);
    for pair in samples.windows(2) {
        assert!(pair[1].decode_time > pair[0].decode_time);
    }
    assert!(samples.iter().all(|s| s.presentation_time >= s.decode_time));
    let movie = probe(
        SliceSource::new(&out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .expect("the output parses");
    let plain = probe(
        SliceSource::new(&fixed.plain),
        &Limits::default(),
        &Allowlist::default(),
    )
    .expect("the plain remux parses");
    assert_eq!(movie.tracks.len(), plain.tracks.len());
    let written = packets(&out, &movie, index);
    assert_eq!(written.len(), samples.len());
    let mut at = 0usize;
    for (p, s) in written.iter().zip(&samples) {
        assert_eq!(bytes(&out, p), spool[at..at + s.size as usize]);
        at += s.size as usize;
        assert_eq!(p.keyframe, s.sync);
        assert_eq!(p.cts - p.dts, s.presentation_time - s.decode_time);
    }
    // Presented frames: at the plain remux's time for the same source
    // time (media ticks of the presentation timeline). MP4: the plain
    // remux presents composition time C at C plus a constant, which the
    // replacement keeps. Matroska: Segment Ticks of 1 ms, presented at
    // that time.
    let source_lead = packets(&fixed.plain, &plain, index)
        .iter()
        .find(|p| p.presented)
        .and_then(|p| p.pts.map(|pts| lead(pts, p.cts)));
    for (p, s) in written.iter().zip(&samples) {
        let (Some(pts), true) = (p.pts, p.presented) else {
            continue;
        };
        if fixed.mp4 {
            if let Some(expected) = source_lead {
                assert!(same_fraction(lead(pts, s.presentation_time), expected));
            }
        } else {
            assert_eq!(
                pts.num,
                i128::from(s.presentation_time) * i128::from(pts.den)
            );
        }
    }
    // The other streams: as the plain remux writes them.
    for other in (0..movie.tracks.len()).filter(|i| *i != index) {
        let a = packets(&fixed.plain, &plain, other);
        let b = packets(&out, &movie, other);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(
                (x.dts, x.cts, x.duration, x.keyframe),
                (y.dts, y.cts, y.duration, y.keyframe)
            );
            assert_eq!(x.pts, y.pts);
            assert_eq!(bytes(&fixed.plain, x), bytes(&out, y));
        }
    }
});
