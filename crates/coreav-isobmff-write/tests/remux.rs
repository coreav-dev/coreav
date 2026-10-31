//! Full-file remux round trips (`docs/design/remux.md`): every output is
//! probed by `coreav-isobmff`; each kept stream's samples, times and edit
//! list must equal the source's, the movie box must come first, and every
//! box left out must be reported.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use coreav_isobmff::{
    Allowlist, Budget, Codec, EditList, Error, FourCc, LimitKind, Limits, Movie, Packet,
    SliceSource, Source, TrackKind, probe,
};
use coreav_isobmff_write::{
    BoxDropReason, Cleared, ClearedField, ExtractError, Keep, Metadata, MetadataKind,
    OnUnsupported, RemuxOptions, RemuxSummary, TrackDropReason, remux_mp4, sanitize_mp4,
};
use std::io::{self, Write};
use std::path::Path;

use common::{bx, cat, full, u32s};

// ------------------------------------------------------------- builders

#[derive(Clone, Copy)]
struct S {
    size: u32,
    duration: u32,
    offset: i32,
    sync: bool,
}

#[derive(Clone)]
enum Kind {
    /// H.264 with this sample entry type.
    Video([u8; 4]),
    /// AAC; QuickTime version 1 sound entry with the `esds` in `wave` and a
    /// `chan` box when `quicktime`.
    Aac { quicktime: bool },
    /// Little-endian 16-bit PCM (`sowt`).
    Pcm,
    /// An audio sample entry of this type holding this configuration box.
    Sound([u8; 4], Vec<u8>),
    /// QuickTime timecode.
    Timecode,
}

#[derive(Clone)]
struct T {
    kind: Kind,
    timescale: u32,
    samples: Vec<S>,
    /// `(duration in movie ticks, media time, rate)`.
    edits: Vec<(u32, i32, u32)>,
    /// The media header's language field as stored.
    language: u16,
    /// Boxes appended to the `trak`, `stbl` and the sample entry.
    trak_extra: Vec<Vec<u8>>,
    stbl_extra: Vec<Vec<u8>>,
    entry_extra: Vec<Vec<u8>>,
    /// Track header flags, layer and alternate group.
    flags: u32,
    alternate_group: u16,
    /// A version 1 track header.
    tkhd_v1: bool,
    /// `hdlr` after `minf` in `mdia`.
    hdlr_last: bool,
    /// Boxes appended inside QuickTime's `wave`.
    wave_extra: Vec<Vec<u8>>,
    /// The visual entry's depth.
    depth: u16,
}

struct F {
    major: [u8; 4],
    tracks: Vec<T>,
    /// Boxes after `ftyp`, and at the end of `moov`.
    top_extra: Vec<Vec<u8>>,
    moov_extra: Vec<Vec<u8>>,
    mdat_first: bool,
}

const UND: u16 = 0x55c4;

fn video(frames: usize, gop: usize) -> T {
    let samples = (0..frames)
        .map(|n| {
            let in_gop = n % gop;
            S {
                size: 40 + (n as u32 % 13),
                duration: 512,
                offset: match in_gop {
                    0 => 1024,
                    k if k % 3 == 1 => 2048,
                    _ => 512,
                },
                sync: in_gop == 0,
            }
        })
        .collect();
    T {
        kind: Kind::Video(*b"avc1"),
        timescale: 12_800,
        samples,
        edits: vec![((frames as u32) * 40, 1024, 0x10000)],
        language: UND,
        trak_extra: Vec::new(),
        stbl_extra: Vec::new(),
        entry_extra: vec![bx(b"pasp", &u32s(&[1, 1])), bx(b"fiel", &[1, 0])],
        flags: 3,
        alternate_group: 0,
        tkhd_v1: false,
        hdlr_last: false,
        wave_extra: Vec::new(),
        depth: 0x18,
    }
}

fn aac(frames: usize, quicktime: bool) -> T {
    T {
        kind: Kind::Aac { quicktime },
        timescale: 48_000,
        samples: (0..frames)
            .map(|n| S {
                size: 20 + (n as u32 % 5),
                duration: 1024,
                offset: 0,
                sync: true,
            })
            .collect(),
        edits: vec![(1000 * frames as u32 * 1024 / 48_000 - 44, 2112, 0x10000)],
        language: UND,
        trak_extra: Vec::new(),
        stbl_extra: Vec::new(),
        entry_extra: Vec::new(),
        flags: 3,
        alternate_group: 1,
        tkhd_v1: false,
        hdlr_last: false,
        wave_extra: Vec::new(),
        depth: 0x18,
    }
}

fn other(kind: Kind, n: usize) -> T {
    let (timescale, duration, size) = match kind {
        Kind::Pcm => (48_000, 1024, 4096),
        _ => (25, 100, 4),
    };
    T {
        kind,
        timescale,
        samples: (0..n)
            .map(|_| S {
                size,
                duration,
                offset: 0,
                sync: true,
            })
            .collect(),
        edits: Vec::new(),
        language: UND,
        trak_extra: Vec::new(),
        stbl_extra: Vec::new(),
        entry_extra: Vec::new(),
        flags: 3,
        alternate_group: 0,
        tkhd_v1: false,
        hdlr_last: false,
        wave_extra: Vec::new(),
        depth: 0x18,
    }
}

fn file(tracks: Vec<T>) -> F {
    F {
        major: *b"isom",
        tracks,
        top_extra: Vec::new(),
        moov_extra: Vec::new(),
        mdat_first: true,
    }
}

fn esds() -> Vec<u8> {
    full(
        b"esds",
        0,
        0,
        &[
            3, 25, 0, 1, 0, 4, 17, 0x40, 0x15, 0, 0, 0, 0, 0, 0xfa, 0, 0, 0, 0xfa, 0, 5, 2, 0x11,
            0x90, 6, 1, 2,
        ],
    )
}

fn sample_entry(t: &T) -> Vec<u8> {
    let sound = |kind: &[u8; 4], version: u16, extra: &[Vec<u8>]| {
        let mut body = vec![0u8; 6];
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&version.to_be_bytes());
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&16u16.to_be_bytes());
        body.extend_from_slice(&[0u8; 4]);
        body.extend_from_slice(&(t.timescale << 16).to_be_bytes());
        if version == 1 {
            body.extend_from_slice(&u32s(&[1024, 0, 0, 2]));
        }
        for e in extra {
            body.extend_from_slice(e);
        }
        for e in &t.entry_extra {
            body.extend_from_slice(e);
        }
        bx(kind, &body)
    };
    match &t.kind {
        Kind::Video(kind) => {
            let mut body = vec![0u8; 6];
            body.extend_from_slice(&1u16.to_be_bytes());
            // QuickTime version, revision and vendor.
            body.extend_from_slice(&[0, 0, 0, 0, b'a', b'p', b'p', b'l']);
            body.extend_from_slice(&[0u8; 8]);
            body.extend_from_slice(&320u16.to_be_bytes());
            body.extend_from_slice(&240u16.to_be_bytes());
            body.extend_from_slice(&[0u8; 14]);
            // A compressor name.
            let mut name = [0u8; 32];
            name[0] = 5;
            name[1..6].copy_from_slice(b"H.264");
            body.extend_from_slice(&name);
            body.extend_from_slice(&t.depth.to_be_bytes());
            body.extend_from_slice(&[0xff, 0xff]);
            body.extend_from_slice(&bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]));
            for e in &t.entry_extra {
                body.extend_from_slice(e);
            }
            bx(kind, &body)
        }
        Kind::Aac { quicktime: false } => sound(b"mp4a", 0, &[esds()]),
        Kind::Aac { quicktime: true } => {
            let mut frma = b"mp4a".to_vec();
            frma.splice(0..0, [0, 0, 0, 12, b'f', b'r', b'm', b'a']);
            let wave = bx(
                b"wave",
                &cat(&[vec![frma, esds()], t.wave_extra.clone(), vec![vec![0; 8]]].concat()),
            );
            let chan = full(b"chan", 0, 0, &u32s(&[0x0065_0002, 0, 0]));
            sound(b"mp4a", 1, &[wave, chan])
        }
        Kind::Pcm => sound(b"sowt", 0, &[]),
        Kind::Sound(kind, config) => sound(kind, 0, std::slice::from_ref(config)),
        Kind::Timecode => bx(b"tmcd", &[0u8; 26]),
    }
}

fn handler(t: &T) -> &'static [u8; 4] {
    match t.kind {
        Kind::Video(_) => b"vide",
        Kind::Aac { .. } | Kind::Pcm | Kind::Sound(..) => b"soun",
        Kind::Timecode => b"tmcd",
    }
}

