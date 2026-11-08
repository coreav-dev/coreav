//! Copied MP3 bitrate normalization and byte-identical unproved-clock fallbacks.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use coreav_isobmff::FourCc;
use coreav_isobmff::{Allowlist, Error, Limits, SliceSource, Source};
use coreav_isobmff_write::mux::{
    FileType, MovieHeader, Muxer, RawBox, Sample, TrackHeader, TrackMedia, audio_sample_entry,
};
use coreav_isobmff_write::{CopiedAudioBitRate, RemuxOptions, remux_mp4};

fn frame(version: u32, index: u32, rate_index: u32) -> Vec<u8> {
    let (rate, bitrate, factor) = if version == 3 {
        (
            [44_100u32, 48_000, 32_000][rate_index as usize],
            if index == 5 { 64_000 } else { 128_000 },
            144,
        )
    } else {
        (if version == 2 { 22_050 } else { 11_025 }, 64_000, 72)
    };
    let word = (0x7FFu32 << 21)
        | (version << 19)
        | (1 << 17)
        | (1 << 16)
        | (index << 12)
        | (rate_index << 10)
        | (3 << 6);
    let mut bytes = vec![0; (factor * bitrate / rate) as usize];
    bytes[..4].copy_from_slice(&word.to_be_bytes());
    bytes
}

fn source(frames: &[Vec<u8>], rate: u32, frame_samples: u32, object: u8) -> Vec<u8> {
    let mut esds = vec![0, 0, 0, 0, 3, 21, 0, 1, 0, 4, 13, object, 0x15];
    let rates: Vec<u8> = [1234u32, 220_000, 1000]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect();
    esds.extend_from_slice(&rates[1..]);
    esds.extend_from_slice(&[6, 1, 2]);
    let entry = audio_sample_entry(
        FourCc(*b"mp4a"),
        1,
        16,
        rate,
        &[
            RawBox {
                kind: FourCc(*b"esds"),
                payload: esds,
            },
            RawBox {
                kind: FourCc(*b"btrt"),
                payload: rates,
            },
        ],
    )
    .unwrap();
    let mut muxer = Muxer::new(
        FileType {
            major: FourCc(*b"isom"),
            minor: 0,
            compatible: vec![FourCc(*b"isom")],
        },
        MovieHeader {
            timescale: 1000,
            creation_time: 0,
            boxes: Vec::new(),
        },
        &Limits::default(),
    )
    .unwrap();
    let track = muxer
        .add_track(TrackHeader {
            media: TrackMedia::Audio { volume: 0x100 },
            timescale: rate,
            language: *b"und",
            creation_time: 0,
            media_creation_time: 0,
            flags: 3,
            layer: 0,
            alternate_group: 0,
            edits: Vec::new(),
            duration: None,
            media_duration: None,
            sample_entry: entry,
            sample_table_boxes: Vec::new(),
            boxes: Vec::new(),
        })
        .unwrap();
    for (index, bytes) in frames.iter().enumerate() {
        muxer
            .push_sample(
                track,
                Sample {
                    size: bytes.len() as u32,
                    duration: if index + 1 == frames.len() {
                        433
                    } else {
                        frame_samples
                    },
                    sync: true,
                    composition_offset: 0,
                },
            )
            .unwrap();
    }
    let mut output = Vec::new();
    muxer.write_head(&mut output).unwrap();
    for bytes in frames {
        output.extend_from_slice(bytes);
    }
    output
}

fn remux(input: &[u8], rate: Option<u32>, limits: &Limits) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    remux_mp4(
        SliceSource::new(input),
        limits,
        &Allowlist::default(),
        &RemuxOptions {
            copied_audio_bit_rate: rate.map(CopiedAudioBitRate::Requested),
            ..RemuxOptions::default()
        },
        &mut output,
    )
    .ok()?;
    Some(output)
}

fn expected(control: &[u8], average: u32, maximum: u32) -> Vec<u8> {
    let before: Vec<u8> = [1234u32, 220_000, 1000]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect();
    let after: Vec<u8> = [0u32, maximum, average]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect();
    let positions: Vec<_> = control
        .windows(11)
        .enumerate()
        .filter_map(|(at, bytes)| (bytes == &before[1..]).then_some(at))
        .collect();
    assert_eq!(positions.len(), 2);
    let mut output = control.to_vec();
    for at in positions {
        output[at..at + 11].copy_from_slice(&after[1..]);
    }
    output
}

