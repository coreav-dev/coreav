//! Synthetic movies (ISO/IEC 14496-12) for the writer's tests: one
//! H.264-style video track and AAC audio, each track's samples in one chunk.
#![allow(dead_code, clippy::indexing_slicing)]

use crate::common::{bx, cat, full, u32s};
use coreav_isobmff::access::{read_av1_sync, read_frame_access};
use coreav_isobmff::{Codec, Movie, Packet, SliceSource, Track, TrackKind};

/// One synthetic sample: size, decode duration, composition offset, sync.
#[derive(Clone, Copy)]
pub struct S {
    pub size: u32,
    pub duration: u32,
    pub offset: i32,
    pub sync: bool,
}

/// The `hvcC` an `hvc1` track gets (ISO/IEC 14496-15 8.3.3.1): a Main
/// profile record of one sequence parameter set, 1280x720 4:2:0 with a
/// conformance window of 625 + 20 chroma columns, which leaves no picture
/// (ITU-T H.265 7.4.3.2.1: 2 * (625 + 20) is not below 1280). The set is
/// the one `scripts/gen-fixtures.sh` writes into `size_hevc_window.mp4`.
pub const EMPTY_WINDOW_HVCC: &[u8] = &[
    0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5d, 0xf0, 0x00, 0xfc,
    0xfd, 0xf8, 0xf8, 0x00, 0x00, 0x0f, 0x01, 0xa1, 0x00, 0x01, 0x00, 0x2f, 0x42, 0x01, 0x01, 0x01,
    0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x5d, 0xa0, 0x02,
    0x80, 0x80, 0x2d, 0x18, 0x02, 0x72, 0x0a, 0xf9, 0x65, 0x66, 0x92, 0x4c, 0xaf, 0x01, 0x68, 0x08,
    0x00, 0x00, 0x03, 0x00, 0x08, 0x00, 0x00, 0x03, 0x00, 0xc8, 0x40,
];

pub struct TrackSpec {
    pub video: bool,
    pub timescale: u32,
    pub samples: Vec<S>,
    /// Edit list entries `(duration in movie ticks, media time)`.
    pub edits: Vec<(u32, i32)>,
    pub entry: [u8; 4],
}

/// A movie of the given tracks, movie timescale 1000, each track's samples
/// in one chunk after the movie box.
pub fn movie(tracks: &[TrackSpec]) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    // Two layouts: first with zero offsets to size the movie box.
    let build = |offsets: &[u32]| -> Vec<u8> {
        let mut traks = Vec::new();
        for (i, t) in tracks.iter().enumerate() {
            traks.push(trak(t, i as u32 + 1, offsets[i]));
        }
        let duration = 10_000u32;
        let mut mvhd = u32s(&[0, 0, 1000, duration]);
        mvhd.extend_from_slice(&[0u8; 80]);
        let mut body = full(b"mvhd", 0, 0, &mvhd);
        for t in traks {
            body.extend_from_slice(&t);
        }
        bx(b"moov", &body)
    };
    let moov_len = build(&vec![0; tracks.len()]).len();
    let mut offsets = Vec::new();
    let mut at = (ftyp.len() + moov_len + 8) as u32;
    let mut data = Vec::new();
    for (i, t) in tracks.iter().enumerate() {
        offsets.push(at);
        for (n, s) in t.samples.iter().enumerate() {
            data.extend((0..s.size).map(|b| (b as usize + n * 7 + i * 101) as u8));
            at += s.size;
        }
    }
    let moov = build(&offsets);
    assert_eq!(moov.len(), moov_len);
    cat(&[ftyp, moov, bx(b"mdat", &data)])
}