fn trak(t: &T, id: u32, chunk_offset: u32) -> Vec<u8> {
    let n = t.samples.len() as u32;
    let media: u32 = t.samples.iter().map(|s| s.duration).sum();
    let mut tkhd = if t.tkhd_v1 {
        let mut v1 = vec![
            0, 0, 0, 0, 0x7c, 0x25, 0xb0, 0x80, 0, 0, 0, 0, 0x7c, 0x25, 0xb0, 0x80,
        ];
        v1.extend_from_slice(&u32s(&[id, 0, 0, 0]));
        v1
    } else {
        u32s(&[0x7c25_b080, 0x7c25_b080, id, 0, 0])
    };
    tkhd.extend_from_slice(&[0u8; 8]);
    tkhd.extend_from_slice(&0u16.to_be_bytes());
    tkhd.extend_from_slice(&t.alternate_group.to_be_bytes());
    tkhd.extend_from_slice(&[1, 0, 0, 0]);
    tkhd.extend_from_slice(&u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]));
    let video = matches!(t.kind, Kind::Video(_));
    tkhd.extend_from_slice(&u32s(if video {
        &[320 << 16, 240 << 16]
    } else {
        &[0, 0]
    }));
    let mut mdhd = u32s(&[0x7c25_b081, 0x7c25_b081, t.timescale, media]);
    mdhd.extend_from_slice(&t.language.to_be_bytes());
    mdhd.extend_from_slice(&[0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(handler(t));
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&sample_entry(t));
    let mut stts = Vec::new();
    for s in &t.samples {
        stts.extend_from_slice(&u32s(&[1, s.duration]));
    }
    let mut stsz = u32s(&[0, n]);
    for s in &t.samples {
        stsz.extend_from_slice(&s.size.to_be_bytes());
    }
    let mut stbl = vec![
        full(b"stsd", 0, 0, &stsd),
        full(b"stts", 0, 0, &[u32s(&[n]), stts].concat()),
    ];
    if t.samples.iter().any(|s| s.offset != 0) {
        let mut ctts = u32s(&[n]);
        for s in &t.samples {
            ctts.extend_from_slice(&u32s(&[1, s.offset as u32]));
        }
        let version = u8::from(t.samples.iter().any(|s| s.offset < 0));
        stbl.push(full(b"ctts", version, 0, &ctts));
    }
    stbl.push(full(b"stsc", 0, 0, &u32s(&[1, 1, n, 1])));
    stbl.push(full(b"stsz", 0, 0, &stsz));
    stbl.push(full(b"stco", 0, 0, &u32s(&[1, chunk_offset])));
    if t.samples.iter().any(|s| !s.sync) {
        let sync: Vec<u32> = (1..=n)
            .filter(|i| t.samples[*i as usize - 1].sync)
            .collect();
        stbl.push(full(
            b"stss",
            0,
            0,
            &[u32s(&[sync.len() as u32]), u32s(&sync)].concat(),
        ));
    }
    stbl.extend(t.stbl_extra.iter().cloned());
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let media_header = match t.kind {
        Kind::Video(_) => full(b"vmhd", 0, 1, &[0; 8]),
        Kind::Aac { .. } | Kind::Pcm | Kind::Sound(..) => full(b"smhd", 0, 0, &[0; 4]),
        Kind::Timecode => full(b"nmhd", 0, 0, &[]),
    };
    let minf = bx(
        b"minf",
        &cat(&[
            media_header,
            bx(b"dinf", &dref),
            bx(b"stbl", &stbl.concat()),
        ]),
    );
    let mdia = if t.hdlr_last {
        bx(
            b"mdia",
            &cat(&[full(b"mdhd", 0, 0, &mdhd), minf, full(b"hdlr", 0, 0, &hdlr)]),
        )
    } else {
        bx(
            b"mdia",
            &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
        )
    };
    let mut parts = vec![full(b"tkhd", u8::from(t.tkhd_v1), t.flags, &tkhd)];
    if !t.edits.is_empty() {
        let mut elst = u32s(&[t.edits.len() as u32]);
        for (duration, time, rate) in &t.edits {
            elst.extend_from_slice(&u32s(&[*duration, *time as u32, *rate]));
        }
        parts.push(bx(b"edts", &full(b"elst", 0, 0, &elst)));
    }
    parts.push(mdia);
    parts.extend(t.trak_extra.iter().cloned());
    bx(b"trak", &parts.concat())
}

/// The file: `ftyp`, the extra top-level boxes, then `mdat` and `moov` in
/// the order asked, each track's samples in one chunk.
fn build(f: &F) -> Vec<u8> {
    let mut brands = f.major.to_vec();
    brands.extend_from_slice(&[0, 0, 2, 0]);
    brands.extend_from_slice(&f.major);
    let head = cat(&[vec![bx(b"ftyp", &brands)], f.top_extra.clone()].concat());
    let moov = |offsets: &[u32]| -> Vec<u8> {
        let mut mvhd = u32s(&[0x7c25_b07f, 0x7c25_b07f, 1000, 10_000]);
        mvhd.extend_from_slice(&[0u8; 80]);
        let mut body = full(b"mvhd", 0, 0, &mvhd);
        for (i, t) in f.tracks.iter().enumerate() {
            body.extend_from_slice(&trak(t, i as u32 + 1, offsets[i]));
        }
        for extra in &f.moov_extra {
            body.extend_from_slice(extra);
        }
        bx(b"moov", &body)
    };
    let moov_len = moov(&vec![0; f.tracks.len()]).len() as u32;
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    let mut at = head.len() as u32 + 8 + if f.mdat_first { 0 } else { moov_len };
    for (i, t) in f.tracks.iter().enumerate() {
        offsets.push(at);
        for (n, s) in t.samples.iter().enumerate() {
            data.extend((0..s.size).map(|b| (b as usize + n * 7 + i * 101) as u8));
            at += s.size;
        }
    }
    let mdat = bx(b"mdat", &data);
    let moov = moov(&offsets);
    if f.mdat_first {
        cat(&[head, mdat, moov])
    } else {
        cat(&[head, moov, mdat])
    }
}

// ------------------------------------------------------------- checks

fn remux(source: &[u8], options: &RemuxOptions) -> Result<(RemuxSummary, Vec<u8>), ExtractError> {
    let mut out = Vec::new();
    let summary = remux_mp4(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        options,
        &mut out,
    )?;
    assert_eq!(summary.bytes_written, out.len() as u64);
    Ok((summary, out))
}

fn packets(data: &[u8], movie: &Movie, index: usize) -> Vec<Packet> {
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    movie
        .packets(index, SliceSource::new(data), &limits, &mut budget)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn bytes(data: &[u8], packet: &Packet) -> Vec<u8> {
    let mut buf = vec![0; packet.size as usize];
    SliceSource::new(data)
        .read_exact_at(packet.offset, &mut buf)
        .unwrap();
    buf
}

/// Top-level box types, in order.
fn top_level(data: &[u8]) -> Vec<[u8; 4]> {
    let mut at = 0;
    let mut kinds = Vec::new();
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let size = if size == 1 {
            u64::from_be_bytes(data[at + 8..at + 16].try_into().unwrap()) as usize
        } else {
            size
        };
        kinds.push(data[at + 4..at + 8].try_into().unwrap());
        at += size;
    }
    kinds
}

/// The output has the movie box first, and each kept stream's samples,
/// times, edit list, language and codec configuration equal the source's.
fn assert_round_trip(source: &[u8], output: &[u8], summary: &RemuxSummary) {
    assert_copied(source, output, summary, None);
}

/// [`assert_round_trip`] for an output sanitized keeping `keep`: the
/// matrix, language and creation times follow `keep`.
/// An HEVC configuration (ISO/IEC 14496-15 8.3.3.1) with only its VPS, SPS
/// and PPS arrays.
fn hevc_parameter_sets(config: &[u8]) -> Vec<u8> {
    let mut out = config[..22].to_vec();
    let mut arrays = Vec::new();
    let mut at = 23;
    for _ in 0..config[22] {
        let start = at;
        let nal_type = config[at] & 0x3f;
        let count = u16::from_be_bytes([config[at + 1], config[at + 2]]);
        at += 3;
        for _ in 0..count {
            at += 2 + usize::from(u16::from_be_bytes([config[at], config[at + 1]]));
        }
        if (32..=34).contains(&nal_type) {
            arrays.push(&config[start..at]);
        }
    }
    out.push(arrays.len() as u8);
    out.extend(arrays.concat());
    out
}

/// An H.264 configuration (ISO/IEC 14496-15 5.3.3.1.2) cut after its PPS
/// array when its profile carries no chroma format and bit depth fields.
fn avc_without_extras(config: &mut Vec<u8>) {
    if matches!(config[1], 100 | 110 | 122 | 144) {
        return;
    }
    let mut at = 6;
    for mask in [0x1f, 0xff] {
        let count = config[at - 1] & mask;
        for _ in 0..count {
            at += 2 + usize::from(u16::from_be_bytes([config[at], config[at + 1]]));
        }
        at += 1;
    }
    config.truncate(at - 1);
}

fn assert_copied(source: &[u8], output: &[u8], summary: &RemuxSummary, keep: Option<Keep>) {
    assert_eq!(top_level(output), [*b"ftyp", *b"moov", *b"mdat"]);
    let limits = Limits::default();
    let src = probe(SliceSource::new(source), &limits, &Allowlist::default()).unwrap();
    let out = probe(SliceSource::new(output), &limits, &Allowlist::default()).unwrap();
    assert_eq!(out.tracks.len(), summary.tracks.len());
    assert!(!out.fragmented);
    if src.has_fragments() {
        // A common multiple of the movie and media timescales.
        assert_eq!(out.timescale % src.timescale, 0);
    } else {
        assert_eq!(out.timescale, src.timescale);
    }
    for (i, kept) in summary.tracks.iter().enumerate() {
        let s = src
            .tracks
            .iter()
            .find(|t| t.stream_index == kept.stream_index)
            .unwrap();
        let o = &out.tracks[i];
        assert_eq!(o.track_id, kept.track_id);
        assert_eq!(kept.track_id, i as u32 + 1);
        assert_eq!(kept.source_track_id, s.track_id);
        assert_eq!(o.kind, s.kind);
        assert_eq!(o.timescale, s.timescale);
        assert_eq!(o.description.codec, s.description.codec);
        let mut config = s.description.codec_config.clone();
        let mut entry_boxes = s.description.entry_boxes.clone();
        if keep.is_some() {
            // Sanitized: an HEVC configuration keeps only its parameter
            // sets, an H.264 one nothing past the fields of its profile, a
            // `colr` only coded colour parameters.
            if let Some(c) = config.as_mut().filter(|c| &c.kind.0 == b"hvcC") {
                c.data = hevc_parameter_sets(&c.data);
            }
            if let Some(c) = config.as_mut().filter(|c| &c.kind.0 == b"avcC") {
                avc_without_extras(&mut c.data);
            }
            entry_boxes
                .retain(|b| &b.kind.0 != b"colr" || matches!(&b.data[..4], b"nclx" | b"nclc"));
        }
        assert_eq!(o.description.codec_config, config);
        assert_eq!(o.description.entry_boxes, entry_boxes);
        assert_eq!(o.description.channels, s.description.channels);
        assert_eq!(o.description.sample_rate, s.description.sample_rate);
        let keep = keep.unwrap_or(Keep {
            rotation: true,
            language: true,
            creation_times: true,
            gapless: true,
            kinds: true,
        });
        let unity = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
        assert_eq!(o.matrix, if keep.rotation { s.matrix } else { unity });
        assert_eq!(o.presentation_size, s.presentation_size);
        let created = |t: u64| if keep.creation_times { t } else { 0 };
        assert_eq!(o.creation_time, created(s.creation_time));
        assert_eq!(o.media_creation_time, created(s.media_creation_time));
        if let Some(code) = s.language {
            let expected = if code.iter().all(u8::is_ascii_lowercase) && keep.language {
                code
            } else {
                *b"und"
            };
            assert_eq!(o.language, Some(expected));
        }
        if !src.has_fragments() {
            assert_eq!(o.mdhd_duration, s.mdhd_duration);
        }
        let from = packets(source, &src, s.index);
        let to = packets(output, &out, o.index);
        assert_eq!(to.len(), from.len());
        assert_eq!(kept.samples as usize, from.len());
        let first = from[0].dts;
        if first == 0 && !src.has_fragments() {
            assert_eq!(o.edit_list, s.edit_list);
        }
        for (a, b) in from.iter().zip(&to) {
            assert_eq!(b.size, a.size);
            assert_eq!(b.duration, a.duration);
            assert_eq!(b.keyframe, a.keyframe);
            assert_eq!(b.dts, a.dts - first);
            assert_eq!(b.cts - b.dts, a.cts - a.dts);
            if a.pts.is_some() {
                assert_eq!(b.pts, a.pts, "sample {}", a.sample);
                assert_eq!(b.presented, a.presented);
            }
            assert_eq!(bytes(output, b), bytes(source, a));
        }
    }
}