#[test]
fn complete_mp3_frame_clocks_change_only_bitrate_fields() {
    for (version, rate, frame_samples, object) in [
        (3, 48_000, 1152, 0x6B),
        (2, 22_050, 576, 0x69),
        (0, 11_025, 576, 0x69),
    ] {
        let frames: Vec<_> = (0..12)
            .map(|index| {
                frame(
                    version,
                    if version == 3 {
                        if index % 2 == 0 { 5 } else { 9 }
                    } else {
                        8
                    },
                    u32::from(version == 3),
                )
            })
            .collect();
        let input = source(&frames, rate, frame_samples, object);
        let control = remux(&input, None, &Limits::default()).unwrap();
        let average = (frames.iter().map(Vec::len).sum::<usize>() as u64 * 8 * u64::from(rate)
            / (12 * u64::from(frame_samples))) as u32;
        for requested in [0, 16_000, 128_000] {
            let actual = remux(&input, Some(requested), &Limits::default()).unwrap();
            assert_eq!(actual, expected(&control, average, average.max(requested)));
        }
    }
}

#[test]
fn unproved_mp3_clocks_preserve_complete_files_without_refusal() {
    let good = frame(3, 5, 1);
    let compound = [good.clone(), good.clone()].concat();
    let mut free = good.clone();
    free[2] &= 0x0F;
    let mut layer2 = good.clone();
    layer2[1] = (layer2[1] & !6) | 4;
    for frames in [
        vec![good.clone(), compound],
        vec![free],
        vec![good[..good.len() - 1].to_vec()],
        vec![good.clone(), frame(3, 5, 0)],
        vec![good.clone(), frame(2, 8, 0)],
        vec![layer2],
        vec![vec![0; 192]],
        vec![vec![0; 3]],
    ] {
        let input = source(&frames, 48_000, 1152, 0x6B);
        let control = remux(&input, None, &Limits::default()).unwrap();
        assert_eq!(
            remux(&input, Some(96_000), &Limits::default()).unwrap(),
            control
        );
    }
}

#[test]
fn absent_request_inherits_the_stored_clock_not_declared_bitrate_fields() {
    let input = source(&[frame(3, 5, 1), frame(3, 5, 1)], 48_000, 1152, 0x6B);
    let control = remux(&input, None, &Limits::default()).unwrap();
    let mut output = Vec::new();
    remux_mp4(
        SliceSource::new(&input),
        &Limits::default(),
        &Allowlist::default(),
        &RemuxOptions {
            copied_audio_bit_rate: Some(CopiedAudioBitRate::FromSource),
            ..RemuxOptions::default()
        },
        &mut output,
    )
    .unwrap();
    let stored = 384 * 8 * 48_000 / (1152 + 433);
    assert_eq!(output, expected(&control, 64_000, stored));
}

#[test]
fn a_clock_pass_without_spare_work_preserves_complete_files() {
    let input = source(&[frame(3, 5, 1), frame(3, 5, 1)], 48_000, 1152, 0x6B);
    for work in 0..1024 {
        let limits = Limits {
            max_sample_work: work,
            ..Limits::default()
        };
        if let Some(control) = remux(&input, None, &limits) {
            assert_eq!(remux(&input, Some(96_000), &limits).unwrap(), control);
            return;
        }
    }
    panic!("no native two-pass work limit was found");
}

#[test]
fn an_optional_header_read_failure_preserves_the_native_output() {
    struct Failing<'a> {
        source: SliceSource<'a>,
        offset: u64,
        tripped: bool,
    }
    impl Source for Failing<'_> {
        fn len(&mut self) -> Result<u64, Error> {
            self.source.len()
        }
        fn read_exact_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<(), Error> {
            if offset == self.offset && bytes.len() == 4 {
                self.tripped = true;
                return Err(Error::Truncated { offset });
            }
            self.source.read_exact_at(offset, bytes)
        }
    }
    let frame = frame(3, 5, 1);
    let input = source(&[frame.clone(), frame.clone()], 48_000, 1152, 0x6B);
    let control = remux(&input, None, &Limits::default()).unwrap();
    let mut source = Failing {
        source: SliceSource::new(&input),
        offset: (input.len() - frame.len() * 2) as u64,
        tripped: false,
    };
    let mut actual = Vec::new();
    remux_mp4(
        &mut source,
        &Limits::default(),
        &Allowlist::default(),
        &RemuxOptions {
            copied_audio_bit_rate: Some(CopiedAudioBitRate::Requested(96_000)),
            ..RemuxOptions::default()
        },
        &mut actual,
    )
    .unwrap();
    assert!(source.tripped);
    assert_eq!(actual, control);
}