pub fn trak(t: &TrackSpec, id: u32, chunk_offset: u32) -> Vec<u8> {
    let n = t.samples.len() as u32;
    let media: u32 = t.samples.iter().map(|s| s.duration).sum();
    let mut tkhd = u32s(&[0, 0, id, 0, 0]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]));
    tkhd.extend_from_slice(&u32s(if t.video {
        &[320 << 16, 240 << 16]
    } else {
        &[0, 0]
    }));
    let mut mdhd = u32s(&[0, 0, t.timescale, media]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(if t.video { b"vide" } else { b"soun" });
    hdlr.extend_from_slice(&[0u8; 13]);
    let entry = if t.video {
        let mut body = vec![0u8; 6];
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&[0u8; 16]);
        body.extend_from_slice(&320u16.to_be_bytes());
        body.extend_from_slice(&240u16.to_be_bytes());
        body.extend_from_slice(&[0u8; 50]);
        if &t.entry == b"hvc1" {
            body.extend_from_slice(&bx(b"hvcC", EMPTY_WINDOW_HVCC));
        } else {
            // avcC: version 1, High profile, level 3.0; no parameter sets.
            body.extend_from_slice(&bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]));
        }
        body.extend_from_slice(&bx(b"pasp", &u32s(&[1, 1])));
        body.extend_from_slice(&bx(b"fiel", &[1, 0]));
        bx(&t.entry, &body)
    } else {
        let mut body = vec![0u8; 6];
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&[0u8; 8]);
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&16u16.to_be_bytes());
        body.extend_from_slice(&[0u8; 4]);
        body.extend_from_slice(&(t.timescale << 16).to_be_bytes());
        body.extend_from_slice(&full(
            b"esds",
            0,
            0,
            &[
                3, 25, 0, 1, 0, 4, 17, 0x40, 0x15, 0, 0, 0, 0, 0, 0xfa, 0, 0, 0, 0xfa, 0, 5, 2,
                0x11, 0x90, 6, 1, 2,
            ],
        ));
        bx(&t.entry, &body)
    };
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&entry);
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
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let media_header = if t.video {
        full(b"vmhd", 0, 1, &[0; 8])
    } else {
        full(b"smhd", 0, 0, &[0; 4])
    };
    let minf = bx(
        b"minf",
        &cat(&[
            media_header,
            bx(b"dinf", &dref),
            bx(b"stbl", &stbl.concat()),
        ]),
    );
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    let mut parts = vec![full(b"tkhd", 0, 3, &tkhd)];
    if !t.edits.is_empty() {
        let mut elst = u32s(&[t.edits.len() as u32]);
        for (duration, time) in &t.edits {
            elst.extend_from_slice(&u32s(&[*duration, *time as u32, 0x10000]));
        }
        parts.push(bx(b"edts", &full(b"elst", 0, 0, &elst)));
    }
    parts.push(mdia);
    bx(b"trak", &parts.concat())
}

/// H.264-style video at 25 frames per second (timescale 12,800, 512 ticks a
/// frame) in GOPs of `gop` frames: I P B B ... in decode order, with the
/// two-frame delay of one reordered B pair removed by the edit list.
pub fn ibbp_video(frames: usize, gop: usize) -> TrackSpec {
    let mut samples = Vec::new();
    for n in 0..frames {
        let in_gop = n % gop;
        // Decode order I P B B P B B ...: P frames are presented two frames
        // after their decode slot, B frames one frame before it; every
        // offset includes the two-frame delay (1,024 ticks).
        let offset = match in_gop {
            0 => 1024,
            k if k % 3 == 1 => 2048,
            _ => 512,
        };
        samples.push(S {
            size: 40 + (n as u32 % 13),
            duration: 512,
            offset,
            sync: in_gop == 0,
        });
    }
    TrackSpec {
        video: true,
        timescale: 12_800,
        edits: vec![((frames as u32) * 40, 1024)],
        samples,
        entry: *b"avc1",
    }
}

/// AAC at 48 kHz: frames of 1024 ticks with a priming edit of 2,112 ticks.
pub fn aac_audio(frames: usize, edits: Vec<(u32, i32)>) -> TrackSpec {
    TrackSpec {
        video: false,
        timescale: 48_000,
        samples: (0..frames)
            .map(|n| S {
                size: 20 + (n as u32 % 5),
                duration: 1024,
                offset: 0,
                sync: true,
            })
            .collect(),
        edits,
        entry: *b"mp4a",
    }
}