fn paths(summary: &RemuxSummary) -> Vec<String> {
    summary
        .dropped_boxes
        .iter()
        .map(|b| {
            b.path
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

// ------------------------------------------------------------- tests

#[test]
fn the_movie_box_moves_to_the_front() {
    let source = build(&file(vec![video(50, 25), aac(95, false)]));
    assert_eq!(top_level(&source), [*b"ftyp", *b"mdat", *b"moov"]);
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    assert!(!summary.source_moov_first);
    assert_eq!(summary.tracks.len(), 2);
    assert_eq!(summary.tracks[0].kind, TrackKind::Video);
    assert_eq!(summary.tracks[1].codec, Codec::Aac);
    assert_eq!(summary.tracks[1].samples, 95);
    assert_eq!(summary.tracks[1].duration, 95 * 1024);
    assert!(summary.dropped_tracks.is_empty());
    assert!(summary.dropped_boxes.is_empty(), "{:?}", paths(&summary));
    // A remux of the output gives the same bytes.
    let (again, twice) = remux(&output, &RemuxOptions::default()).unwrap();
    assert!(again.source_moov_first);
    assert_eq!(twice, output);
}

#[test]
fn a_compressed_movie_is_not_copied() {
    // The movie box last, replaced by a QuickTime compressed movie box
    // (docs/design/compressed-movie.md): probe reads it, the copy does not.
    let source = build(&file(vec![video(10, 5)]));
    let at = source.windows(4).rposition(|w| w == b"moov").unwrap() - 4;
    let moov = &source[at..];
    let mut cmvd = (moov.len() as u32).to_be_bytes().to_vec();
    cmvd.extend(miniz_oxide::deflate::compress_to_vec_zlib(moov, 6));
    let cmov = bx(b"cmov", &cat(&[bx(b"dcom", b"zlib"), bx(b"cmvd", &cmvd)]));
    let packed = cat(&[source[..at].to_vec(), bx(b"moov", &cmov)]);
    let movie = probe(
        SliceSource::new(&packed),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert!(movie.compressed);
    let unsupported = Error::Unsupported {
        reason: "compressed movie box (cmov): its boxes cannot be copied",
    };
    assert_eq!(
        remux(&packed, &RemuxOptions::default()).unwrap_err(),
        ExtractError::Source(unsupported.clone())
    );
    let mut out = Vec::new();
    let sanitized = sanitize_mp4(
        SliceSource::new(&packed),
        &Limits::default(),
        &Allowlist::default(),
        Keep::default(),
        &mut out,
    );
    assert_eq!(sanitized.unwrap_err(), ExtractError::Source(unsupported));
    assert!(out.is_empty());
}

#[test]
fn track_header_fields_and_entry_boxes_are_copied() {
    let mut v = video(10, 5);
    v.entry_extra.push(bx(b"dvvC", &[1, 0, 2, 3]));
    let mut a = aac(10, false);
    a.entry_extra.push(full(b"chnl", 0, 0, &[1, 2]));
    a.flags = 1;
    let source = build(&file(vec![v, a]));
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    let find = |kind: &[u8; 4]| output.windows(4).position(|w| w == kind);
    // Every child of the video entry, in order.
    let (pasp, fiel, dvvc) = (find(b"pasp"), find(b"fiel"), find(b"dvvC"));
    assert!(pasp < fiel && fiel < dvvc, "{pasp:?} {fiel:?} {dvvc:?}");
    assert!(find(b"chnl").is_some());
    // The compressor name is copied, QuickTime's vendor field is not.
    assert!(find(b"appl").is_none() && find(b"H.26").is_some());
    // Flags 1 and alternate group 1 of the audio track header.
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(out.tracks.len(), 2);
    let tkhd = output
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"tkhd")
        .map(|(i, _)| i)
        .nth(1)
        .unwrap();
    assert_eq!(&output[tkhd + 4..tkhd + 8], &[0, 0, 0, 1]);
    assert_eq!(&output[tkhd + 4 + 4 + 20 + 8..][..4], &[0, 0, 0, 1]);
}

#[test]
fn version_1_track_headers_and_any_mdia_order() {
    let mut v = video(10, 5);
    v.tkhd_v1 = true;
    v.flags = 1;
    v.alternate_group = 3;
    v.hdlr_last = true;
    v.depth = 0x20;
    let mut a = aac(10, true);
    a.tkhd_v1 = true;
    a.flags = 7;
    a.alternate_group = 1;
    a.hdlr_last = true;
    let source = build(&file(vec![v, a]));
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    // Flags and alternate groups of both track headers.
    let tkhds: Vec<usize> = output
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"tkhd")
        .map(|(i, _)| i)
        .collect();
    let fields = |at: usize| {
        let version = output[at + 4];
        let flags = u32::from_be_bytes(output[at + 4..at + 8].try_into().unwrap()) & 0xff_ffff;
        let group = at + 4 + 4 + if version == 1 { 32 } else { 20 } + 8 + 2;
        (
            flags,
            u16::from_be_bytes(output[group..group + 2].try_into().unwrap()),
        )
    };
    assert_eq!(fields(tkhds[0]), (1, 3));
    assert_eq!(fields(tkhds[1]), (7, 1));
    // The sample entries were read although `hdlr` comes last: the video
    // entry keeps its children and depth, the sound entry its children.
    // (The first `avc1` is the brand in `ftyp`.)
    let stsd = output.windows(4).position(|w| w == b"stsd").unwrap();
    let avc1 = stsd
        + output[stsd..]
            .windows(4)
            .position(|w| w == b"avc1")
            .unwrap();
    assert_eq!(&output[avc1 + 4 + 74..avc1 + 4 + 76], &[0, 0x20]);
    assert!(output.windows(4).any(|w| w == b"avcC"));
    assert!(output.windows(4).any(|w| w == b"pasp"));
    assert!(output.windows(4).any(|w| w == b"esds"));
}

#[test]
fn wave_contents_are_reported() {
    let mut a = aac(10, true);
    a.wave_extra = vec![bx(b"enda", &[0, 1])];
    let source = build(&file(vec![video(10, 5), a]));
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    assert_eq!(
        paths(&summary),
        [
            "moov/trak/mdia/minf/stbl/stsd/mp4a/wave/enda",
            "moov/trak/mdia/minf/stbl/stsd/mp4a/chan"
        ]
    );
}

#[test]
fn edit_lists_count_against_retained_bytes() {
    let mut v = video(10, 5);
    v.edits = (0..2000).map(|_| (1, 1024, 0x10000)).collect();
    let source = build(&file(vec![v]));
    let run = |max_retained_bytes| {
        remux_mp4(
            SliceSource::new(&source),
            &Limits {
                max_retained_bytes,
                ..Limits::default()
            },
            &Allowlist::default(),
            &RemuxOptions::default(),
            &mut Vec::new(),
        )
    };
    assert!(run(1 << 20).is_ok());
    assert!(matches!(
        run(64 << 10),
        Err(ExtractError::Source(Error::LimitExceeded {
            limit: LimitKind::RetainedBytes,
            ..
        }))
    ));
}

#[test]
fn quicktime_movies_become_mp4() {
    let mut a = aac(40, true);
    // QuickTime's Macintosh language code 2: German.
    a.language = 2;
    let mut v = video(30, 10);
    v.language = 0x7fff;
    let mut f = file(vec![v, a]);
    f.major = *b"qt  ";
    f.top_extra.push(bx(b"wide", &[]));
    let source = build(&f);
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    assert_eq!(&output[8..12], b"isom");
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(out.major_brand, Some(FourCc(*b"isom")));
    assert_eq!(out.tracks[0].language, Some(*b"und"));
    assert_eq!(out.tracks[1].language, Some(*b"deu"));
    // A version 0 sound entry with the configuration box; `wave` is not
    // copied, `chan` is reported.
    let mp4a = output.windows(4).position(|w| w == b"mp4a").unwrap();
    assert_eq!(&output[mp4a + 4 + 8..mp4a + 4 + 10], &[0, 0]);
    assert_eq!(&output[mp4a + 4 + 28 + 4..mp4a + 4 + 28 + 8], b"esds");
    assert!(!output.windows(4).any(|w| w == b"chan" || w == b"wave"));
    assert_eq!(paths(&summary), ["moov/trak/mdia/minf/stbl/stsd/mp4a/chan"]);
    assert_eq!(summary.dropped_boxes[0].stream_index, Some(1));
}

#[test]
fn metadata_boxes_are_copied_and_the_rest_reported() {
    let udta = bx(b"udta", &bx(b"\xa9xyz", b"\0\x11\0\0+37.5+127.0/"));
    let meta = bx(
        b"meta",
        &cat(&[
            full(b"hdlr", 0, 0, b"\0\0\0\0mdta\0\0\0\0\0\0\0\0\0\0\0\0\0"),
            bx(b"keys", &[0; 8]),
        ]),
    );
    let mut v = video(10, 5);
    v.trak_extra = vec![
        bx(b"tref", &bx(b"tmcd", &u32s(&[3]))),
        bx(b"udta", &bx(b"name", b"camera")),
    ];
    v.stbl_extra = vec![
        full(b"sdtp", 0, 0, &[0x20; 10]),
        full(b"saiz", 0, 0, &[0; 5]),
    ];
    let mut f = file(vec![v, aac(10, false)]);
    f.moov_extra = vec![full(b"iods", 0, 0, &[0x10; 7]), udta.clone(), meta.clone()];
    f.top_extra = vec![bx(b"free", &[0; 4]), bx(b"uuid", &[7; 20])];
    let source = build(&f);
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    let contains = |needle: &[u8]| output.windows(needle.len()).any(|w| w == needle);
    assert!(contains(&udta) && contains(&meta));
    assert!(contains(&bx(b"udta", &bx(b"name", b"camera"))));
    assert!(contains(&full(b"sdtp", 0, 0, &[0x20; 10])));
    assert_eq!(
        paths(&summary),
        [
            "uuid",
            "moov/iods",
            "moov/trak/mdia/minf/stbl/saiz",
            "moov/trak/tref"
        ]
    );
    assert_eq!(summary.dropped_boxes[0].stream_index, None);
    assert_eq!(summary.dropped_boxes[0].size, 28);
    assert_eq!(summary.dropped_boxes[2].stream_index, Some(0));
    assert!(
        summary
            .dropped_boxes
            .iter()
            .all(|b| b.reason == BoxDropReason::NotCarried)
    );
    assert_eq!(summary.dropped_boxes_not_listed, 0);
    // Metadata beyond `max_retained_bytes` is left out and reported; below
    // it, a box larger than `max_box_bytes` (cover art, say) is copied.
    let big = bx(b"udta", &bx(b"\xa9cmt", &vec![b'x'; 1 << 20]));
    f.moov_extra = vec![big.clone(), meta.clone()];
    let source = build(&f);
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert!(output.windows(big.len()).any(|w| w == big));
    assert!(
        summary
            .dropped_boxes
            .iter()
            .all(|b| b.reason == BoxDropReason::NotCarried)
    );
    let mut output = Vec::new();
    let summary = remux_mp4(
        SliceSource::new(&source),
        &Limits {
            max_retained_bytes: 64 << 10,
            ..Limits::default()
        },
        &Allowlist::default(),
        &RemuxOptions::default(),
        &mut output,
    )
    .unwrap();
    assert_round_trip(&source, &output, &summary);
    let large: Vec<_> = summary
        .dropped_boxes
        .iter()
        .filter(|b| b.reason == BoxDropReason::TooLarge)
        .map(|b| (b.path.clone(), b.size))
        .collect();
    assert_eq!(
        large,
        [(vec![FourCc(*b"moov"), FourCc(*b"udta")], (1 << 20) + 16)]
    );
    assert!(output.windows(meta.len()).any(|w| w == meta));
}

#[test]
fn streams_mp4_cannot_carry_are_dropped_and_reported() {
    let source = build(&file(vec![
        other(Kind::Timecode, 1),
        video(10, 5),
        other(Kind::Pcm, 4),
        aac(10, false),
    ]));
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    let kept: Vec<usize> = summary.tracks.iter().map(|t| t.stream_index).collect();
    assert_eq!(kept, [1, 3]);
    let dropped: Vec<_> = summary
        .dropped_tracks
        .iter()
        .map(|d| (d.stream_index, d.track_id, d.reason))
        .collect();
    assert_eq!(
        dropped,
        [
            (
                0,
                1,
                TrackDropReason::Unsupported {
                    reason: "timecode tracks are not copied to MP4"
                }
            ),
            (
                2,
                3,
                TrackDropReason::Unsupported {
                    reason: "PCM audio is not copied to MP4"
                }
            ),
        ]
    );
    // Failing instead writes nothing.
    let mut out = Vec::new();
    let fail = RemuxOptions {
        on_unsupported: OnUnsupported::Fail,
        ..RemuxOptions::default()
    };
    let result = remux_mp4(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        &fail,
        &mut out,
    );
    assert_eq!(
        result,
        Err(ExtractError::Source(Error::Unsupported {
            reason: "timecode tracks are not copied to MP4"
        }))
    );
    assert!(out.is_empty());
    // Choosing streams.
    let only = |streams: Vec<usize>| RemuxOptions {
        streams: Some(streams),
        ..RemuxOptions::default()
    };
    let (summary, output) = remux(&source, &only(vec![3])).unwrap();
    assert_round_trip(&source, &output, &summary);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(
        summary
            .dropped_tracks
            .iter()
            .map(|d| d.reason)
            .collect::<Vec<_>>(),
        [
            TrackDropReason::NotSelected,
            TrackDropReason::NotSelected,
            TrackDropReason::NotSelected
        ]
    );
    assert!(matches!(
        remux(&source, &only(vec![2])),
        Err(ExtractError::Source(Error::Unsupported { .. }))
    ));
    assert_eq!(
        remux(&source, &only(vec![4])),
        Err(ExtractError::Source(Error::NoSuchTrack))
    );
    // A stream the allowlist drops is reported; naming it fails.
    let mut out = Vec::new();
    let summary = remux_mp4(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::new(&[Codec::H264]),
        &RemuxOptions::default(),
        &mut out,
    )
    .unwrap();
    assert_eq!(summary.tracks.len(), 1);
    assert!(
        summary
            .dropped_tracks
            .iter()
            .any(|d| d.stream_index == 3 && d.reason == TrackDropReason::CodecNotAllowed)
    );
    let named = remux_mp4(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::new(&[Codec::H264]),
        &only(vec![3]),
        &mut Vec::new(),
    );
    assert!(matches!(
        named,
        Err(ExtractError::Source(Error::CodecNotAllowed { .. }))
    ));
    // Nothing to copy: the reason of the first stream.
    let pcm = build(&file(vec![other(Kind::Pcm, 3)]));
    assert_eq!(
        remux(&pcm, &RemuxOptions::default()),
        Err(ExtractError::Source(Error::Unsupported {
            reason: "PCM audio is not copied to MP4"
        }))
    );
}

/// Named streams are written in the order named, a stream named twice as
/// two tracks with the same samples, as `ffmpeg` writes its `-map`s.
#[test]
fn named_streams_keep_their_order_and_repeats() {
    let source = build(&file(vec![video(30, 10), aac(40, false)]));
    let named = |streams: Vec<usize>| RemuxOptions {
        streams: Some(streams),
        ..RemuxOptions::default()
    };
    let (summary, output) = remux(&source, &named(vec![1, 0, 0])).unwrap();
    assert_round_trip(&source, &output, &summary);
    assert_eq!(
        summary
            .tracks
            .iter()
            .map(|t| t.stream_index)
            .collect::<Vec<_>>(),
        [1, 0, 0]
    );
    assert!(summary.dropped_tracks.is_empty());
    let (summary, _) = remux(&source, &named(vec![1])).unwrap();
    assert_eq!(
        summary
            .dropped_tracks
            .iter()
            .map(|d| (d.stream_index, d.reason))
            .collect::<Vec<_>>(),
        [(0, TrackDropReason::NotSelected)]
    );
}

#[test]
fn flac_alac_ac3_and_eac3_keep_their_sample_entries() {
    // FLAC's STREAMINFO (RFC 9639 8.2): 1,152-sample blocks, 48 kHz, two
    // channels, 16 bits, 6,912 samples, an MD5 signature.
    let mut streaminfo = vec![0x04, 0x80, 0x04, 0x80, 0, 0, 20, 0, 0, 40];
    streaminfo.extend([0x0B, 0xB8, 0x02, 0xF0, 0, 0, 0x1B, 0]);
    streaminfo.extend([0x5A; 16]);
    // The Apple Lossless magic cookie: 4,096-sample frames, 16 bits, two
    // channels at 48 kHz.
    let cookie = [
        0, 0, 0x10, 0, 0, 16, 40, 10, 14, 2, 0, 0xFF, 0, 0, 0, 40, 0, 0, 0xFA, 0, 0, 0, 0xBB, 0x80,
    ];
    let entries = [
        (
            *b"fLaC",
            full(b"dfLa", 0, 0, &cat(&[vec![0x80, 0, 0, 34], streaminfo])),
            1152,
        ),
        (*b"alac", full(b"alac", 0, 0, &cookie), 4096),
        // ETSI TS 102 366 F.4 and F.6: 48 kHz stereo.
        (*b"ac-3", bx(b"dac3", &[0x10, 0x10, 0x00]), 1536),
        (*b"ec-3", bx(b"dec3", &[0x02, 0x00, 0x20, 0x04, 0x00]), 1536),
    ];
    let tracks = entries
        .into_iter()
        .map(|(kind, config, frame)| {
            let mut t = other(Kind::Sound(kind, config), 6);
            t.timescale = 48_000;
            for (n, s) in t.samples.iter_mut().enumerate() {
                s.duration = frame;
                s.size = 30 + n as u32;
            }
            t
        })
        .collect();
    let source = build(&file(tracks));
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert!(summary.dropped_tracks.is_empty(), "{summary:?}");
    assert_eq!(summary.tracks.len(), 4);
    assert_round_trip(&source, &output, &summary);
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let codecs: Vec<Codec> = out.tracks.iter().map(|t| t.description.codec).collect();
    assert_eq!(codecs, [Codec::Flac, Codec::Alac, Codec::Ac3, Codec::Eac3]);
}

#[test]
fn edit_lists_are_copied_as_stored() {
    let mut v = video(30, 10);
    // An empty edit, a dwell (rate 0) and two media edits: outside the
    // parser's presentation model, copied entry for entry.
    v.edits = vec![
        (200, -1, 0x10000),
        (400, 1024, 0),
        (500, 2048, 0x10000),
        (300, 5120, 0x10000),
    ];
    let mut a = aac(30, false);
    a.edits = Vec::new();
    let source = build(&file(vec![v, a]));
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    let elst = |data: &[u8]| {
        let at = data.windows(4).position(|w| w == b"elst").unwrap();
        let size = u32::from_be_bytes(data[at - 4..at].try_into().unwrap()) as usize;
        data[at..at - 4 + size].to_vec()
    };
    assert_eq!(elst(&output), elst(&source));
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(out.tracks[0].edit_list, EditList::Unsupported);
    assert_eq!(out.tracks[1].edit_list, EditList::None);
}

#[test]
fn fragmented_sources_are_flattened() {
    for edit in [None, Some((0, 1024)), Some((80, 1024))] {
        let source = common::fragmented_movie(Some(2048), edit);
        let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
        assert_round_trip(&source, &output, &summary);
        let out = probe(
            SliceSource::new(&output),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        assert_eq!(out.tracks[0].sample_count, 4);
        // A last edit of duration 0 lasts to the end of the media: 1,024
        // ticks at 12,800 Hz, exactly 80 ticks of the 1,000 Hz movie
        // timescale, which is kept since nothing rounds in it.
        assert_eq!(out.timescale, 1_000);
        if let Some((0, _)) = edit {
            assert_eq!(
                out.tracks[0].edit_list,
                EditList::Simple {
                    empty: 0,
                    media_time: 1024,
                    segment: 80,
                }
            );
        }
    }
    // Fragments with a base decode time: the start becomes an empty edit,
    // so every sample keeps its presentation time.
    let source = fragmented_aac(&[4800, 4800 + 2048]);
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    // 4,096 ticks at 48 kHz are 85.33 ms: the movie timescale becomes
    // 48,000, the least common multiple of 1,000 and 48,000, so the edits
    // are exact.
    assert_eq!(
        out.tracks[0].edit_list,
        EditList::Simple {
            empty: 4_800,
            media_time: 0,
            segment: 4_096,
        }
    );
    // A gap between fragments cannot be stored as durations.
    assert_eq!(
        remux(&fragmented_aac(&[0, 4096]), &RemuxOptions::default()),
        Err(ExtractError::Source(Error::Unsupported {
            reason: "a gap or an overlap between decode times"
        }))
    );
}

#[test]
fn fragmented_late_start_with_b_frames_presents_every_frame() {
    // Decoded from 2 s with no edit list, frames reordered: the frame
    // presented last ends 512 ticks after the decode durations do, and the
    // media edit must reach it (ISO/IEC 14496-12 8.6.1.3, 8.6.6).
    let source = fragmented_reordered(2 * common::TIMESCALE);
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(
        out.tracks[0].edit_list,
        EditList::Simple {
            empty: 2_000,
            media_time: 0,
            segment: 200,
        }
    );
    // Decoded from 0 the source needs no edit list, and neither does the
    // output: a composition end that would round in the movie timescale
    // (2,592 ticks at 12,800 Hz are 202.5 ms) leaves that timescale alone.
    let source = fragmented_reordered_with(0, [512, 1568, 0, 0]);
    let (summary, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert_round_trip(&source, &output, &summary);
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(out.timescale, 1_000);
    assert_eq!(out.tracks[0].edit_list, EditList::None);
}

/// `common::fragmented_movie`'s movie with its one fragment decoded from
/// `base` (`tfdt`) and composition offsets of an I, P, B, B order: decode
/// times 0, 512, 1,024 and 1,536 after `base`, composition times 512,
/// 2,048, 1,024 and 1,536, so the frames end at 2,560 while the decode
/// durations add up to 2,048.
fn fragmented_reordered(base: u32) -> Vec<u8> {
    fragmented_reordered_with(base, [512, 1536, 0, 0])
}

/// `fragmented_reordered` with other composition offsets.
fn fragmented_reordered_with(base: u32, offsets: [u32; 4]) -> Vec<u8> {
    let movie = common::fragmented_movie(None, None);
    let at = movie.windows(4).position(|w| w == b"moof").expect("a moof") - 4;
    let tfhd = full(b"tfhd", 0, 0x02_0000, &u32s(&[1]));
    let tfdt = full(b"tfdt", 0, 0, &u32s(&[base]));
    // trun: data offset and one composition offset per sample.
    let traf_len = 8 + tfhd.len() + tfdt.len() + 36;
    let moof_len = 8 + 16 + traf_len;
    let trun = full(
        b"trun",
        0,
        0x801,
        &u32s(&[
            4,
            (moof_len + 8) as u32,
            offsets[0],
            offsets[1],
            offsets[2],
            offsets[3],
        ]),
    );
    let moof = bx(
        b"moof",
        &cat(&[
            full(b"mfhd", 0, 0, &u32s(&[1])),
            bx(b"traf", &cat(&[tfhd, tfdt, trun])),
        ]),
    );
    assert_eq!(moof.len(), moof_len);
    cat(&[movie[..at].to_vec(), moof, bx(b"mdat", &[0xAB; 400])])
}

/// A fragmented AAC movie: one fragment of two 10-byte samples of 1024
/// ticks per base decode time.
fn fragmented_aac(base_times: &[u32]) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"iso6\0\0\x02\0");
    let mut mvhd = u32s(&[0, 0, 1000, 0]);
    mvhd.extend_from_slice(&[0u8; 80]);
    let mut tkhd = u32s(&[0, 0, 1, 0, 0]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[
        0x10000,
        0,
        0,
        0,
        0x10000,
        0,
        0,
        0,
        0x4000_0000,
        0,
        0,
    ]));
    let mut mdhd = u32s(&[0, 0, 48_000, 0]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut entry = vec![0u8; 6];
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&[0u8; 8]);
    entry.extend_from_slice(&2u16.to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes());
    entry.extend_from_slice(&[0u8; 4]);
    entry.extend_from_slice(&(48_000u32 << 16).to_be_bytes());
    entry.extend_from_slice(&esds());
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&bx(b"mp4a", &entry));
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &u32s(&[0])),
            full(b"stsc", 0, 0, &u32s(&[0])),
            full(b"stsz", 0, 0, &u32s(&[0, 0])),
            full(b"stco", 0, 0, &u32s(&[0])),
        ]),
    );
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let minf = bx(b"minf", &cat(&[bx(b"dinf", &dref), stbl]));
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia])),
            bx(b"mvex", &full(b"trex", 0, 0, &u32s(&[1, 1, 1024, 10, 0]))),
        ]),
    );
    let mut parts = vec![ftyp, moov];
    for (i, base) in base_times.iter().enumerate() {
        let tfhd = full(b"tfhd", 0, 0x02_0000, &u32s(&[1]));
        let tfdt = full(b"tfdt", 0, 0, &u32s(&[*base]));
        let traf_len = 8 + tfhd.len() + tfdt.len() + 20;
        let moof_len = 8 + 16 + traf_len;
        let trun = full(b"trun", 0, 0x1, &u32s(&[2, (moof_len + 8) as u32]));
        let moof = bx(
            b"moof",
            &cat(&[
                full(b"mfhd", 0, 0, &u32s(&[i as u32 + 1])),
                bx(b"traf", &cat(&[tfhd, tfdt, trun])),
            ]),
        );
        assert_eq!(moof.len(), moof_len);
        parts.push(moof);
        parts.push(bx(b"mdat", &[i as u8 + 1; 20]));
    }
    cat(&parts)
}

