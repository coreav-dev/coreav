//! Extraction round trips: every output is probed by
//! `coreav-isobmff`, and its stream and packets must equal the source
//! track's.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use coreav_isobmff::{
    Allowlist, Budget, Codec, EditList, Error, Limits, Movie, Packet, SliceSource, Source, Track,
    probe,
};
use coreav_isobmff_write::{ExtractError, Summary, TrackChoice, extract_audio};
use std::io::{self, Write};
use std::path::Path;

use common::{bx, cat, full, u32s};

/// An AAC LC, 48 kHz stereo `esds` (ISO/IEC 14496-1 7.2.6.5, 14496-3 1.6.2.1).
fn esds() -> Vec<u8> {
    esds_with(&[0x11, 0x90])
}

/// An `esds` holding the `AudioSpecificConfig` `config`.
fn esds_with(config: &[u8]) -> Vec<u8> {
    let len = config.len() as u8;
    full(
        b"esds",
        0,
        0,
        &cat(&[
            vec![3, 23 + len, 0, 1, 0], // ES_Descriptor
            vec![
                4,
                15 + len,
                0x40,
                0x15,
                0,
                0,
                0,
                0,
                0,
                0xfa,
                0,
                0,
                0,
                0xfa,
                0,
            ], // DecoderConfigDescriptor
            vec![5, len],               // DecoderSpecificInfo
            config.to_vec(),
            vec![6, 1, 2], // SLConfigDescriptor
        ]),
    )
}

/// A one-track AAC movie: `n` samples of `size` bytes and 1024 ticks at
/// 48 kHz, movie timescale 1000, an optional simple edit (empty, media time,
/// segment) and `first` samples per chunk after the first chunk.
fn aac_movie(n: u32, size: u32, edit: Option<(u32, i32, u32)>, version_1_entry: bool) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let data: Vec<u8> = (0..n * size).map(|i| (i % 251) as u8).collect();
    let mdat = bx(b"mdat", &data);
    let moov = aac_moov(n, size, edit, version_1_entry, (ftyp.len() + 8) as u32);
    cat(&[ftyp, mdat, moov])
}

/// The movie box of [`aac_movie`], with its one chunk at `chunk_offset`.
fn aac_moov(
    n: u32,
    size: u32,
    edit: Option<(u32, i32, u32)>,
    version_1_entry: bool,
    chunk_offset: u32,
) -> Vec<u8> {
    aac_moov_chunks(n, size, edit, version_1_entry, n, &[chunk_offset])
}

/// The movie box of [`aac_movie`] with `per_chunk` samples in each chunk,
/// the chunks at `chunk_offsets`.
fn aac_moov_chunks(
    n: u32,
    size: u32,
    edit: Option<(u32, i32, u32)>,
    version_1_entry: bool,
    per_chunk: u32,
    chunk_offsets: &[u32],
) -> Vec<u8> {
    audio_moov(
        (&[0x11, 0x90], 48_000),
        n,
        size,
        edit,
        version_1_entry,
        per_chunk,
        chunk_offsets,
    )
}

/// [`aac_moov_chunks`] for the `AudioSpecificConfig` and the rate of
/// `audio`, which is the media timescale and the sample entry's rate.
fn audio_moov(
    (config, rate): (&[u8], u32),
    n: u32,
    size: u32,
    edit: Option<(u32, i32, u32)>,
    version_1_entry: bool,
    per_chunk: u32,
    chunk_offsets: &[u32],
) -> Vec<u8> {
    let media = n * 1024;
    let ms = (u64::from(media) * 1000 / u64::from(rate)) as u32;
    let mut mvhd = u32s(&[0x7c25_b080, 0x7c25_b080, 1000, ms]);
    mvhd.extend_from_slice(&[0u8; 80]);
    let mut tkhd = u32s(&[0, 0, 7, 0, ms]);
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
    let mut mdhd = u32s(&[0x7c25_b081, 0x7c25_b081, rate, media]);
    // "fra"
    mdhd.extend_from_slice(&[0x1a, 0x41, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut entry = vec![0u8; 6];
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&if version_1_entry { 1u16 } else { 0 }.to_be_bytes());
    entry.extend_from_slice(&[0u8; 6]);
    entry.extend_from_slice(&2u16.to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes());
    entry.extend_from_slice(&[0u8; 4]);
    entry.extend_from_slice(&(rate << 16).to_be_bytes());
    if version_1_entry {
        // QuickTime version 1: four more 32-bit fields.
        entry.extend_from_slice(&u32s(&[1024, 0, 0, 2]));
    }
    entry.extend_from_slice(&esds_with(config));
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&bx(b"mp4a", &entry));
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &u32s(&[1, n, 1024])),
            full(b"stsc", 0, 0, &u32s(&[1, 1, per_chunk, 1])),
            full(b"stsz", 0, 0, &u32s(&[size, n])),
            full(
                b"stco",
                0,
                0,
                &[u32s(&[chunk_offsets.len() as u32]), u32s(chunk_offsets)].concat(),
            ),
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
    let mut trak = vec![full(b"tkhd", 0, 3, &tkhd)];
    if let Some((empty, media_time, segment)) = edit {
        let mut entries = Vec::new();
        if empty > 0 {
            entries.push((empty, -1));
        }
        entries.push((segment, media_time));
        let mut elst = u32s(&[entries.len() as u32]);
        for (duration, time) in entries {
            elst.extend_from_slice(&u32s(&[duration, time as u32, 0x10000]));
        }
        trak.push(bx(b"edts", &full(b"elst", 0, 0, &elst)));
    }
    trak.push(mdia);
    bx(
        b"moov",
        &cat(&[full(b"mvhd", 0, 0, &mvhd), bx(b"trak", &cat(&trak))]),
    )
}

/// A fragmented one-track AAC movie (48 kHz, empty movie box tables): one
/// fragment per entry of `base_times`, each with two 10-byte samples of
/// 1024 ticks (from `trex`) starting at that base media decode time.
fn fragmented_aac(base_times: &[u32]) -> Vec<u8> {
    fragmented_aac_with_edit(base_times, None)
}