pub fn av_movie() -> Vec<u8> {
    // 4 s of video in GOPs of 25 frames (1 s), 4 s of audio after priming.
    movie(&[ibbp_video(100, 25), aac_audio(190, vec![(4000, 2112)])])
}

/// Overwrites each sample of track 0 of `source` with one H.264 NAL unit
/// (4-byte length): an IDR slice where `idr` is true, a P slice elsewhere
/// (docs/design/cut-points.md: the cut rule reads them).
#[allow(dead_code)]
pub fn h264_slices(source: &mut [u8], idr: &[bool]) {
    let kinds: String = idr.iter().map(|i| if *i { 'I' } else { 'P' }).collect();
    h264_frames(source, &kinds);
}

/// [`h264_slices`] with one letter per sample: `I` an IDR slice, `P` a P
/// slice, `R` an exact recovery point SEI (recovery_frame_cnt 0) before a P
/// slice.
#[allow(dead_code)]
pub fn h264_frames(source: &mut [u8], kinds: &str) {
    let limits = coreav_isobmff::Limits::default();
    let movie = coreav_isobmff::probe(
        coreav_isobmff::SliceSource::new(source),
        &limits,
        &coreav_isobmff::Allowlist::default(),
    )
    .unwrap();
    let mut budget = coreav_isobmff::Budget::new(&limits);
    let packets: Vec<_> = movie
        .packets(
            0,
            coreav_isobmff::SliceSource::new(source),
            &limits,
            &mut budget,
        )
        .unwrap()
        .map(|p| p.unwrap())
        .collect();
    for (packet, kind) in packets.iter().zip(kinds.chars()) {
        let at = packet.offset as usize;
        let size = packet.size as usize;
        let mut nal = Vec::new();
        if kind == 'R' {
            // payloadType 6, size 1: recovery_frame_cnt 0 (ue 1),
            // exact_match_flag 1, broken_link_flag 0; trailing bits.
            nal.extend_from_slice(&[0, 0, 0, 5, 0x06, 6, 1, 0b1100_0000, 0x80]);
        }
        // first_mb_in_slice 0, then slice_type 7 (I) or 0 (P).
        let head: [u8; 2] = if kind == 'I' {
            [0x65, 0x88]
        } else {
            [0x41, 0xC0]
        };
        let rest = size - nal.len() - 4;
        nal.extend_from_slice(&(rest as u32).to_be_bytes());
        nal.extend_from_slice(&head);
        nal.resize(size, 0);
        source[at..at + size].copy_from_slice(&nal);
    }
}

/// Whether `packet`, a sample of video `track` of `movie` read from
/// `data`, is written as a sync sample by `clip` and `split`
/// (docs/design/cut-points.md, "The output's sync flags"): the container
/// marks it, and for H.264 the frame starts decoding by itself (an
/// unreadable frame keeps a stated flag), for HEVC it starts with an IRAP
/// picture (`nal_unit_type` 16 to 23, ITU-T H.265 7.4.2.2), for AV1 it is a
/// sync sample as the AV1 ISOBMFF binding defines one.
pub fn written_sync(data: &[u8], movie: &Movie, track: &Track, packet: &Packet) -> bool {
    if !packet.keyframe || track.kind != TrackKind::Video {
        return packet.keyframe;
    }
    if let Some(format) = movie.access_format(track.index) {
        let mut source = SliceSource::new(data);
        let len = u64::from(packet.size);
        let frame = read_frame_access(format, &mut source, packet.offset, len, &mut || Ok(()));
        return match frame.unwrap() {
            Some(frame) => frame.starts_decoding(movie.max_num_ref_frames(track.index)),
            None => track.sync_table() || packet.sample >= u64::from(track.sample_count),
        };
    }
    if track.description.codec == Codec::Av1 {
        let mut source = SliceSource::new(data);
        let len = u64::from(packet.size);
        return read_av1_sync(&mut source, packet.offset, len, &mut || Ok(()))
            .unwrap()
            .unwrap_or(track.sync_table() || packet.sample >= u64::from(track.sample_count));
    }
    if track.description.codec == Codec::Hevc {
        let config = &track.description.codec_config.as_ref().unwrap().data;
        let length = usize::from(config[21] & 3) + 1;
        let (mut at, end) = (
            packet.offset as usize,
            (packet.offset + u64::from(packet.size)) as usize,
        );
        while at + length < end {
            let size = data[at..at + length]
                .iter()
                .fold(0usize, |n, b| (n << 8) | usize::from(*b));
            let kind = (data[at + length] >> 1) & 0x3f;
            if kind < 32 {
                return (16..=23).contains(&kind);
            }
            at += length + size;
        }
        return false;
    }
    true
}