#[test]
fn table_bytes_are_limited_across_streams() {
    let source = build(&file(vec![video(400, 25), aac(400, false)]));
    let run = |max_table_bytes| {
        remux_mp4(
            SliceSource::new(&source),
            &Limits {
                max_table_bytes,
                ..Limits::default()
            },
            &Allowlist::default(),
            &RemuxOptions::default(),
            &mut Vec::new(),
        )
    };
    assert!(run(1 << 20).is_ok());
    assert!(matches!(
        run(4000),
        Err(ExtractError::Source(Error::LimitExceeded {
            limit: LimitKind::TableBytes,
            ..
        }))
    ));
}

struct FailingWriter {
    left: usize,
}

impl Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let n = buf.len().min(self.left);
        self.left -= n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn write_errors_are_reported() {
    let source = build(&file(vec![video(20, 5), aac(20, false)]));
    for left in [0, 30, 500, 2000] {
        let result = remux_mp4(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            &RemuxOptions::default(),
            FailingWriter { left },
        );
        assert_eq!(result, Err(ExtractError::Write(io::ErrorKind::BrokenPipe)));
    }
}

/// A source whose reads fail after `reads` reads.
struct Shrinking<'a> {
    data: &'a [u8],
    reads: u32,
}

impl Source for Shrinking<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.data.len() as u64)
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        if self.reads == 0 {
            return Err(Error::Io(io::ErrorKind::UnexpectedEof));
        }
        self.reads -= 1;
        SliceSource::new(self.data).read_exact_at(offset, buf)
    }
}