/// [`fragmented_aac`] with an optional single media edit (segment duration
/// in movie ticks, media time).
fn fragmented_aac_with_edit(base_times: &[u32], edit: Option<(u32, i32)>) -> Vec<u8> {
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
    let edts = edit.map_or_else(Vec::new, |(segment, media_time)| {
        bx(
            b"edts",
            &full(
                b"elst",
                0,
                0,
                &u32s(&[1, segment, media_time as u32, 0x10000]),
            ),
        )
    });
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), edts, mdia])),
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
fn an_edit_that_lasts_to_the_end_of_the_media_is_refused() {
    let source = fragmented_aac_with_edit(&[0, 2048], Some((0, 1024)));
    let mut out = Vec::new();
    let result = extract_audio(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
        TrackChoice::FirstAudio,
        &mut out,
    );
    assert_eq!(
        result,
        Err(ExtractError::Source(Error::Unsupported {
            reason: "an edit that lasts to the end of the media"
        }))
    );
    assert!(out.is_empty());
}

#[test]
fn fragmented_sources_need_contiguous_decode_times() {
    for bases in [&[0, 2048][..], &[0, 2048, 4096]] {
        let source = fragmented_aac(bases);
        let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
        assert_eq!(summary.samples as usize, bases.len() * 2);
        assert_round_trip(&source, 0, &output, &summary);
    }
    // A gap or an overlap would move later audio, and a late start without
    // an edit list would move all of it; each fails instead.
    for (bases, reason) in [
        ([0, 4096], "a gap or an overlap between decode times"),
        ([0, 1024], "a gap or an overlap between decode times"),
        (
            [2048, 4096],
            "first decode time is not 0 and there is no edit list",
        ),
    ] {
        let source = fragmented_aac(&bases);
        let mut out = Vec::new();
        let result = extract_audio(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            TrackChoice::FirstAudio,
            &mut out,
        );
        assert_eq!(
            result,
            Err(ExtractError::Source(Error::Unsupported { reason })),
            "{bases:?}"
        );
        assert!(out.is_empty());
    }
}

/// A source of `len` bytes: `head`, then a repeating pattern.
struct Virtual {
    head: Vec<u8>,
    len: u64,
}

impl Source for Virtual {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.len)
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::Truncated { offset })?;
        if end > self.len {
            return Err(Error::Truncated { offset });
        }
        for (i, byte) in buf.iter_mut().enumerate() {
            let at = offset + i as u64;
            *byte = match self.head.get(at as usize) {
                Some(b) if at < self.head.len() as u64 => *b,
                _ => (at % 251) as u8,
            };
        }
        Ok(())
    }
}

/// Keeps the first `keep` bytes written and counts the rest.
struct Head {
    keep: usize,
    bytes: Vec<u8>,
    total: u64,
}