/// `data` with every track's sync sample table turned into a `free` box of
/// the same size, so that every sample is a sync sample (ISO/IEC 14496-12
/// 8.6.2). Walks `moov`, `trak`, `mdia`, `minf` and `stbl`.
pub fn without_sync_table(data: &[u8]) -> Vec<u8> {
    fn walk(out: &mut [u8], start: usize, end: usize) {
        const CONTAINERS: [&[u8; 4]; 5] = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];
        let mut at = start;
        while at + 8 <= end {
            let size = u32::from_be_bytes(out[at..at + 4].try_into().unwrap()) as usize;
            let (header, size) = match size {
                0 => (8, end - at),
                1 => (
                    16,
                    u64::from_be_bytes(out[at + 8..at + 16].try_into().unwrap()) as usize,
                ),
                n => (8, n),
            };
            if size < header || at + size > end {
                return;
            }
            let kind: [u8; 4] = out[at + 4..at + 8].try_into().unwrap();
            if kind == *b"stss" {
                out[at + 4..at + 8].copy_from_slice(b"free");
            } else if CONTAINERS.contains(&&kind) {
                walk(out, at + header, at + size);
            }
            at += size;
        }
    }
    let mut out = data.to_vec();
    let end = out.len();
    walk(&mut out, 0, end);
    out
}

/// A fragmented H.264 video of `frames` 40-byte samples of 512 ticks at
/// 12,800 ticks a second, coded as `kinds` (see `h264_frames`). Its samples
/// take their size, duration and flags from the track defaults (`trex`),
/// which mark every sample as a sync sample (ISO/IEC 14496-12 8.8.3), in
/// one fragment.
pub fn fragmented_video(kinds: &str) -> Vec<u8> {
    use crate::common::visual_entry;
    let frames = kinds.len() as u32;
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
    let mut mdhd = u32s(&[0, 0, 12_800, 0]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"vide");
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&visual_entry(
        b"avc1",
        &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]),
    ));
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
    let mdia = bx(
        b"mdia",
        &cat(&[
            full(b"mdhd", 0, 0, &mdhd),
            full(b"hdlr", 0, 0, &hdlr),
            bx(b"minf", &stbl),
        ]),
    );
    // trex: track 1, description 1, 512 ticks, 40 bytes, flags 0 (sync).
    let mvex = bx(b"mvex", &full(b"trex", 0, 0, &u32s(&[1, 1, 512, 40, 0])));
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia])),
            mvex,
        ]),
    );
    let tfhd = full(b"tfhd", 0, 0x02_0000, &u32s(&[1]));
    let tfdt = full(b"tfdt", 0, 0, &u32s(&[0]));
    let traf_len = 8 + tfhd.len() + tfdt.len() + 20;
    let moof_len = 8 + 16 + traf_len;
    let trun = full(b"trun", 0, 0x1, &u32s(&[frames, (moof_len + 8) as u32]));
    let moof = bx(
        b"moof",
        &cat(&[
            full(b"mfhd", 0, 0, &u32s(&[1])),
            bx(b"traf", &cat(&[tfhd, tfdt, trun])),
        ]),
    );
    assert_eq!(moof.len(), moof_len);
    let mdat = bx(b"mdat", &vec![0; 40 * frames as usize]);
    let mut source = cat(&[ftyp, moov, moof, mdat]);
    h264_frames(&mut source, kinds);
    source
}