#[test]
fn read_errors_are_reported() {
    let source = build(&file(vec![video(20, 5), aac(20, false)]));
    let run = |reads| {
        remux_mp4(
            Shrinking {
                data: &source,
                reads,
            },
            &Limits::default(),
            &Allowlist::default(),
            &RemuxOptions::default(),
            io::sink(),
        )
    };
    let mut total = 0;
    while run(total).is_err() {
        total += 1;
        assert!(total < 10_000);
    }
    for reads in 0..total {
        assert_eq!(
            run(reads),
            Err(ExtractError::Source(Error::Io(
                io::ErrorKind::UnexpectedEof
            ))),
            "{reads}"
        );
    }
}

/// Every truncation of a valid input fails cleanly or writes a valid output.
#[test]
fn truncated_inputs_are_safe() {
    let source = build(&file(vec![video(8, 4), aac(8, true)]));
    for len in 0..source.len() {
        let mut out = Vec::new();
        if let Ok(summary) = remux_mp4(
            SliceSource::new(&source[..len]),
            &Limits::default(),
            &Allowlist::default(),
            &RemuxOptions::default(),
            &mut out,
        ) {
            assert_round_trip(&source[..len], &out, &summary);
        }
    }
}

/// The fuzz property on deterministic mutations: every input fails cleanly
/// or gives an output the parser accepts, with every sample of the kept
/// streams.
fn assert_mutations_are_safe(source: &[u8], rounds: u32, seed: u64) {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let limits = Limits {
        max_sample_work: 1 << 20,
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    for _ in 0..rounds {
        let mut input = source.to_vec();
        for _ in 0..1 + next() % 4 {
            let at = (next() % input.len() as u64) as usize;
            input[at] = next() as u8;
        }
        let mut out = Vec::new();
        let Ok(summary) = remux_mp4(
            SliceSource::new(&input),
            &limits,
            &Allowlist::default(),
            &RemuxOptions::default(),
            &mut out,
        ) else {
            continue;
        };
        assert_eq!(summary.bytes_written, out.len() as u64);
        let movie = probe(
            SliceSource::new(&out),
            &Limits::default(),
            &Allowlist::default(),
        )
        .expect("output parses");
        assert_eq!(movie.tracks.len(), summary.tracks.len());
        for (track, kept) in movie.tracks.iter().zip(&summary.tracks) {
            assert_eq!(
                packets(&out, &movie, track.index).len(),
                kept.samples as usize
            );
        }
    }
}

#[test]
fn mutated_inputs_are_safe() {
    let mut f = file(vec![other(Kind::Timecode, 1), video(12, 4), aac(12, true)]);
    f.moov_extra = vec![bx(b"udta", &bx(b"\xa9xyz", b"\0\x05\0\0+1+2/"))];
    assert_mutations_are_safe(&build(&f), 3000, 0x2545_f491);
    assert_mutations_are_safe(
        &common::fragmented_movie(Some(2048), None),
        2000,
        0x9e37_79b9,
    );
}

// ------------------------------------------------------------- sanitization

fn sanitize(source: &[u8], keep: Keep) -> (RemuxSummary, Vec<u8>) {
    let mut out = Vec::new();
    let summary = sanitize_mp4(
        SliceSource::new(source),
        &Limits::default(),
        &Allowlist::default(),
        keep,
        &mut out,
    )
    .unwrap();
    assert_eq!(summary.bytes_written, out.len() as u64);
    assert_copied(source, &out, &summary, Some(keep));
    (summary, out)
}

/// QuickTime metadata keys (`keys`) and values (`ilst`): one item per key,
/// its type the key's 1-based index.
fn quicktime_meta(keys: &[&str]) -> Vec<u8> {
    let mut entries = u32s(&[0, keys.len() as u32]);
    let mut items = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        entries.extend_from_slice(&((key.len() + 8) as u32).to_be_bytes());
        entries.extend_from_slice(b"mdta");
        entries.extend_from_slice(key.as_bytes());
        let data = bx(b"data", &cat(&[u32s(&[1, 0]), b"value".to_vec()]));
        items.extend_from_slice(&bx(&(i as u32 + 1).to_be_bytes(), &data));
    }
    bx(
        b"meta",
        &cat(&[
            full(b"hdlr", 0, 0, b"\0\0\0\0mdta\0\0\0\0\0\0\0\0\0\0\0\0\0"),
            bx(b"keys", &entries),
            bx(b"ilst", &items),
        ]),
    )
}

