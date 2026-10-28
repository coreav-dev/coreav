//! MP4 edit lists in `decode`: the presentation timeline, sample-exact,
//! with empty edits as silence (`docs/design/audio-decode.md`, "Edit
//! lists"), on MP4 FLAC files built here from constant frames, frame k of
//! value 100 (k + 1).

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_audio::{
    Error, LimitKind, Limits, Options, OutputFormat, SampleFormat, SilenceSpan, Sink, Summary,
    decode,
};
use coreav_isobmff::SliceSource;

const RATE: u32 = 44_100;
const BLOCK: u16 = 1000;

#[derive(Default)]
struct Keep(Vec<u8>);

impl Sink for Keep {
    fn start(&mut self, _: &OutputFormat) -> Result<(), Error> {
        Ok(())
    }
    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(samples);
        Ok(())
    }
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A mono 16-bit 44.1 kHz FLAC frame of `BLOCK` samples of `value`.
fn frame(number: u8, value: i16) -> Vec<u8> {
    let mut frame = vec![0xFF, 0xF8, 0x79, 0x08, number];
    frame.extend_from_slice(&(BLOCK - 1).to_be_bytes());
    frame.push(crc8(&frame));
    // A constant subframe: header byte 0, then the value.
    frame.push(0);
    frame.extend_from_slice(&value.to_be_bytes());
    let crc = crc16(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame
}

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// An MP4 file of `count` frames with the edit list `edits` (duration in
/// movie ticks, media time in media ticks, -1 for an empty edit) at movie
/// timescale `movie_timescale`; no edit list for `None`.
fn mp4(count: usize, edits: Option<&[(u64, i64)]>, movie_timescale: u32) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = (0..count)
        .map(|k| frame(k as u8, (k as i16 + 1) * 100))
        .collect();
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
    let data: Vec<u8> = frames.concat();
    let mdat = bx(b"mdat", &data);
    let offset = (ftyp.len() + 8) as u32;
    let media = (count * usize::from(BLOCK)) as u32;
    let matrix: Vec<u8> = [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let mut mvhd = vec![0u8; 12];
    mvhd.extend_from_slice(&movie_timescale.to_be_bytes());
    mvhd.extend_from_slice(&0u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd.extend_from_slice(&[0; 10]);
    mvhd.extend_from_slice(&matrix);
    mvhd.extend_from_slice(&[0; 24]);
    mvhd.extend_from_slice(&2u32.to_be_bytes());
    let mut tkhd = vec![0, 0, 0, 3];
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&1u32.to_be_bytes());
    tkhd.extend_from_slice(&[0; 4]);
    tkhd.extend_from_slice(&0u32.to_be_bytes());
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&[0, 0, 0, 0, 0x01, 0x00, 0, 0]);
    tkhd.extend_from_slice(&matrix);
    tkhd.extend_from_slice(&[0; 8]);
    let mut trak = bx(b"tkhd", &tkhd);
    if let Some(edits) = edits {
        let wide = edits.iter().any(|&(d, _)| d > u64::from(u32::MAX));
        let mut elst = vec![u8::from(wide), 0, 0, 0];
        elst.extend_from_slice(&(edits.len() as u32).to_be_bytes());
        for &(duration, media_time) in edits {
            if wide {
                elst.extend_from_slice(&duration.to_be_bytes());
                elst.extend_from_slice(&media_time.to_be_bytes());
            } else {
                elst.extend_from_slice(&(duration as u32).to_be_bytes());
                elst.extend_from_slice(&(media_time as i32).to_be_bytes());
            }
            elst.extend_from_slice(&[0, 1, 0, 0]);
        }
        trak.extend(bx(b"edts", &bx(b"elst", &elst)));
    }
    let mut mdhd = vec![0u8; 12];
    mdhd.extend_from_slice(&RATE.to_be_bytes());
    mdhd.extend_from_slice(&media.to_be_bytes());
    mdhd.extend_from_slice(&[0x55, 0xC4, 0, 0]);
    let mut hdlr = vec![0u8; 8];
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0; 13]);
    // fLaC sample entry with its dfLa box (the streaminfo block, last).
    let mut streaminfo = Vec::new();
    streaminfo.extend_from_slice(&BLOCK.to_be_bytes());
    streaminfo.extend_from_slice(&BLOCK.to_be_bytes());
    streaminfo.extend_from_slice(&[0; 6]);
    let packed: u64 = (u64::from(RATE) << 44) | (15 << 36) | media as u64;
    streaminfo.extend_from_slice(&packed.to_be_bytes());
    streaminfo.extend_from_slice(&[0; 16]);
    let mut dfla = vec![0, 0, 0, 0, 0x80, 0, 0, 34];
    dfla.extend_from_slice(&streaminfo);
    let mut entry = vec![0u8; 6];
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&[0; 8]);
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes());
    entry.extend_from_slice(&[0; 4]);
    entry.extend_from_slice(&(RATE << 16).to_be_bytes());
    entry.extend(bx(b"dfLa", &dfla));
    let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stsd.extend(bx(b"fLaC", &entry));
    let mut stts = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stts.extend_from_slice(&(count as u32).to_be_bytes());
    stts.extend_from_slice(&u32::from(BLOCK).to_be_bytes());
    let mut stsc = vec![0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1];
    stsc.extend_from_slice(&(count as u32).to_be_bytes());
    stsc.extend_from_slice(&1u32.to_be_bytes());
    let mut stsz = vec![0u8; 8];
    stsz.extend_from_slice(&(count as u32).to_be_bytes());
    for f in &frames {
        stsz.extend_from_slice(&(f.len() as u32).to_be_bytes());
    }
    let mut stco = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stco.extend_from_slice(&offset.to_be_bytes());
    let stbl = [
        bx(b"stsd", &stsd),
        bx(b"stts", &stts),
        bx(b"stsc", &stsc),
        bx(b"stsz", &stsz),
        bx(b"stco", &stco),
    ]
    .concat();
    let dinf = bx(
        b"dinf",
        &bx(
            b"dref",
            &[&[0u8, 0, 0, 0, 0, 0, 0, 1][..], &bx(b"url ", &[0, 0, 0, 1])].concat(),
        ),
    );
    let minf = [bx(b"smhd", &[0; 8]), dinf, bx(b"stbl", &stbl)].concat();
    let mdia = [bx(b"mdhd", &mdhd), bx(b"hdlr", &hdlr), bx(b"minf", &minf)].concat();
    trak.extend(bx(b"mdia", &mdia));
    let moov = [bx(b"mvhd", &mvhd), bx(b"trak", &trak)].concat();
    [ftyp, mdat, bx(b"moov", &moov)].concat()
}