impl Write for Head {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = self.keep.saturating_sub(self.bytes.len()).min(buf.len());
        self.bytes.extend_from_slice(&buf[..room]);
        self.total += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Outputs above 4 GiB use `co64` chunk offsets and a 64-bit `mdat` size,
/// from a virtual 4.4 GB source.
#[test]
#[ignore = "copies 4.4 GB from a virtual file; run with --ignored --release"]
fn outputs_above_4_gib_use_64_bit_offsets() {
    let (n, size) = (1100u32, 4u32 << 20);
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let draft = aac_moov(n, size, None, false, 0);
    let data_at = (ftyp.len() + draft.len() + 16) as u32;
    let mut head = cat(&[ftyp, aac_moov(n, size, None, false, data_at)]);
    let data = u64::from(n) * u64::from(size);
    head.extend_from_slice(&1u32.to_be_bytes());
    head.extend_from_slice(b"mdat");
    head.extend_from_slice(&(data + 16).to_be_bytes());
    let len = head.len() as u64 + data;
    let mut out = Head {
        keep: 1 << 20,
        bytes: Vec::new(),
        total: 0,
    };
    let summary = extract_audio(
        Virtual { head, len },
        &Limits::default(),
        &Allowlist::default(),
        TrackChoice::FirstAudio,
        &mut out,
    )
    .unwrap();
    assert_eq!(summary.bytes_written, out.total);
    assert!(out.total > u64::from(u32::MAX));
    let moov_len = u32::from_be_bytes(out.bytes[28..32].try_into().unwrap()) as usize;
    let mdat = &out.bytes[28 + moov_len..28 + moov_len + 16];
    assert_eq!(&mdat[..8], b"\0\0\0\x01mdat");
    assert_eq!(u64::from_be_bytes(mdat[8..].try_into().unwrap()), data + 16);
    assert!(out.bytes.windows(4).any(|w| w == b"co64"));
    // The output's header boxes, re-read over a virtual file of the output's
    // length: every chunk offset must land on its sample in the 64-bit range.
    let mut copy = Virtual {
        head: out.bytes[..28 + moov_len + 16].to_vec(),
        len: out.total,
    };
    let movie = probe(&mut copy, &Limits::default(), &Allowlist::default()).unwrap();
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    let packets: Vec<Packet> = movie
        .packets(0, &mut copy, &limits, &mut budget)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(packets.len(), n as usize);
    let first = packets[0].offset;
    assert_eq!(first, (28 + moov_len + 16) as u64);
    for (i, p) in packets.iter().enumerate() {
        assert_eq!(p.offset, first + i as u64 * u64::from(size));
        assert_eq!(p.size, size);
    }
}

fn extract(data: &[u8], choice: TrackChoice) -> Result<(Summary, Vec<u8>), ExtractError> {
    let mut out = Vec::new();
    let summary = extract_audio(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
        choice,
        &mut out,
    )?;
    Ok((summary, out))
}

fn packets(movie: &Movie, track: &Track, data: &[u8]) -> Vec<Packet> {
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    movie
        .packets(track.index, SliceSource::new(data), &limits, &mut budget)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn bytes_of(data: &[u8], packet: &Packet) -> Vec<u8> {
    let mut buf = vec![0; packet.size as usize];
    SliceSource::new(data)
        .read_exact_at(packet.offset, &mut buf)
        .unwrap();
    buf
}

/// Checks that `output` holds exactly the track `stream_index` of `source`.
fn assert_round_trip(source: &[u8], stream_index: usize, output: &[u8], summary: &Summary) {
    let limits = Limits::default();
    let src_movie = probe(SliceSource::new(source), &limits, &Allowlist::default()).unwrap();
    let src = src_movie
        .tracks
        .iter()
        .find(|t| t.stream_index == stream_index)
        .unwrap();
    let movie = probe(SliceSource::new(output), &limits, &Allowlist::default()).unwrap();
    assert_eq!(movie.major_brand.map(|b| b.0), Some(*b"M4A "));
    assert!(!movie.fragmented);
    assert!(movie.dropped.is_empty());
    assert_eq!(movie.tracks.len(), 1);
    assert_eq!(movie.creation_time, src_movie.creation_time);
    let out = &movie.tracks[0];
    assert_eq!(out.warnings, src.warnings);
    assert_eq!(out.description.codec, Codec::Aac);
    assert_eq!(out.description.fourcc.0, *b"mp4a");
    assert_eq!(out.description.channels, src.description.channels);
    assert_eq!(out.description.sample_rate, src.description.sample_rate);
    assert_eq!(out.description.profile, src.description.profile);
    assert_eq!(out.description.codec_config, src.description.codec_config);
    assert_eq!(out.descriptions, 1);
    assert_eq!(out.timescale, src.timescale);
    assert_eq!(out.media_creation_time, src.media_creation_time);
    let language = src
        .language
        .filter(|l| l.iter().all(u8::is_ascii_lowercase))
        .unwrap_or(*b"und");
    assert_eq!(out.language, Some(language));

    // The declared media duration is copied.
    if !src_movie.has_fragments() && src.mdhd_duration != 0 {
        assert_eq!(out.mdhd_duration, src.mdhd_duration);
    }
    // Amendment point 2: an edit list that the output has and the source
    // has not comes from the source's `iTunSMPB` item.
    let gapless = match (src.edit_list, out.edit_list) {
        (EditList::None, EditList::Simple { .. }) => {
            src_movie.itunes_gapless(SliceSource::new(source)).unwrap()
        }
        _ => None,
    };

    let src_packets = packets(&src_movie, src, source);
    let out_packets = packets(&movie, out, output);
    assert_eq!(out_packets.len(), src_packets.len());
    assert_eq!(summary.samples as usize, src_packets.len());
    assert_eq!(summary.bytes_written, output.len() as u64);
    assert_eq!(summary.stream_index, stream_index);
    let first = src_packets.first().map_or(0, |p| p.dts);
    for (a, b) in src_packets.iter().zip(&out_packets) {
        assert_eq!(b.size, a.size, "sample {}", a.sample);
        assert_eq!(b.duration, a.duration, "sample {}", a.sample);
        assert_eq!(b.dts, a.dts - first, "sample {}", a.sample);
        assert_eq!(b.keyframe, a.keyframe, "sample {}", a.sample);
        if gapless.is_none() {
            assert_eq!(b.pts, a.pts, "sample {}", a.sample);
            assert_eq!(b.presented, a.presented, "sample {}", a.sample);
        }
        assert_eq!(
            bytes_of(output, b),
            bytes_of(source, a),
            "sample {}",
            a.sample
        );
    }
    assert_eq!(
        summary.duration,
        out_packets
            .iter()
            .map(|p| u64::from(p.duration))
            .sum::<u64>()
    );
    match (src.edit_list, out.edit_list) {
        (EditList::None, EditList::None) => {}
        (
            EditList::Simple {
                empty: se,
                media_time: sm,
                segment: ss,
            },
            EditList::Simple {
                empty: oe,
                media_time: om,
                segment: os,
            },
        ) => {
            assert_eq!(movie.timescale, src_movie.timescale);
            assert_eq!((oe, os, om), (se, ss, sm - first));
        }
        (
            EditList::None,
            EditList::Simple {
                empty,
                media_time,
                segment,
            },
        ) => {
            let g = gapless.expect("an added edit list needs an iTunSMPB item");
            assert_eq!(first, 0);
            assert_eq!(empty, 0);
            assert_eq!(media_time, i64::from(g.encoder_delay));
            let media = u64::from(src.timescale);
            let movie_ts = u64::from(movie.timescale);
            assert_eq!(segment, (g.original_samples * movie_ts + media / 2) / media);
            assert_eq!(
                u64::from(g.encoder_delay) + u64::from(g.padding) + g.original_samples,
                summary.duration
            );
        }
        (EditList::Multiple, EditList::Multiple) => {
            // Several media edits are the source's entries with their media
            // times moved by the first decode time, which output decode
            // times leave at 0 (docs/design/edit-list-writers.md, "M4A
            // extraction").
            assert_eq!(movie.timescale, src_movie.timescale);
            assert_eq!(out.edits.len(), src.edits.len());
            for (a, b) in src.edits.iter().zip(&out.edits) {
                assert_eq!(b.segment_duration, a.segment_duration);
                assert_eq!(b.media_rate, a.media_rate);
                let moved = if a.media_time == -1 {
                    -1
                } else {
                    a.media_time - first
                };
                assert_eq!(b.media_time, moved);
            }
        }
        other => panic!("edit lists differ: {other:?}"),
    }
}

/// `moov` of `movie` (an [`aac_movie`]) with `extra` appended to its children.
fn with_moov_children(movie: &[u8], extra: &[u8]) -> Vec<u8> {
    let at = movie.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let children = &movie[at + 8..];
    cat(&[
        movie[..at].to_vec(),
        bx(b"moov", &cat(&[children.to_vec(), extra.to_vec()])),
    ])
}

/// `udta`/`meta`/`ilst` with one iTunes `iTunSMPB` item holding `text`.
fn smpb_udta(text: &str) -> Vec<u8> {
    let data = cat(&[u32s(&[1, 0]), text.as_bytes().to_vec()]);
    let item = bx(
        b"----",
        &cat(&[
            full(b"mean", 0, 0, b"com.apple.iTunes"),
            full(b"name", 0, 0, b"iTunSMPB"),
            bx(b"data", &data),
        ]),
    );
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"mdirappl");
    hdlr.extend_from_slice(&[0u8; 9]);
    let meta = full(
        b"meta",
        0,
        0,
        &cat(&[full(b"hdlr", 0, 0, &hdlr), bx(b"ilst", &item)]),
    );
    bx(b"udta", &meta)
}

#[test]
fn declared_media_duration_is_copied() {
    // The media header declares 100 ticks less than the 100 samples of 1024.
    let mut source = aac_movie(100, 10, None, false);
    let at = source.windows(4).position(|w| w == b"mdhd").unwrap() + 4 + 16;
    source[at..at + 4].copy_from_slice(&(100 * 1024 - 100u32).to_be_bytes());
    let src = probe(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert!(!src.tracks[0].warnings.is_empty());
    let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
    assert_eq!(summary.duration, 100 * 1024);
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(out.tracks[0].mdhd_duration, 100 * 1024 - 100);
    assert_eq!(out.tracks[0].duration, src.tracks[0].duration);
    assert_round_trip(&source, 0, &output, &summary);
}

#[test]
fn a_matching_gapless_item_becomes_an_edit_list() {
    // 100 samples of 1024 = 102,400 = 2,112 delay + 100,000 + 288 padding.
    let plain = aac_movie(100, 10, None, false);
    let source = with_moov_children(
        &plain,
        &smpb_udta(" 00000000 00000840 00000120 00000000000186A0 00000000"),
    );
    let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    // 100,000 samples at 48 kHz in movie ticks of 1 ms, rounded: 2,083.
    assert_eq!(
        out.tracks[0].edit_list,
        EditList::Simple {
            empty: 0,
            media_time: 2112,
            segment: 2083
        }
    );
    assert_round_trip(&source, 0, &output, &summary);
    // The item itself is not copied: no `udta` or `meta` box of the source is.
    assert!(!output.windows(8).any(|w| w == b"iTunSMPB"));

    // An item that does not add up to the samples, one that describes a
    // longer recording, and one beside an edit list change nothing.
    for text in [
        " 00000000 00000840 00000120 00000000000186A1",
        " 00000000 00000840 000003DC 00000000004D8BE4",
        " 00000000 00000840 00000120",
    ] {
        let source = with_moov_children(&plain, &smpb_udta(text));
        let (_, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
        let out = probe(
            SliceSource::new(&output),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        assert_eq!(out.tracks[0].edit_list, EditList::None, "{text}");
    }
    let edited = aac_movie(100, 10, Some((0, 1024, 2000)), false);
    let source = with_moov_children(
        &edited,
        &smpb_udta(" 00000000 00000840 00000120 00000000000186A0"),
    );
    let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
    let out = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(
        out.tracks[0].edit_list,
        EditList::Simple {
            empty: 0,
            media_time: 1024,
            segment: 2000
        }
    );
    assert_round_trip(&source, 0, &output, &summary);
}

/// A raw AAC frame of `size` bytes that ends with SBR data: a fill element
/// of two bytes with `EXT_SBR_DATA`, then `ID_END` and byte alignment
/// (ISO/IEC 14496-3 4.4.2.1, 4.4.2.7).
fn sbr_frame(size: usize) -> Vec<u8> {
    // 110 0010 1101 0000 00000000 111 000000
    let tail = [0b1100_0101, 0b1010_0000, 0b0000_0001, 0b1100_0000];
    cat(&[vec![0xaa; size - tail.len()], tail.to_vec()])
}

/// A one-track AAC movie of `frames`, all of one size, with the
/// `AudioSpecificConfig` `config` at 22,050 Hz, and `edit` as in
/// [`aac_movie`].
fn aac_22k_movie(config: &[u8], frames: &[Vec<u8>], edit: Option<(u32, i32, u32)>) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let mdat = bx(b"mdat", &frames.concat());
    let n = frames.len() as u32;
    let moov = audio_moov(
        (config, 22_050),
        n,
        frames[0].len() as u32,
        edit,
        false,
        n,
        &[(ftyp.len() + 8) as u32],
    );
    cat(&[ftyp, mdat, moov])
}

#[test]
fn he_aac_is_timed_at_its_decoded_rate() {
    // LC at 22.05 kHz, SBR only in the frames (implicit signalling), and
    // SBR at 44.1 kHz in the configuration (object type 5, explicit).
    let implicit = [0x13, 0x90];
    let explicit = [0x2b, 0x92, 0x08];
    let sbr = sbr_frame(10);
    let plain = vec![0u8; 10];
    for (config, frame, rate) in [
        (implicit.as_slice(), &sbr, 44_100),
        (&explicit, &plain, 44_100),
        (&implicit, &plain, 22_050),
    ] {
        let source = aac_22k_movie(config, &vec![frame.clone(); 100], Some((0, 2048, 4000)));
        let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
        assert_eq!((summary.timescale, summary.duration), (22_050, 102_400));
        let movie = probe(
            SliceSource::new(&output),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        let track = &movie.tracks[0];
        let scale = rate / 22_050;
        assert_eq!(track.timescale, rate, "{config:x?}");
        assert_eq!(track.description.sample_rate, Some(rate), "{config:x?}");
        assert_eq!(
            track.description.codec_config,
            probe(
                SliceSource::new(&source),
                &Limits::default(),
                &Allowlist::default()
            )
            .unwrap()
            .tracks[0]
                .description
                .codec_config
        );
        assert_eq!(track.mdhd_duration, 102_400 * u64::from(scale));
        assert_eq!(
            track.edit_list,
            EditList::Simple {
                empty: 0,
                media_time: 2048 * i64::from(scale),
                segment: 4000
            }
        );
        let all = packets(&movie, track, &output);
        assert_eq!(all.len(), 100);
        assert!(all.iter().all(|p| p.duration == 1024 * scale));
        assert!(all.iter().all(|p| bytes_of(&output, p) == *frame));
    }
    // An `iTunSMPB` item that adds up in the source's ticks (2,112 +
    // 100,000 + 288 = 102,400) gives the same edit, in the output's ticks.
    let source = with_moov_children(
        &aac_22k_movie(&implicit, &vec![sbr.clone(); 100], None),
        &smpb_udta(" 00000000 00000840 00000120 00000000000186A0 00000000"),
    );
    let (_, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
    let movie = probe(
        SliceSource::new(&output),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    // A plain frame may look as if it ends with SBR data now and then, and
    // SBR that starts after the first frames is not looked for: neither
    // changes the timescale.
    let mut sporadic = vec![plain.clone(); 100];
    sporadic[0] = sbr.clone();
    sporadic[5] = sbr.clone();
    let mut late = vec![sbr.clone(); 100];
    late[..4].fill(plain.clone());
    for frames in [sporadic, late] {
        let (_, output) = extract(
            &aac_22k_movie(&implicit, &frames, Some((0, 2048, 4000))),
            TrackChoice::FirstAudio,
        )
        .unwrap();
        let movie = probe(
            SliceSource::new(&output),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        assert_eq!(movie.tracks[0].timescale, 22_050);
    }
    // 100,000 samples at 22.05 kHz in movie ticks of 1 ms, rounded: 4,535.
    assert_eq!(
        movie.tracks[0].edit_list,
        EditList::Simple {
            empty: 0,
            media_time: 2 * 2112,
            segment: 4535
        }
    );
}

#[test]
fn aac_track_round_trips() {
    let source = common::av_movie_with(common::aac_entry(), false);
    let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
    assert_eq!(summary.stream_index, 1);
    assert_eq!(summary.samples, 4);
    assert_eq!(summary.timescale, common::TIMESCALE);
    assert_eq!(summary.duration, 2048);
    assert_round_trip(&source, 1, &output, &summary);
    // The layout: `ftyp` (M4A brands), then `moov`, then `mdat`.
    assert_eq!(&output[4..8], b"ftyp");
    assert_eq!(&output[8..12], b"M4A ");
    assert_eq!(&output[16..28], b"M4A mp42isom");
    assert_eq!(&output[32..36], b"moov");
    let moov_len = u32::from_be_bytes(output[28..32].try_into().unwrap()) as usize;
    assert_eq!(&output[28 + moov_len + 4..28 + moov_len + 8], b"mdat");
    assert_eq!(output.len(), 28 + moov_len + 8 + 400);
    // The same track chosen by stream index gives the same bytes.
    let (_, again) = extract(&source, TrackChoice::Stream(1)).unwrap();
    assert_eq!(again, output);
}

#[test]
fn only_user_chosen_metadata_is_copied() {
    let source = common::av_movie_with(common::aac_entry(), false);
    let (_, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
    for kind in [b"udta", b"meta", b"avc1", b"vide"] {
        assert!(
            !output.windows(4).any(|w| w == kind),
            "{}",
            String::from_utf8_lossy(kind)
        );
    }
}

#[test]
fn tracks_that_cannot_be_extracted_write_nothing() {
    let source = common::av_movie_with(common::aac_entry(), false);
    let cases = [
        (
            TrackChoice::Stream(0),
            Error::Unsupported {
                reason: "the stream is not an audio track",
            },
        ),
        (TrackChoice::Stream(2), Error::NoSuchTrack),
    ];
    for (choice, expected) in cases {
        let mut out = Vec::new();
        let result = extract_audio(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            choice,
            &mut out,
        );
        assert_eq!(result, Err(ExtractError::Source(expected)), "{choice:?}");
        assert!(out.is_empty());
    }

    // A stream the allowlist drops cannot be extracted.
    let video_only = Allowlist::new(&[Codec::H264]);
    let mut out = Vec::new();
    let result = extract_audio(
        SliceSource::new(&source),
        &Limits::default(),
        &video_only,
        TrackChoice::Stream(1),
        &mut out,
    );
    assert!(matches!(
        result,
        Err(ExtractError::Source(Error::CodecNotAllowed {
            track_id: 2,
            ..
        }))
    ));
    let result = extract_audio(
        SliceSource::new(&source),
        &Limits::default(),
        &video_only,
        TrackChoice::FirstAudio,
        &mut out,
    );
    // The allowlist dropped the only audio track: its codec is not allowed.
    assert!(matches!(
        result,
        Err(ExtractError::Source(Error::CodecNotAllowed {
            track_id: 2,
            ..
        }))
    ));
    assert!(out.is_empty());

    // Audio other than AAC is refused.
    for kind in [b"sowt", b"Opus"] {
        let source = common::av_movie(kind);
        let result = extract(&source, TrackChoice::FirstAudio);
        assert_eq!(
            result.map(|_| ()),
            Err(ExtractError::Source(Error::Unsupported {
                reason: "only AAC audio can be extracted"
            })),
            "{}",
            String::from_utf8_lossy(kind)
        );
    }
}

#[test]
fn table_limit_applies_to_the_output_tables() {
    // A constant sample size costs the parser no table bytes; the output
    // charges every size (4 bytes each, kept once two sizes differ), one run
    // (8) and three chunks (20 each for their `stsc` and `co64` entries).
    let source = aac_movie(100, 10, None, false);
    for (limit, ok) in [(468, true), (467, false)] {
        let limits = Limits {
            max_table_bytes: limit,
            ..Limits::default()
        };
        assert!(probe(SliceSource::new(&source), &limits, &Allowlist::default()).is_ok());
        let mut out = Vec::new();
        let result = extract_audio(
            SliceSource::new(&source),
            &limits,
            &Allowlist::default(),
            TrackChoice::FirstAudio,
            &mut out,
        );
        if ok {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(matches!(
                result,
                Err(ExtractError::Source(Error::LimitExceeded { .. }))
            ));
            assert!(out.is_empty());
        }
    }
}

#[test]
fn edit_lists_are_copied() {
    for edit in [
        None,
        Some((0, 1024, 2000)),
        Some((250, 0, 2133)),
        Some((7, 2048, 1)),
    ] {
        let source = aac_movie(100, 10, edit, false);
        let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
        assert_round_trip(&source, 0, &output, &summary);
        let movie = probe(
            SliceSource::new(&output),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        assert_eq!(movie.timescale, 1000);
        assert_eq!(movie.tracks[0].language, Some(*b"fra"));
        assert_eq!(movie.creation_time, 0x7c25_b080);
        // The movie lasts as long as the edits add up to, or as the media
        // without them (14496-12 8.6.6).
        assert_eq!(
            movie.mvhd_duration,
            match edit {
                Some((empty, _, segment)) => u64::from(empty) + u64::from(segment),
                None => 100u64 * 1024 * 1000 / 48_000,
            },
            "{edit:?}"
        );
        let expected = edit;
        match movie.tracks[0].edit_list {
            EditList::None => assert_eq!(expected, None),
            EditList::Simple {
                empty,
                media_time,
                segment,
            } => assert_eq!(
                Some((empty as u32, media_time as i32, segment as u32)),
                expected
            ),
            EditList::Multiple | EditList::Unsupported => panic!("not the simple model"),
        }
    }
}

/// [`aac_movie`] whose track carries `entries` as its edit list, each
/// `(segment duration in movie ticks, media time, rate)`. `moov` comes
/// last, so resizing it moves no sample.
fn aac_movie_with_edits(n: u32, size: u32, entries: &[(u64, i64, i32)]) -> Vec<u8> {
    aac_movie_with_edit_list(n, size, entries, false)
}

/// [`aac_movie_with_edits`], in version 1 entries (64-bit durations and
/// media times) when `wide`.
fn aac_movie_with_edit_list(n: u32, size: u32, entries: &[(u64, i64, i32)], wide: bool) -> Vec<u8> {
    let movie = aac_movie(n, size, None, false);
    let mut elst = u32s(&[entries.len() as u32]);
    for (duration, media_time, rate) in entries {
        if wide {
            elst.extend_from_slice(&duration.to_be_bytes());
            elst.extend_from_slice(&media_time.to_be_bytes());
        } else {
            elst.extend_from_slice(&u32s(&[*duration as u32, *media_time as i32 as u32]));
        }
        elst.extend_from_slice(&u32s(&[*rate as u32]));
    }
    let edts = bx(b"edts", &full(b"elst", u8::from(wide), 0, &elst));
    // Insert `edts` after `tkhd`, where the parser expects it, and grow the
    // `trak` and `moov` sizes by its length.
    let moov_at = movie.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let trak_at = movie.windows(4).position(|w| w == b"trak").unwrap() - 4;
    let mdia_at = movie.windows(4).position(|w| w == b"mdia").unwrap() - 4;
    let grow = |at: usize, out: &mut Vec<u8>| {
        let old = u32::from_be_bytes(out[at..at + 4].try_into().unwrap());
        let new = old + edts.len() as u32;
        out[at..at + 4].copy_from_slice(&new.to_be_bytes());
    };
    let mut out = movie[..mdia_at].to_vec();
    out.extend_from_slice(&edts);
    out.extend_from_slice(&movie[mdia_at..]);
    grow(trak_at, &mut out);
    grow(moov_at, &mut out);
    out
}

/// Several media edits are copied entry by entry
/// (`docs/design/edit-list-writers.md`, "M4A extraction", decision 1).
#[test]
fn several_edits_are_copied() {
    // 100 samples of 1,024 ticks at 48 kHz, movie timescale 1,000: two media
    // edits that skip 10,240 ticks of media, with an empty edit between
    // them. An empty edit presents nothing at any rate, so its rate of 0 is
    // inside the model and is copied as stored.
    let entries = [
        (1_000u64, 0i64, 0x10000i32),
        (500, -1, 0),
        (1_000, 58_368, 0x10000),
    ];
    for wide in [false, true] {
        let source = aac_movie_with_edit_list(100, 10, &entries, wide);
        let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
        assert_round_trip(&source, 0, &output, &summary);
        let movie = probe(
            SliceSource::new(&output),
            &Limits::default(),
            &Allowlist::default(),
        )
        .unwrap();
        assert_eq!(movie.timescale, 1000);
        assert_eq!(movie.tracks[0].edit_list, EditList::Multiple, "wide {wide}");
        let copied: Vec<(u64, i64, i32)> = movie.tracks[0]
            .edits
            .iter()
            .map(|e| (e.segment_duration, e.media_time, e.media_rate))
            .collect();
        assert_eq!(copied, entries, "wide {wide}");
        // The movie lasts as long as the entries add up to (14496-12 8.6.6).
        assert_eq!(movie.mvhd_duration, 2_500, "wide {wide}");
    }
}

/// The refusals of `docs/design/edit-list-writers.md`, "M4A extraction",
/// decision 2: each writes nothing.
#[test]
fn several_edits_outside_the_model_are_refused() {
    // Windows that go backwards, a media time below -1 and a media edit of
    // duration 0 are all outside the model probe applies
    // (docs/design/edit-lists.md, decision 1).
    let unit = 0x10000i32;
    let cases = [
        vec![(1_000u64, 48_000i64, unit), (1_000, 0, unit)],
        vec![(1_000, 0, unit), (1_000, -1024, unit)],
        vec![(1_000, 0, unit), (0, 58_368, unit), (1_000, 96_000, unit)],
        vec![(1_000, 0, unit), (1_000, 58_368, 0x20000)],
    ];
    for entries in &cases {
        let source = aac_movie_with_edits(100, 10, entries);
        let mut out = Vec::new();
        let result = extract_audio(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            TrackChoice::FirstAudio,
            &mut out,
        );
        assert_eq!(
            result,
            Err(ExtractError::Source(Error::Unsupported {
                reason: "edit list outside the supported model"
            })),
            "{entries:?}"
        );
        assert!(out.is_empty());
    }
    // The same shape in order is extracted, so the refusals above are the
    // list's doing and not the writer's.
    let ordered = aac_movie_with_edits(
        100,
        10,
        &[(1_000, 0, unit), (500, -1, unit), (1_000, 58_368, unit)],
    );
    assert!(extract(&ordered, TrackChoice::FirstAudio).is_ok());
}

#[test]
fn quicktime_sound_entries_become_version_0() {
    let source = aac_movie(10, 10, None, true);
    let (summary, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
    assert_round_trip(&source, 0, &output, &summary);
    let at = output.windows(4).position(|w| w == b"mp4a").unwrap();
    // Version field of the sound sample entry (14496-12 12.2.3).
    assert_eq!(&output[at + 12..at + 14], &[0, 0]);
}

#[test]
fn chunks_hold_about_one_second() {
    // 100 samples of 1024 ticks at 48 kHz: chunks close after 47 samples.
    let source = aac_movie(100, 10, None, false);
    let (_, output) = extract(&source, TrackChoice::FirstAudio).unwrap();
    let at = output.windows(4).position(|w| w == b"stsc").unwrap();
    let entries: Vec<u32> = output[at + 8..at + 8 + 4 + 24]
        .chunks(4)
        .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
        .collect();
    assert_eq!(entries, [2, 1, 47, 1, 3, 6, 1]);
    let at = output.windows(4).position(|w| w == b"stco").unwrap();
    assert_eq!(&output[at + 8..at + 12], &3u32.to_be_bytes());
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
    let source = common::av_movie_with(common::aac_entry(), false);
    for left in [0, 30, 500] {
        let result = extract_audio(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
            TrackChoice::FirstAudio,
            FailingWriter { left },
        );
        assert_eq!(result, Err(ExtractError::Write(io::ErrorKind::BrokenPipe)));
    }
}

/// A source whose sample bytes disappear after `reads` reads.
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
fn read_errors_while_copying_are_reported() {
    let source = aac_movie(20, 100_000, None, false);
    // Count the reads of a successful run, then fail on each one in turn.
    let mut total = 0;
    while extract_audio(
        Shrinking {
            data: &source,
            reads: total,
        },
        &Limits::default(),
        &Allowlist::default(),
        TrackChoice::FirstAudio,
        io::sink(),
    )
    .is_err()
    {
        total += 1;
        assert!(total < 10_000);
    }
    for reads in 0..total {
        let result = extract_audio(
            Shrinking {
                data: &source,
                reads,
            },
            &Limits::default(),
            &Allowlist::default(),
            TrackChoice::FirstAudio,
            io::sink(),
        );
        assert_eq!(
            result,
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
    let source = common::av_movie_with(common::aac_entry(), false);
    for len in 0..source.len() {
        let mut out = Vec::new();
        if extract_audio(
            SliceSource::new(&source[..len]),
            &Limits::default(),
            &Allowlist::default(),
            TrackChoice::FirstAudio,
            &mut out,
        )
        .is_ok()
        {
            probe(
                SliceSource::new(&out),
                &Limits::default(),
                &Allowlist::default(),
            )
            .unwrap();
        }
    }
}

/// The fuzz property on deterministic mutations: every
/// input fails cleanly or gives an output the parser accepts, with the
/// reported samples.
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
        let Ok(summary) = extract_audio(
            SliceSource::new(&input),
            &limits,
            &Allowlist::default(),
            TrackChoice::FirstAudio,
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
        let count = packets(&movie, &movie.tracks[0], &out).len();
        assert_eq!(count, summary.samples as usize);
    }
}

#[test]
fn mutated_inputs_are_safe() {
    assert_mutations_are_safe(
        &aac_movie(30, 10, Some((5, 1024, 500)), false),
        3000,
        0x2545_f491,
    );
    assert_mutations_are_safe(
        &common::av_movie_with(common::aac_entry(), false),
        3000,
        0x9e37_79b9,
    );
}

/// A JSON string literal (RFC 8259 section 7).
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

/// The round trip on the generated fixtures (`COREAV_FIXTURES`): every
/// AAC track of every MP4, MOV and M4A fixture is extracted and round-trips.
/// With `COREAV_EXTRACT_OUT`, the outputs and a `manifest.json` listing them
/// are written there for the black-box oracles (`scripts/extract_check.py`).
#[test]
#[ignore = "needs COREAV_FIXTURES"]
fn fixtures_round_trip() {
    let dir = std::env::var("COREAV_FIXTURES").expect("COREAV_FIXTURES");
    let out_dir = std::env::var("COREAV_EXTRACT_OUT").ok();
    if let Some(out_dir) = &out_dir {
        std::fs::create_dir_all(out_dir).unwrap();
    }
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
    for path in &entries {
        let source = std::fs::read(path).unwrap();
        let Ok(movie) = probe(
            SliceSource::new(&source),
            &Limits::default(),
            &Allowlist::default(),
        ) else {
            continue;
        };
        for track in movie
            .tracks
            .iter()
            .filter(|t| t.description.codec == Codec::Aac)
        {
            let name = path.file_name().unwrap().to_string_lossy();
            let (summary, output) = extract(&source, TrackChoice::Stream(track.stream_index))
                .unwrap_or_else(|e| panic!("{name} stream {}: {e}", track.stream_index));
            assert_round_trip(&source, track.stream_index, &output, &summary);
            if let Some(out_dir) = &out_dir {
                let stem = path.file_stem().unwrap().to_string_lossy();
                let output_path =
                    Path::new(out_dir).join(format!("{stem}.{}.m4a", track.stream_index));
                std::fs::write(&output_path, &output).unwrap();
                manifest.push(format!(
                    "{{\"source\": {}, \"stream_index\": {}, \"output\": {}}}",
                    json_string(&path.display().to_string()),
                    track.stream_index,
                    json_string(&output_path.display().to_string())
                ));
            }
            eprintln!(
                "{name} stream {}: {} samples",
                track.stream_index, summary.samples
            );
        }
    }
    assert!(!manifest.is_empty() || out_dir.is_none(), "no AAC fixtures");
    if let Some(out_dir) = &out_dir {
        std::fs::write(
            Path::new(out_dir).join("manifest.json"),
            format!("[\n{}\n]\n", manifest.join(",\n")),
        )
        .unwrap();
    }
}

/// A source that counts the reads that reach it.
struct Counting<'a> {
    data: &'a [u8],
    reads: u32,
}

impl Source for Counting<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.data.len() as u64)
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.reads += 1;
        SliceSource::new(self.data).read_exact_at(offset, buf)
    }
}

#[test]
fn samples_are_copied_through_a_read_window() {
    // 1,000 samples of 30 bytes (30 KB, inside one 64 KiB window): without
    // the window an unbuffered source sees one read per sample.
    let source = aac_movie(1000, 30, None, false);
    let mut counting = Counting {
        data: &source,
        reads: 0,
    };
    let mut out = Vec::new();
    let summary = extract_audio(
        &mut counting,
        &Limits::default(),
        &Allowlist::default(),
        TrackChoice::FirstAudio,
        &mut out,
    )
    .unwrap();
    assert!(counting.reads < 50, "{} reads", counting.reads);
    assert_round_trip(&source, 0, &out, &summary);
    // Samples larger than the window are copied through it in pieces.
    let source = aac_movie(3, 70_000, None, false);
    let (summary, out) = extract(&source, TrackChoice::FirstAudio).unwrap();
    assert_round_trip(&source, 0, &out, &summary);
}

/// A source whose reads that lie wholly inside `[from, to)` fail, as a
/// failing disk or network would.
struct FailsInside<'a> {
    data: &'a [u8],
    from: u64,
    to: u64,
}

impl Source for FailsInside<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.data.len() as u64)
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        if offset >= self.from && offset + buf.len() as u64 <= self.to {
            return Err(Error::Io(io::ErrorKind::Other));
        }
        SliceSource::new(self.data).read_exact_at(offset, buf)
    }
}

#[test]
fn a_failed_read_of_the_gapless_item_is_an_error() {
    let source = with_moov_children(
        &aac_movie(100, 10, None, false),
        &smpb_udta(" 00000000 00000840 00000120 00000000000186A0 00000000"),
    );
    let udta = source.windows(4).position(|w| w == b"udta").unwrap() as u64 - 4;
    let size = u64::from(u32::from_be_bytes(
        source[udta as usize..udta as usize + 4].try_into().unwrap(),
    ));
    // Probe reads the whole movie box at once; only the item's own reads,
    // inside the `udta` payload, fail.
    let failing = || FailsInside {
        data: &source,
        from: udta + 8,
        to: udta + size,
    };
    let movie = probe(failing(), &Limits::default(), &Allowlist::default()).unwrap();
    assert_eq!(
        movie.itunes_gapless(failing()),
        Err(Error::Io(io::ErrorKind::Other))
    );
    assert!(
        movie
            .itunes_gapless(SliceSource::new(&source))
            .unwrap()
            .is_some()
    );
    // Before, the failure read as "no item" and the output silently lost
    // its edit list; now the extraction fails.
    let mut out = Vec::new();
    let result = extract_audio(
        failing(),
        &Limits::default(),
        &Allowlist::default(),
        TrackChoice::FirstAudio,
        &mut out,
    );
    assert_eq!(
        result,
        Err(ExtractError::Source(Error::Io(io::ErrorKind::Other)))
    );
    // A malformed item is still no item, never an error.
    let malformed = with_moov_children(
        &aac_movie(100, 10, None, false),
        &smpb_udta(" 00000000 zz 00000120 00000000000186A0 00000000"),
    );
    let movie = probe(
        SliceSource::new(&malformed),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(movie.itunes_gapless(SliceSource::new(&malformed)), Ok(None));
}

/// A source that records the advice it gets.
struct Advised<'a> {
    data: SliceSource<'a>,
    advice: Vec<(u64, u64)>,
}

impl coreav_isobmff::Source for Advised<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.data.read_exact_at(offset, buf)
    }

    fn will_read(&mut self, offset: u64, len: u64) {
        self.advice.push((offset, len));
    }
}

#[test]
fn the_copy_pass_advises_where_it_starts_and_how_much_it_copies() {
    let source = common::av_movie_with(common::aac_entry(), false);
    let mut advised = Advised {
        data: SliceSource::new(&source),
        advice: Vec::new(),
    };
    let summary = extract_audio(
        &mut advised,
        &Limits::default(),
        &Allowlist::default(),
        TrackChoice::FirstAudio,
        io::sink(),
    )
    .unwrap();
    let movie = probe(
        SliceSource::new(&source),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let track = &movie.tracks[summary.stream_index];
    let all = packets(&movie, track, &source);
    let end = all
        .iter()
        .map(|p| p.offset + u64::from(p.size))
        .max()
        .unwrap();
    assert_eq!(advised.advice, vec![(all[0].offset, end - all[0].offset)]);
}

/// What a remote source that can skip bytes sees: its reads and its
/// advice, in order.
#[derive(Debug, PartialEq)]
enum Seen {
    Read(u64, u64),
    Regions(Vec<(u64, u64)>),
    Extent(u64, u64),
}

struct Skipping<'a> {
    data: SliceSource<'a>,
    seen: Vec<Seen>,
}

impl coreav_isobmff::Source for Skipping<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.seen.push(Seen::Read(offset, buf.len() as u64));
        self.data.read_exact_at(offset, buf)
    }

    fn will_read(&mut self, offset: u64, len: u64) {
        self.seen.push(Seen::Extent(offset, len));
    }

    fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        self.seen.push(Seen::Regions(regions.to_vec()));
    }

    fn wants_regions(&mut self) -> bool {
        true
    }
}