/// A camera-like movie: location, device and date metadata at every level.
fn camera_movie() -> Vec<u8> {
    let mut v = video(20, 10);
    v.language = 0x2df2; // "kor"
    v.trak_extra = vec![bx(b"udta", &bx(b"\xa9mak", b"\0\x05\0\0Maker"))];
    v.entry_extra.push(bx(b"uuid", &[7; 24]));
    v.stbl_extra = vec![full(b"sdtp", 0, 0, &[0x20; 20])];
    let mut a = aac(20, false);
    a.entry_extra.push(full(b"chnl", 0, 0, &[1, 2]));
    a.entry_extra.push(bx(b"vend", &[1, 2, 3, 4]));
    let mut f = file(vec![v, a]);
    // A rotation of 90 degrees.
    let udta = bx(
        b"udta",
        &cat(&[
            bx(b"\xa9xyz", b"\0\x11\0\0+37.5+127.0/"),
            bx(b"\xa9mod", b"\0\x05\0\0Model"),
            bx(b"\xa9day", b"\0\x04\0\x002026"),
            bx(b"\xa9nam", b"\0\x05\0\0Title"),
        ]),
    );
    f.moov_extra = vec![
        udta,
        quicktime_meta(&[
            "com.apple.quicktime.location.ISO6709",
            "com.apple.quicktime.make",
            "com.apple.quicktime.model",
            "com.apple.quicktime.creationdate",
        ]),
    ];
    f.top_extra = vec![bx(b"uuid", &[9; 40])];
    let mut source = build(&f);
    // Rotate the video track header's matrix by a quarter turn.
    let tkhd = source.windows(4).position(|w| w == b"tkhd").unwrap();
    let matrix = tkhd + 4 + 4 + 20 + 8 + 8;
    source[matrix..matrix + 36].copy_from_slice(&u32s(&[
        0,
        0x10000,
        0,
        0xffff_0000,
        0,
        0,
        0,
        0,
        0x4000_0000,
    ]));
    source
}

#[test]
fn sanitizing_removes_metadata_and_reports_it() {
    let source = camera_movie();
    let (summary, output) = sanitize(&source, Keep::default());
    let kinds = box_tree(&output);
    for kind in [b"udta", b"meta", b"keys", b"ilst", b"uuid", b"vend"] {
        assert!(!kinds.contains(kind), "{}", String::from_utf8_lossy(kind));
    }
    let contains = |needle: &[u8]| output.windows(needle.len()).any(|w| w == needle);
    for gone in [
        &b"udta"[..],
        b"meta",
        b"keys",
        b"\xa9xyz",
        b"\xa9mak",
        b"+37.5",
        b"Maker",
        b"location",
        b"vend",
        b"H.264",
    ] {
        assert!(!contains(gone), "{}", String::from_utf8_lossy(gone));
    }
    // Kept: structural and allowlisted boxes.
    for kept in [&b"avcC"[..], b"pasp", b"fiel", b"chnl", b"esds", b"sdtp"] {
        assert!(contains(kept), "{}", String::from_utf8_lossy(kept));
    }
    let by_path = |path: &str| {
        summary
            .dropped_boxes
            .iter()
            .filter(|b| {
                b.path
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("/")
                    == path
            })
            .map(|b| (b.reason, b.kind, b.name.clone()))
            .collect::<Vec<_>>()
    };
    let sanitized = BoxDropReason::Sanitized;
    assert_eq!(by_path("moov/udta"), [(sanitized, None, None)]);
    assert_eq!(
        by_path("moov/udta/\\xa9xyz"),
        [(sanitized, Some(MetadataKind::Location), None)]
    );
    assert_eq!(
        by_path("moov/udta/\\xa9mod"),
        [(sanitized, Some(MetadataKind::Device), None)]
    );
    assert_eq!(
        by_path("moov/udta/\\xa9day"),
        [(sanitized, Some(MetadataKind::Date), None)]
    );
    assert_eq!(
        by_path("moov/udta/\\xa9nam"),
        [(sanitized, Some(MetadataKind::Other), None)]
    );
    let keys: Vec<_> = by_path("moov/meta/keys")
        .into_iter()
        .map(|(_, kind, name)| (kind.unwrap(), name.unwrap()))
        .collect();
    assert_eq!(
        keys,
        [
            (
                MetadataKind::Location,
                "com.apple.quicktime.location.ISO6709".to_string()
            ),
            (MetadataKind::Device, "com.apple.quicktime.make".to_string()),
            (
                MetadataKind::Device,
                "com.apple.quicktime.model".to_string()
            ),
            (
                MetadataKind::Date,
                "com.apple.quicktime.creationdate".to_string()
            ),
        ]
    );
    // Keyed items are named by their key.
    let items: Vec<_> = summary
        .dropped_boxes
        .iter()
        .filter(|b| b.path.len() == 4 && b.path[2] == FourCc(*b"ilst"))
        .map(|b| b.name.clone().unwrap())
        .collect();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0], "com.apple.quicktime.location.ISO6709");
    assert_eq!(
        by_path("moov/trak/udta/\\xa9mak"),
        [(sanitized, Some(MetadataKind::Device), None)]
    );
    assert_eq!(
        by_path("moov/trak/mdia/minf/stbl/stsd/avc1/uuid"),
        [(sanitized, Some(MetadataKind::Other), None)]
    );
    assert_eq!(
        by_path("moov/trak/mdia/minf/stbl/stsd/mp4a/vend"),
        [(sanitized, Some(MetadataKind::Other), None)]
    );
    assert_eq!(by_path("uuid"), [(BoxDropReason::NotCarried, None, None)]);
    // Cleared fields: creation times everywhere, the compressor name; the
    // rotation and the language are kept by default.
    assert_eq!(
        summary.cleared,
        [
            Cleared {
                stream_index: None,
                field: ClearedField::CreationTime
            },
            Cleared {
                stream_index: Some(0),
                field: ClearedField::CompressorName
            },
            Cleared {
                stream_index: Some(0),
                field: ClearedField::CreationTime
            },
            Cleared {
                stream_index: Some(1),
                field: ClearedField::CreationTime
            },
        ]
    );
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(out.creation_time, 0);
    assert_eq!(out.tracks[0].rotation, Some(90));
    assert_eq!(out.tracks[0].language, Some(*b"kor"));
}