fn run(file: &[u8], threads: usize, limits: &Limits) -> Result<(Summary, Vec<i16>), Error> {
    let mut sink = Keep::default();
    let options = Options {
        threads,
        format: SampleFormat::S16,
        ..Options::default()
    };
    let summary = decode(SliceSource::new(file), &options, limits, &mut sink)?;
    let samples = sink
        .0
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok((summary, samples))
}

/// The media's samples `from..to`: frame k is 100 (k + 1).
fn media(from: usize, to: usize) -> Vec<i16> {
    (from..to)
        .map(|n| (n / usize::from(BLOCK) + 1) as i16 * 100)
        .collect()
}

#[test]
fn without_an_edit_list_every_sample_is_written() {
    let (summary, samples) = run(&mp4(10, None, 1000), 1, &Limits::default()).unwrap();
    assert!(samples == media(0, 10_000));
    assert_eq!(summary.timeline.start, 0);
    assert!(summary.timeline.silence.is_empty());
}

#[test]
fn a_leading_empty_edit_is_silence() {
    // 100 ms of empty edit at a movie timescale of 1000: 4410 samples.
    let (summary, samples) = run(
        &mp4(10, Some(&[(100, -1), (227, 0)]), 1000),
        1,
        &Limits::default(),
    )
    .unwrap();
    let mut want = vec![0i16; 4410];
    want.extend(media(0, 10_000));
    assert!(
        samples == want,
        "{} samples, {} wanted",
        samples.len(),
        want.len()
    );
    assert_eq!(summary.timeline.start, 4410);
    assert_eq!(
        summary.timeline.silence,
        [SilenceSpan {
            at: 0,
            samples: 4410
        }]
    );
}