/// An AAC track of `chunks` chunks of `per_chunk` samples of `size` bytes,
/// each chunk followed by `gap` bytes of other data, the movie box last.
fn interleaved_aac(chunks: u32, per_chunk: u32, size: u32, gap: u32) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    let base = (ftyp.len() + 8) as u32;
    for c in 0..chunks {
        offsets.push(base + data.len() as u32);
        data.extend((0..per_chunk * size).map(|i| (i % 251) as u8 ^ c as u8));
        data.extend(std::iter::repeat_n(0xEE, gap as usize));
    }
    let moov = aac_moov_chunks(chunks * per_chunk, size, None, false, per_chunk, &offsets);
    cat(&[ftyp, bx(b"mdat", &data), moov])
}

#[test]
fn a_source_that_skips_bytes_is_told_exactly_what_the_copy_pass_reads() {
    // (chunks, samples per chunk, sample size, gap): gaps larger and
    // smaller than the 64 KiB window, none, and samples larger than it.
    for (chunks, per_chunk, size, gap) in [
        (12, 40, 300, 200_000),
        (12, 40, 300, 30_000),
        (6, 10, 500, 0),
        (5, 2, 70_000, 100_000),
        (40, 3, 700, 1_000_000),
    ] {
        let file = interleaved_aac(chunks, per_chunk, size, gap);
        let mut expected = Vec::new();
        extract_audio(
            SliceSource::new(&file),
            &Limits::default(),
            &Allowlist::default(),
            TrackChoice::FirstAudio,
            &mut expected,
        )
        .unwrap();
        for buffered in [false, true] {
            let mut skipping = Skipping {
                data: SliceSource::new(&file),
                seen: Vec::new(),
            };
            let mut out = Vec::new();
            if buffered {
                extract_audio(
                    coreav_isobmff::BufferedSource::new(&mut skipping),
                    &Limits::default(),
                    &Allowlist::default(),
                    TrackChoice::FirstAudio,
                    &mut out,
                )
            } else {
                extract_audio(
                    &mut skipping,
                    &Limits::default(),
                    &Allowlist::default(),
                    TrackChoice::FirstAudio,
                    &mut out,
                )
            }
            .unwrap();
            assert_eq!(out, expected, "{chunks} x {per_chunk} x {size}, gap {gap}");
            // Region advice, the pass's reads, then the end of the advice;
            // never the extent.
            let start = skipping
                .seen
                .iter()
                .position(|s| matches!(s, Seen::Regions(r) if !r.is_empty()))
                .unwrap();
            let Seen::Regions(regions) = &skipping.seen[start] else {
                unreachable!()
            };
            assert_eq!(skipping.seen.last(), Some(&Seen::Regions(vec![])));
            assert!(!skipping.seen.iter().any(|s| matches!(s, Seen::Extent(..))));
            let reads: Vec<(u64, u64)> = skipping.seen[start + 1..skipping.seen.len() - 1]
                .iter()
                .map(|s| match s {
                    Seen::Read(offset, len) => (*offset, *len),
                    other => panic!("{other:?} during the pass"),
                })
                .collect();
            // The reads cover exactly the regions: each read inside one,
            // every region byte read.
            let mut union: Vec<(u64, u64)> = Vec::new();
            for &(offset, len) in &reads {
                assert!(
                    regions
                        .iter()
                        .any(|&(o, l)| offset >= o && offset + len <= o + l),
                    "read {offset}+{len} outside {regions:?}"
                );
                match union.last_mut() {
                    Some(last) if offset <= last.0 + last.1 => {
                        last.1 = (offset + len).max(last.0 + last.1) - last.0;
                    }
                    _ => union.push((offset, len)),
                }
            }
            assert_eq!(&union, regions, "gap {gap}, buffered {buffered}");
            // Regions in increasing order, apart from each other.
            assert!(regions.windows(2).all(|w| w[0].0 + w[0].1 < w[1].0));
            let advised: u64 = regions.iter().map(|r| r.1).sum();
            if gap >= 100_000 {
                assert_eq!(regions.len() as u32, chunks, "one region per chunk");
                // The samples, and at most one window past each chunk.
                let audio = u64::from(chunks * per_chunk * size);
                assert!(advised <= audio + u64::from(chunks) * (64 << 10));
            }
            if gap == 0 {
                assert_eq!(regions.len(), 1);
            }
        }
    }
}