#[test]
fn sanitizing_can_clear_rotation_and_language_too() {
    let source = camera_movie();
    let (summary, _) = sanitize(&source, Keep::NOTHING);
    let fields: Vec<_> = summary
        .cleared
        .iter()
        .filter(|c| c.stream_index == Some(0))
        .map(|c| c.field)
        .collect();
    assert_eq!(
        fields,
        [
            ClearedField::CompressorName,
            ClearedField::Rotation,
            ClearedField::Language,
            ClearedField::CreationTime
        ]
    );
    // Keeping the dates as well.
    let (summary, _) = sanitize(
        &source,
        Keep {
            creation_times: true,
            ..Keep::default()
        },
    );
    assert!(
        summary
            .cleared
            .iter()
            .all(|c| c.field == ClearedField::CompressorName)
    );
}

/// A free-form item (`----`): namespace, name, UTF-8 value.
fn freeform(mean: &str, name: &str, value: &str) -> Vec<u8> {
    bx(
        b"----",
        &cat(&[
            full(b"mean", 0, 0, mean.as_bytes()),
            full(b"name", 0, 0, name.as_bytes()),
            bx(b"data", &cat(&[u32s(&[1, 0]), value.as_bytes().to_vec()])),
        ]),
    )
}

#[test]
fn sanitizing_keeps_gapless_information_and_track_kinds() {
    let mut a = aac(20, false);
    a.trak_extra = vec![bx(
        b"udta",
        &cat(&[
            full(b"kind", 0, 0, b"urn:mpeg:dash:role:2011\0forced-subtitle\0"),
            // Not from the vocabulary: removed, even when printable.
            full(b"kind", 0, 0, b"urn:x\0\xa9here\0"),
            full(b"kind", 0, 0, b"urn:mpeg:dash:role:2011\0Alice at +37.7\0"),
            bx(b"\xa9nam", b"\0\x05\0\0Title"),
        ]),
    )];
    let mut f = file(vec![video(20, 10), a]);
    let ilst = cat(&[
        // Another field than the canonical zeros: written as zero.
        freeform(
            "com.apple.iTunes",
            "iTunSMPB",
            " 00000000 00000840 000001C0 0000000000A1E7DC DEADBEEF 00000000",
        ),
        freeform("com.example", "Location", "+37.5+127.0/"),
        // A name that could hold a value itself is not repeated.
        freeform("com.example", "lat=+37.7749 owner=Alice", "x"),
        bx(
            b"\xa9nam",
            &bx(b"data", &cat(&[u32s(&[1, 0]), b"Title".to_vec()])),
        ),
    ]);
    f.moov_extra = vec![bx(
        b"udta",
        &cat(&[
            // Kinds describe tracks: removed at the movie level.
            full(b"kind", 0, 0, b"urn:mpeg:dash:role:2011\0main\0"),
            full(
                b"meta",
                0,
                0,
                &cat(&[
                    full(b"hdlr", 0, 0, b"\0\0\0\0mdirappl\0\0\0\0\0\0\0\0\0"),
                    bx(b"ilst", &ilst),
                ]),
            ),
        ]),
    )];
    let source = build(&f);
    let (summary, output) = sanitize(&source, Keep::default());
    let contains = |needle: &[u8]| output.windows(needle.len()).any(|w| w == needle);
    let canonical = format!(
        " 00000000 00000840 000001C0 0000000000A1E7DC{}",
        " 00000000".repeat(8)
    );
    assert!(contains(canonical.as_bytes()));
    assert!(contains(b"urn:mpeg:dash:role:2011\0forced-subtitle\0"));
    for gone in [
        &b"DEADBEEF"[..],
        b"Title",
        b"+37.5",
        b"com.example",
        b"urn:x",
        b"Alice",
        b"\0main\0",
    ] {
        assert!(!contains(gone), "{}", String::from_utf8_lossy(gone));
    }
    let named: Vec<_> = summary
        .dropped_boxes
        .iter()
        .filter_map(|b| b.name.clone().map(|n| (b.kind, n)))
        .collect();
    assert_eq!(
        named,
        [(Some(MetadataKind::Location), "Location".to_string())]
    );
    let paths: Vec<_> = summary
        .dropped_boxes
        .iter()
        .map(|b| {
            b.path
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    assert_eq!(
        paths.iter().filter(|p| *p == "moov/trak/udta/kind").count(),
        2
    );
    assert_eq!(paths.iter().filter(|p| *p == "moov/udta/kind").count(), 1);
    // Both free-form items are listed; only the identifier is named.
    assert_eq!(
        paths
            .iter()
            .filter(|p| *p == "moov/udta/meta/ilst/----")
            .count(),
        2
    );
    assert!(paths.iter().any(|p| p == "moov/udta/meta/ilst/----"));
    assert!(!paths.iter().any(|p| p.ends_with("iTunSMPB")));
    // The output's own tree: a track `udta` with the kind, a movie `udta`
    // with one item; nothing kept with `Keep::NOTHING`.
    assert_eq!(
        box_tree(&output).iter().filter(|k| *k == b"udta").count(),
        2
    );
    let (_, output) = sanitize(&source, Keep::NOTHING);
    assert!(!box_tree(&output).contains(b"udta"));
    let (_, again) = sanitize(&sanitize(&source, Keep::default()).1, Keep::default());
    assert!(
        again
            .windows(canonical.len())
            .any(|w| w == canonical.as_bytes())
    );
}

#[test]
fn sanitizing_rewrites_allowlisted_boxes_that_can_hold_text() {
    let mut v = video(20, 10);
    // An ICC profile naming the device, and a spherical video header naming
    // the camera; coded colour parameters stay.
    v.entry_extra.push(bx(b"colr", b"profICC: Maker Model X"));
    v.entry_extra.push(bx(
        b"sv3d",
        &cat(&[
            full(b"svhd", 0, 0, b"Camera 360 v1.2\0"),
            bx(b"proj", &full(b"prhd", 0, 0, &[0; 12])),
        ]),
    ));
    let mut w = video(20, 10);
    w.entry_extra.push(bx(b"colr", b"nclx\0\x01\0\x01\0\x01\0"));
    let source = build(&file(vec![v, w]));
    let (summary, output) = sanitize(&source, Keep::default());
    let contains = |needle: &[u8]| output.windows(needle.len()).any(|w| w == needle);
    assert!(!contains(b"Maker Model"));
    assert!(!contains(b"Camera 360"));
    assert!(contains(b"nclx"));
    assert!(contains(b"prhd"));
    assert!(summary.dropped_boxes.iter().any(|b| {
        b.stream_index == Some(0)
            && b.path.last() == Some(&FourCc(*b"colr"))
            && b.reason == BoxDropReason::Sanitized
    }));
    assert!(summary.cleared.contains(&Cleared {
        stream_index: Some(0),
        field: ClearedField::SphericalSource
    }));
}

#[test]
fn sanitized_reports_and_their_memory_are_bounded() {
    // 600 user data items: 256 listed, the rest counted.
    let items: Vec<Vec<u8>> = (0..600).map(|_| bx(b"\xa9nam", b"\0\x01\0\0x")).collect();
    let mut f = file(vec![video(20, 10)]);
    f.moov_extra = vec![bx(b"udta", &items.concat())];
    let source = build(&f);
    let (summary, _) = sanitize(&source, Keep::default());
    assert_eq!(summary.dropped_boxes.len(), 256);
    assert!(summary.dropped_boxes_not_listed >= 600 - 256);
    // Each key of a `keys` box counts against max_total_boxes.
    let keys: Vec<String> = (0..200).map(|i| format!("k{i}")).collect();
    let names: Vec<&str> = keys.iter().map(String::as_str).collect();
    let mut f = file(vec![video(20, 10)]);
    f.moov_extra = vec![quicktime_meta(&names)];
    let source = build(&f);
    let limits = Limits {
        max_total_boxes: 150,
        ..Limits::default()
    };
    let error = sanitize_mp4(
        SliceSource::new(&source),
        &limits,
        &Allowlist::default(),
        Keep::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ExtractError::Source(Error::LimitExceeded {
            limit: LimitKind::TotalBoxes,
            ..
        })
    ));
}

#[test]
fn sanitized_mutations_are_safe() {
    let source = camera_movie();
    let mut state = 0x5bd1_e995u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let limits = Limits {
        max_sample_work: 1 << 20,
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    for _ in 0..3000 {
        let mut input = source.clone();
        for _ in 0..1 + next() % 4 {
            let at = (next() % input.len() as u64) as usize;
            input[at] = next() as u8;
        }
        let mut out = Vec::new();
        let Ok(summary) = sanitize_mp4(
            SliceSource::new(&input),
            &limits,
            &Allowlist::default(),
            Keep::default(),
            &mut out,
        ) else {
            continue;
        };
        assert_eq!(summary.bytes_written, out.len() as u64);
        let movie = probe(
            SliceSource::new(&out),
            &Limits::default(),
            &Allowlist::default(),
        )
        .expect("output parses");
        assert_eq!(movie.tracks.len(), summary.tracks.len());
        // No metadata box in the box tree; sample data is copied as is, and
        // a damaged chunk offset can make it overlap the source's boxes.
        let kinds = box_tree(&out);
        assert!(
            !kinds
                .iter()
                .any(|k| matches!(k, b"udta" | b"meta" | b"keys" | b"ilst")),
            "{kinds:?}"
        );
    }
}

/// Every box type of the movie box's tree, containers walked.
fn box_tree(data: &[u8]) -> Vec<[u8; 4]> {
    fn walk(data: &[u8], at: usize, end: usize, out: &mut Vec<[u8; 4]>) {
        let mut at = at;
        while at + 8 <= end {
            let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
            if size < 8 || at + size > end {
                return;
            }
            out.push(kind);
            if matches!(
                &kind,
                b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf"
            ) {
                walk(data, at + 8, at + size, out);
            }
            if &kind == b"stsd" {
                // The entry and its children after the fixed fields.
                let entry = at + 16;
                let entry_size = u32::from_be_bytes(data[entry..entry + 4].try_into().unwrap());
                let fixed = if matches!(&data[entry + 4..entry + 8], b"mp4a" | b"Opus") {
                    36
                } else {
                    86
                };
                out.push(data[entry + 4..entry + 8].try_into().unwrap());
                walk(data, entry + fixed, entry + entry_size as usize, out);
            }
            at += size;
        }
    }
    let mut out = Vec::new();
    walk(data, 0, data.len(), &mut out);
    out
}

// ------------------------------------------------------------- fixtures

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Fixtures that must remux when they were generated.
const REQUIRED: &[&str] = &[
    "basic.mp4",
    "basic.mov",
    "faststart.mp4",
    "fragmented.mp4",
    "fragmented_moov.mp4",
    "edit_list.mp4",
    "negative_cts.mp4",
    "multitrack.mp4",
    "timecode.mov",
    "hevc_hvc1.mp4",
    "hevc_hev1.mp4",
    "vp9.mp4",
    "av1.mp4",
    "audio_opus.mp4",
    "audio_mp3.mp4",
    "audio_aac.m4a",
    "rotate_ccw90.mp4",
    "color_bt2020_pq.mp4",
    "remux_metadata.mov",
    "remux_metadata.mp4",
];

/// Every MP4, MOV and M4A fixture is remuxed; each output round-trips. With
/// `COREAV_REMUX_OUT` the outputs and a manifest for
/// `scripts/remux_check.py` are written there.
#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_round_trip() {
    fixtures("COREAV_REMUX_OUT", None);
}

/// The same with sanitization keeping what it keeps by default
/// (`docs/design/sanitize.md`); outputs go to `COREAV_SANITIZE_OUT`.
#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_sanitize_round_trip() {
    fixtures("COREAV_SANITIZE_OUT", Some(Keep::default()));
}