/// Every write is whole sample frames, as `Sink` promises, silence
/// included: 4 KiB is not a multiple of a 3-byte frame.
#[test]
fn silence_is_written_in_whole_frames() {
    #[derive(Default)]
    struct Frames {
        frame: usize,
        bytes: usize,
    }
    impl Sink for Frames {
        fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
            self.frame = usize::from(format.channels) * format.format.bytes();
            Ok(())
        }
        fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
            assert_eq!(samples.len() % self.frame, 0, "{} bytes", samples.len());
            self.bytes += samples.len();
            Ok(())
        }
    }
    let file = mp4(10, Some(&[(100, -1), (227, 0)]), 1000);
    for (format, channels) in [(SampleFormat::S24, None), (SampleFormat::S24, Some(2))] {
        let mut sink = Frames::default();
        let options = Options {
            format,
            channels,
            ..Options::default()
        };
        let summary = decode(
            SliceSource::new(&file),
            &options,
            &Limits::default(),
            &mut sink,
        )
        .unwrap();
        assert_eq!(summary.samples, 14_410);
        assert_eq!(sink.bytes, 14_410 * sink.frame);
    }
}

#[test]
fn a_media_edit_is_cut_sample_exact_at_both_ends() {
    // Media 1500..4500 at a movie timescale equal to the sample rate.
    let (summary, samples) =
        run(&mp4(10, Some(&[(3000, 1500)]), RATE), 1, &Limits::default()).unwrap();
    assert!(samples == media(1500, 4500), "{}", samples.len());
    assert_eq!(summary.samples, 3000);
}

#[test]
fn cuts_and_inner_empty_edits_follow_the_presentation_timeline() {
    let edits = [(1000, 0), (500, -1), (1000, 5000), (700, 8150)];
    let file = mp4(10, Some(&edits), RATE);
    let mut want = media(0, 1000);
    want.extend(vec![0i16; 500]);
    want.extend(media(5000, 6000));
    want.extend(media(8150, 8850));
    for threads in [1, 3] {
        let (summary, samples) = run(&file, threads, &Limits::default()).unwrap();
        assert!(
            samples == want,
            "{threads} threads: {} samples, {} wanted",
            samples.len(),
            want.len()
        );
        assert_eq!(summary.timeline.start, 0);
        assert_eq!(
            summary.timeline.silence,
            [SilenceSpan {
                at: 1000,
                samples: 500
            }]
        );
    }
}

/// Resampled, the timeline's positions are the output's: each the first
/// output sample at or after the stream's position.
#[test]
fn a_resampled_timeline_is_in_output_samples() {
    let edits = [(100, -1), (1000, 0), (500, -1), (1000, 5000)];
    let file = mp4(10, Some(&edits), RATE);
    let (native, _) = run(&file, 1, &Limits::default()).unwrap();
    for threads in [1, 3] {
        let mut sink = Keep::default();
        let options = Options {
            threads,
            sample_rate: Some(16_000),
            ..Options::default()
        };
        let summary = decode(
            SliceSource::new(&file),
            &options,
            &Limits::default(),
            &mut sink,
        )
        .unwrap();
        let at = |n: u64| (n * 16_000).div_ceil(u64::from(RATE));
        assert_eq!(summary.output.sample_rate, 16_000);
        assert_eq!(summary.samples, at(native.samples));
        assert_eq!(summary.timeline.start, at(native.timeline.start));
        let spans: Vec<_> = native
            .timeline
            .silence
            .iter()
            .map(|s| SilenceSpan {
                at: at(s.at),
                samples: at(s.at + s.samples) - at(s.at),
            })
            .collect();
        assert_eq!(summary.timeline.silence, spans);
        assert_eq!(sink.0.len() as u64, summary.samples * 2);
    }
}

#[test]
fn boundaries_round_to_the_nearest_sample_without_drift() {
    // At a movie timescale of 1000, 10 ms is 441 samples, 1 ms 44.1: the
    // boundaries 0, 44.1, 88.2, ... round to 0, 44, 88, 132, 176, 221.
    let edits = [(1, -1), (1, 0), (1, -1), (1, 2000), (1, -1)];
    let (summary, samples) = run(&mp4(10, Some(&edits), 1000), 1, &Limits::default()).unwrap();
    let mut want = vec![0i16; 44];
    want.extend(media(0, 44));
    want.extend(vec![0i16; 44]);
    want.extend(media(2000, 2044));
    want.extend(vec![0i16; 45]);
    assert!(
        samples == want,
        "{} samples, {} wanted",
        samples.len(),
        want.len()
    );
    assert_eq!(
        summary.timeline.silence,
        [
            SilenceSpan { at: 0, samples: 44 },
            SilenceSpan {
                at: 88,
                samples: 44
            },
            SilenceSpan {
                at: 176,
                samples: 45
            }
        ]
    );
}

/// An empty edit that would be days of silence fails with the output
/// limit before anything of it is written; 2^64 - 1 ticks at a movie
/// timescale of 1 do not overflow the conversion.
#[test]
fn a_huge_empty_edit_is_the_output_limit() {
    for (edits, movie_timescale) in [
        (vec![(u64::MAX, -1), (1, 0)], 1u32),
        (vec![(1, 0), (u64::MAX - 7, -1)], 1),
        (vec![(2_392_423_000u64, -1), (1000, 0)], 1000),
    ] {
        let file = mp4(3, Some(&edits), movie_timescale);
        let limits = Limits {
            max_output_samples: 1 << 26,
            ..Limits::default()
        };
        let result = run(&file, 1, &limits);
        assert!(
            matches!(result, Err(Error::LimitExceeded(LimitKind::OutputSamples))),
            "{edits:?}: {:?}",
            result.map(|(s, _)| s.samples)
        );
    }
}

/// Where decoding stops is known before it starts (the last media edit's
/// end): the packets after it are not decoded, on any number of threads,
/// so damaged packets there change nothing.
#[test]
fn where_decoding_stops_does_not_depend_on_the_threads() {
    let mut file = mp4(200, Some(&[(1000, 0)]), RATE);
    // Frames are 13 bytes after `ftyp` and the `mdat` header: from the
    // third on, a wrong CRC-8.
    let start = bx(b"ftyp", b"isom\0\0\0\0isom").len() + 8;
    for k in 2..200 {
        file[start + 13 * k + 7] ^= 0xFF;
    }
    let limits = Limits {
        max_decode_errors: 10,
        ..Limits::default()
    };
    let (one, samples) = run(&file, 1, &limits).unwrap();
    assert!(samples == media(0, 1000));
    for threads in [2, 3, 8] {
        let (many, again) = run(&file, threads, &limits).unwrap();
        assert!(again == samples, "{threads} threads");
        assert_eq!(many.packets, one.packets, "{threads} threads");
        assert_eq!(many.warnings, one.warnings, "{threads} threads");
    }
    assert!(one.warnings.is_empty(), "{:?}", one.warnings);
}

/// A last media edit whose end saturates runs to the media's end: it does
/// not stop decoding at the end of an earlier edit.
#[test]
fn a_last_edit_past_u64_runs_to_the_end_of_the_media() {
    // At a movie timescale of 1: one second (44,100 samples) from 0, then
    // about 2^60 seconds from 50,000.
    let file = mp4(100, Some(&[(1, 0), (1 << 60, 50_000)]), 1);
    let mut want = media(0, 44_100);
    want.extend(media(50_000, 100_000));
    for threads in [1, 3] {
        let (_, samples) = run(&file, threads, &Limits::default()).unwrap();
        assert!(
            samples == want,
            "{threads} threads: {} samples, {} wanted",
            samples.len(),
            want.len()
        );
    }
}

/// A seed: its file name, its edit list, its movie timescale.
type Seed = (&'static str, Vec<(u64, i64)>, u32);

/// Writes the seeds of the `decode` fuzz target for edit lists
/// (`fuzz/regressions/decode`): huge empty edits, at a movie timescale of
/// 1 and near 2^64 ticks, cuts and inner empty edits.
#[test]
#[ignore = "writes the fuzz seeds into COREAV_SEEDS"]
fn write_fuzz_seeds() {
    let dir = std::path::PathBuf::from(std::env::var("COREAV_SEEDS").unwrap());
    let seeds: [Seed; 5] = [
        (
            "edit-empty-u64-max-timescale-1.mp4",
            vec![(u64::MAX, -1), (1, 0)],
            1,
        ),
        (
            "edit-trailing-empty-near-u64-max.mp4",
            vec![(1, 0), (u64::MAX - 7, -1)],
            1,
        ),
        (
            "edit-empty-40-minutes.mp4",
            vec![(2_392_423_000, -1), (1000, 0)],
            1000,
        ),
        (
            "edit-cut-inner-empty.mp4",
            vec![(1000, 0), (500, -1), (1000, 5000), (700, 8150)],
            RATE,
        ),
        (
            "edit-rounding.mp4",
            vec![(1, -1), (1, 0), (1, -1), (1, 2000), (1, -1)],
            1000,
        ),
    ];
    for (name, edits, movie_timescale) in seeds {
        std::fs::write(dir.join(name), mp4(10, Some(&edits), movie_timescale)).unwrap();
    }
}