fn fixtures(out_var: &str, keep: Option<Keep>) {
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
    let out_dir = std::env::var(out_var).ok();
    if let Some(out_dir) = &out_dir {
        std::fs::create_dir_all(out_dir).unwrap();
    }
    let generated = std::fs::read_to_string(Path::new(&dir).join("MANIFEST")).unwrap_or_default();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("mp4" | "mov" | "m4a")
            )
        })
        .collect();
    entries.sort();
    let mut manifest = Vec::new();
    let mut done = std::collections::BTreeSet::new();
    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read(path).unwrap();
        let options = RemuxOptions {
            metadata: keep.map_or(Metadata::Copy, Metadata::Sanitize),
            ..RemuxOptions::default()
        };
        let invalid = common::INVALID_STREAMS.contains(&name.as_str());
        let (summary, output) = match remux(&source, &options) {
            Ok(done) => {
                assert!(!invalid, "{name}: written");
                done
            }
            Err(error) => {
                assert!(!REQUIRED.contains(&name.as_str()), "{name}: {error}");
                assert!(
                    !invalid || matches!(error, ExtractError::Source(Error::InvalidStream { .. })),
                    "{name}: {error}"
                );
                eprintln!("{name}: refused: {error}");
                continue;
            }
        };
        assert_copied(&source, &output, &summary, keep);
        done.insert(name.clone());
        eprintln!(
            "{name}: {} streams kept, {} dropped, {} boxes dropped",
            summary.tracks.len(),
            summary.dropped_tracks.len(),
            summary.dropped_boxes.len()
        );
        if let Some(out_dir) = &out_dir {
            let output_path = Path::new(out_dir).join(format!(
                "{name}.{}.mp4",
                if keep.is_some() { "sanitized" } else { "remux" }
            ));
            std::fs::write(&output_path, &output).unwrap();
            let streams: Vec<String> = summary
                .tracks
                .iter()
                .map(|t| {
                    format!(
                        "{{\"stream_index\": {}, \"track_id\": {}, \"source_track_id\": {}}}",
                        t.stream_index, t.track_id, t.source_track_id
                    )
                })
                .collect();
            let dropped: Vec<String> = summary
                .dropped_tracks
                .iter()
                .map(|d| d.stream_index.to_string())
                .collect();
            let keep = keep.map_or("null".to_string(), |k| {
                let names: Vec<&str> = [
                    (k.rotation, "\"rotation\""),
                    (k.language, "\"language\""),
                    (k.creation_times, "\"dates\""),
                    (k.gapless, "\"gapless\""),
                    (k.kinds, "\"kinds\""),
                ]
                .into_iter()
                .filter_map(|(on, name)| on.then_some(name))
                .collect();
                format!("[{}]", names.join(", "))
            });
            manifest.push(format!(
                "{{\"source\": {}, \"output\": {}, \"streams\": [{}], \"dropped_streams\": [{}], \"keep\": {}}}",
                json_string(&path.display().to_string()),
                json_string(&output_path.display().to_string()),
                streams.join(", "),
                dropped.join(", "),
                keep,
            ));
        }
    }
    for name in REQUIRED {
        if generated.contains(&format!("generated {name}")) {
            assert!(done.contains(*name), "{name} was not remuxed");
        }
    }
    if let Some(out_dir) = &out_dir {
        std::fs::write(
            Path::new(out_dir).join("manifest.json"),
            format!("[\n{}\n]\n", manifest.join(",\n")),
        )
        .unwrap();
    }
}

/// `left_out_covers` takes cover pictures out of the copied item lists: a
/// `data` box of an item that keeps another, an item that keeps none (an
/// `ilst` item, or a QuickTime keyed one), each listed as dropped; every
/// other box is copied as stored. An offset that starts no `data` value
/// fails before anything is written.
#[test]
fn left_out_covers_leave_the_copied_item_lists() {
    let data = |kind: u32, value: &[u8]| bx(b"data", &cat(&[u32s(&[kind, 0]), value.to_vec()]));
    let (jpeg, png, art) = (
        data(13, &[0xFF, 0xD8, 1]),
        data(14, &[0x89, b'P', 2]),
        data(13, &[0xFF, 0xD8, 3]),
    );
    let title = bx(b"\xa9nam", &data(1, b"title"));
    let udta = bx(
        b"udta",
        &full(
            b"meta",
            0,
            0,
            &cat(&[
                full(b"hdlr", 0, 0, b"\0\0\0\0mdirappl\0\0\0\0\0\0\0\0\0"),
                bx(
                    b"ilst",
                    &cat(&[
                        title.clone(),
                        bx(b"covr", &cat(&[jpeg.clone(), png.clone()])),
                    ]),
                ),
            ]),
        ),
    );
    let key = "com.apple.quicktime.artwork";
    let keys = cat(&[
        u32s(&[0, 1, key.len() as u32 + 8]),
        b"mdta".to_vec(),
        key.as_bytes().to_vec(),
    ]);
    let meta = bx(
        b"meta",
        &cat(&[
            full(b"hdlr", 0, 0, b"\0\0\0\0mdta\0\0\0\0\0\0\0\0\0\0\0\0\0"),
            bx(b"keys", &keys),
            bx(b"ilst", &bx(&1u32.to_be_bytes(), &art)),
        ]),
    );
    let mut f = file(vec![video(10, 5), aac(10, false)]);
    f.moov_extra = vec![udta, meta];
    let source = build(&f);
    let value =
        |data: &[u8]| source.windows(data.len()).position(|w| w == data).unwrap() as u64 + 16;
    let contains = |output: &[u8], needle: &[u8]| output.windows(needle.len()).any(|w| w == needle);
    let left_out = |offsets: Vec<u64>| {
        remux(
            &source,
            &RemuxOptions {
                left_out_covers: offsets,
                ..RemuxOptions::default()
            },
        )
    };

    let (summary, output) = left_out(vec![value(&jpeg)]).unwrap();
    assert_round_trip(&source, &output, &summary);
    assert!(contains(&output, &bx(b"covr", &png)) && contains(&output, &title));
    assert!(!contains(&output, &jpeg) && contains(&output, &art));
    assert_eq!(paths(&summary), ["moov/udta/meta/ilst/covr/data"]);
    assert_eq!(summary.dropped_boxes[0].offset, value(&jpeg) - 16);
    assert_eq!(summary.dropped_boxes[0].reason, BoxDropReason::NotCarried);

    let (summary, output) = left_out(vec![value(&art), value(&png), value(&jpeg)]).unwrap();
    assert_round_trip(&source, &output, &summary);
    assert!(contains(&output, &bx(b"ilst", &title)) && contains(&output, &bx(b"keys", &keys)));
    assert!(!contains(&output, &jpeg) && !contains(&output, &png) && !contains(&output, &art));
    assert_eq!(summary.dropped_boxes.len(), 2);
    assert_eq!(paths(&summary)[0], "moov/udta/meta/ilst/covr");

    let (_, output) = remux(&source, &RemuxOptions::default()).unwrap();
    assert!(contains(&output, &jpeg) && contains(&output, &png) && contains(&output, &art));
    for offsets in [vec![value(&jpeg) + 1], vec![0]] {
        assert!(matches!(
            left_out(offsets),
            Err(ExtractError::Source(Error::NoSuchTrack))
        ));
    }
}
