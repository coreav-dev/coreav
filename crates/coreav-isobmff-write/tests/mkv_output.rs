//! Source edit windows, replacement boundaries and payload ownership.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;
#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;
mod movies;
use coreav_isobmff::{Limits, SliceSource};
use coreav_isobmff_write::mkv_output::{OutputOptions, copied_audio, write_output};
use coreav_isobmff_write::{
    AudioReplacement, AudioSample, Replacement, ReplacementSample, Replacements,
};
use coreav_mkv::{Allowlist, DocType};

fn options() -> OutputOptions {
    OutputOptions {
        matroska_source: false,
        doc_type: DocType::Matroska,
        streams: None,
    }
}
fn copy(input: &[u8]) -> Result<Vec<u8>, coreav_mkv::Error> {
    let mut out = Vec::new();
    write_output(
        SliceSource::new(input),
        &options(),
        &Limits::default(),
        &coreav_mkv::Limits::default(),
        Replacements::<SliceSource<'_>> {
            video: None,
            audio: None,
        },
        &mut out,
    )?;
    Ok(out)
}
fn probe(bytes: &[u8]) -> coreav_mkv::Segment {
    coreav_mkv::probe(
        SliceSource::new(bytes),
        &coreav_mkv::Limits::default(),
        &Allowlist::any(),
    )
    .unwrap()
}

#[test]
fn copied_container_colour_and_scan_do_not_leak_into_replacement_headers() {
    use coreav_isobmff::Color;
    let avcc = [1, 100, 0, 30, 255, 224, 0];
    let input = mkv::file(
        "matroska",
        &[
            mkv::info(None),
            mkv::el(
                mkv::TRACKS,
                &mkv::el(
                    mkv::TRACK_ENTRY,
                    &mkv::cat(&[
                        mkv::uint(0xd7, 1),
                        mkv::uint(0x73c5, 1),
                        mkv::uint(0x83, 1),
                        mkv::string(0x86, "V_MPEG4/ISO/AVC"),
                        mkv::el(0x63a2, &avcc),
                        mkv::uint(0x23e383, 40_000_000),
                        mkv::el(
                            0xe0,
                            &mkv::cat(&[
                                mkv::uint(0xb0, 320),
                                mkv::uint(0xba, 240),
                                mkv::uint(0x9a, 2),
                                mkv::el(
                                    0x55b0,
                                    &mkv::cat(&[
                                        mkv::uint(0x55b1, 1),
                                        mkv::uint(0x55b9, 2),
                                        mkv::uint(0x55ba, 16),
                                        mkv::uint(0x55bb, 9),
                                    ]),
                                ),
                            ]),
                        ),
                    ]),
                ),
            ),
            mkv::cluster(0, &[mkv::simple_block(1, 0, true, &[7])]),
        ],
    );
    let source_colour = Color {
        primaries: Some(9),
        transfer: Some(16),
        matrix: Some(1),
        full_range: Some(true),
    };
    assert_eq!(probe(&input).tracks[0].colour, source_colour);
    for (replaced, declared) in [
        (false, None),
        (true, None),
        (
            true,
            Some(coreav_isobmff_write::mux::Nclx {
                primaries: 2,
                transfer: 2,
                matrix: 2,
                full_range: false,
            }),
        ),
    ] {
        let entry =
            coreav_isobmff_write::mux::avc1_sample_entry(320, 240, &avcc, None, declared).unwrap();
        let samples = [ReplacementSample {
            size: 1,
            sync: true,
            decode_time: 0,
            presentation_time: 0,
        }];
        let mut output = Vec::new();
        write_output(
            SliceSource::new(&input),
            &OutputOptions {
                matroska_source: true,
                ..options()
            },
            &Limits::default(),
            &coreav_mkv::Limits::default(),
            Replacements {
                video: replaced.then_some(Replacement {
                    stream_index: 0,
                    sample_entry: &entry,
                    width: 320 << 16,
                    height: 240 << 16,
                    samples: &samples,
                    last_duration: 40,
                    data: SliceSource::new(&[8]),
                }),
                audio: None,
            },
            &mut output,
        )
        .unwrap();
        let segment = probe(&output);
        let expected = if replaced {
            Color {
                full_range: declared.map(|_| false),
                ..Color::default()
            }
        } else {
            source_colour
        };
        assert_eq!(segment.tracks[0].colour, expected);
        assert_eq!(
            segment.tracks[0].video.as_ref().unwrap().interlaced,
            if replaced { 0 } else { 2 }
        );
    }
}

#[test]
fn extreme_edit_boundaries_refuse_without_overflowing_the_preroll_distance() {
    let mut input = common::movie(&common::Spec {
        sizes: vec![4],
        composition: vec![0],
        elst: Some(vec![(40, 0)]),
        ..common::Spec::default()
    });
    let elst = input.windows(4).position(|v| v == b"elst").unwrap();
    let size = u32::from_be_bytes(input[elst - 4..elst].try_into().unwrap()) as usize;
    let mut entries = 2u32.to_be_bytes().to_vec();
    for (duration, media) in [
        (8_000_000_000_000_000_000u64, -1i64),
        (40, 8_000_000_000_000_000_000),
    ] {
        entries.extend(duration.to_be_bytes());
        entries.extend(media.to_be_bytes());
        entries.extend([0, 1, 0, 0]);
    }
    let replacement = common::full(b"elst", 1, 0, &entries);
    let growth = replacement.len() - size;
    input.splice(elst - 4..elst - 4 + size, replacement);
    for kind in [b"moov", b"trak", b"edts"] {
        let at = input.windows(4).position(|v| v == kind).unwrap();
        let size = u32::from_be_bytes(input[at - 4..at].try_into().unwrap()) + growth as u32;
        input[at - 4..at].copy_from_slice(&size.to_be_bytes());
    }
    for (kind, timescale) in [(b"mvhd", 1_000_000_000u32), (b"mdhd", 500_000_000)] {
        let at = input.windows(4).position(|v| v == kind).unwrap();
        input[at + 16..at + 20].copy_from_slice(&timescale.to_be_bytes());
    }
    let movie = coreav_isobmff::probe(
        SliceSource::new(&input),
        &Limits::default(),
        &coreav_isobmff::Allowlist::listing(),
    )
    .unwrap();
    assert!(matches!(
        movie.tracks[0].edit_list,
        coreav_isobmff::EditList::Simple {
            empty: 8_000_000_000_000_000_000,
            media_time: 8_000_000_000_000_000_000,
            ..
        }
    ));
    assert!(copy(&input).is_err());
}

#[test]
fn aac_empty_edit_and_finite_window_preserve_priming_and_tail_padding() {
    let input = movies::movie(&[movies::aac_audio(6, vec![(100, -1), (40, 1024)])]);
    let output = copy(&input).unwrap();
    let segment = probe(&output);
    assert_eq!(segment.tracks[0].codec_delay, 21_333_333);
    let packets = segment
        .packets(SliceSource::new(&output), &coreav_mkv::Limits::default())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(packets.len(), 3);
    assert_eq!(packets.last().unwrap().discard_padding, Some(2_666_666));
    assert!(
        packets
            .iter()
            .all(|p| p.pts_ns(&segment).unwrap() < 140_000_000)
    );
}

#[test]
fn video_preroll_inside_an_empty_edit_is_invisible_and_a_trailing_suffix_is_dropped() {
    let input = common::movie(&common::Spec {
        sizes: vec![1; 6],
        ctts: Some(Vec::new()),
        elst: Some(vec![(100, -1), (80, 512)]),
        ..Default::default()
    });
    let output = copy(&input).unwrap();
    let segment = probe(&output);
    let packets = segment
        .packets(SliceSource::new(&output), &coreav_mkv::Limits::default())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(packets.len(), 3);
    assert!(packets[0].invisible);
    assert!(!packets[1].invisible);
    assert!(!packets[2].invisible);
    assert_eq!(packets[1].pts_ns(&segment), Some(100_000_000));
}

#[test]
fn a_replaced_unknown_codec_needs_no_source_copy_mapping() {
    let input = common::movie(&common::Spec {
        sample_entry: *b"xxxx",
        ctts: Some(Vec::new()),
        elst: None,
        ..Default::default()
    });
    assert!(copy(&input).is_err());
    let entry = coreav_isobmff_write::mux::avc1_sample_entry(
        320,
        240,
        &[1, 100, 0, 30, 255, 224, 0],
        None,
        None,
    )
    .unwrap();
    let samples = [ReplacementSample {
        size: 1,
        sync: true,
        decode_time: 0,
        presentation_time: 0,
    }];
    let mut output = Vec::new();
    write_output(
        SliceSource::new(&input),
        &options(),
        &Limits::default(),
        &coreav_mkv::Limits::default(),
        Replacements {
            video: Some(Replacement {
                stream_index: 0,
                sample_entry: &entry,
                width: 320 << 16,
                height: 240 << 16,
                samples: &samples,
                last_duration: 512,
                data: SliceSource::new(&[7]),
            }),
            audio: None,
        },
        &mut output,
    )
    .unwrap();
    let segment = probe(&output);
    assert_eq!(segment.tracks[0].language, "und");
    let packets = segment
        .packets(SliceSource::new(&output), &coreav_mkv::Limits::default())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(packets.len(), 1);
    assert_eq!(output[packets[0].offset as usize], 7);
}

#[test]
fn wholly_padded_replacement_packets_are_not_written() {
    let input = movies::movie(&[movies::aac_audio(4, Vec::new())]);
    let entry = coreav_isobmff_write::mux::audio_sample_entry(
        coreav_isobmff::FourCc(*b"mp4a"),
        1,
        16,
        48_000,
        &[coreav_isobmff_write::mux::RawBox {
            kind: coreav_isobmff::FourCc(*b"esds"),
            payload: coreav_isobmff_write::mux::esds(&[0x11, 0x90]),
        }],
    )
    .unwrap();
    let samples = [AudioSample {
        size: 1,
        duration: 1024,
    }; 3];
    let mut output = Vec::new();
    write_output(
        SliceSource::new(&input),
        &options(),
        &Limits::default(),
        &coreav_mkv::Limits::default(),
        Replacements {
            video: None,
            audio: Some(AudioReplacement {
                stream_index: 0,
                sample_entry: &entry,
                timescale: 48_000,
                samples: &samples,
                start: 0,
                priming: 0,
                roll: 0,
                presented: 1536,
                aac_mp4_tail: true,
                data: SliceSource::new(&[1, 2, 3]),
            }),
        },
        &mut output,
    )
    .unwrap();
    let segment = probe(&output);
    let packets = segment
        .packets(SliceSource::new(&output), &coreav_mkv::Limits::default())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[1].discard_padding, Some(10_666_667));
}

#[test]
fn quicktime_sound_packet_tables_are_refused_before_codec_mapping() {
    let input = common::sound::qdm2_movie(&[2048], 0);
    assert!(
        matches!(copy(&input),Err(coreav_mkv::Error::Unsupported{reason}) if reason.contains("frame-based"))
    );
}

#[test]
fn duplicate_selections_share_the_total_packet_limit() {
    let input = common::movie(&common::Spec {
        ctts: Some(Vec::new()),
        elst: None,
        ..Default::default()
    });
    let mut selection = options();
    selection.streams = Some(vec![0, 0]);
    let limits = coreav_mkv::Limits {
        max_block_work: 6,
        ..Default::default()
    };
    let result = write_output(
        SliceSource::new(&input),
        &selection,
        &Limits::default(),
        &limits,
        Replacements::<SliceSource<'_>> {
            video: None,
            audio: None,
        },
        Vec::new(),
    );
    assert!(matches!(
        result,
        Err(coreav_mkv::Error::LimitExceeded { .. })
    ));
}

fn opus_movie(media_time: i32) -> Vec<u8> {
    let mut spec = movies::aac_audio(5, vec![(40, media_time)]);
    for sample in &mut spec.samples {
        sample.duration = 960;
    }
    let mut input = movies::movie(&[spec]);
    let entry = input.windows(4).position(|v| v == b"mp4a").unwrap();
    input[entry..entry + 4].copy_from_slice(b"Opus");
    let child = input.windows(4).position(|v| v == b"esds").unwrap() - 4;
    let size = u32::from_be_bytes(input[child..child + 4].try_into().unwrap()) as usize;
    let mut config = vec![0, 2];
    config.extend(312u16.to_be_bytes());
    config.extend(48_000u32.to_be_bytes());
    config.extend([0, 0, 0]);
    let mut boxes = common::bx(b"dOps", &config);
    boxes.extend(common::bx(b"free", &vec![0; size - boxes.len() - 8]));
    input[child..child + size].copy_from_slice(&boxes);
    let movie = coreav_isobmff::probe(
        SliceSource::new(&input),
        &Limits::default(),
        &coreav_isobmff::Allowlist::listing(),
    )
    .unwrap();
    let mut budget = coreav_isobmff::Budget::new(&Limits::default());
    let packets = movie
        .packets(0, SliceSource::new(&input), &Limits::default(), &mut budget)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for packet in packets {
        input[packet.offset as usize] = 31 << 3;
    }
    input
}

#[test]
fn iso_opus_trim_owns_both_preskip_and_codec_delay() {
    for (media, trim, count, padding) in [(96, 312, 3, 18_000_000), (960, 960, 3, 0)] {
        let output = copy(&opus_movie(media)).unwrap();
        let segment = probe(&output);
        let track = &segment.tracks[0];
        assert_eq!(track.codec_delay, (trim as u64) * 1_000_000_000 / 48_000);
        assert_eq!(
            u16::from_le_bytes(
                track.codec_private.as_ref().unwrap()[10..12]
                    .try_into()
                    .unwrap()
            ),
            trim
        );
        let packets = segment
            .packets(SliceSource::new(&output), &coreav_mkv::Limits::default())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(packets.len(), count);
        assert_eq!(
            packets.last().unwrap().discard_padding.unwrap_or(0),
            padding
        );
    }
}

#[test]
fn laced_block_duration_and_padding_belong_to_the_boundary_frame() {
    for default in [true, false] {
        let extra = mkv::cat(&[
            mkv::uint(0x6264, 16),
            if default {
                mkv::uint(0x23_E383, 20_000_000)
            } else {
                Vec::new()
            },
        ]);
        let input = mkv::file(
            "matroska",
            &[
                mkv::info(None),
                mkv::el(
                    mkv::TRACKS,
                    &mkv::el(
                        mkv::TRACK_ENTRY,
                        &mkv::cat(&[
                            mkv::uint(0xd7, 1),
                            mkv::uint(0x73c5, 1),
                            mkv::uint(0x83, 2),
                            mkv::string(0x86, "A_PCM/INT/LIT"),
                            mkv::el(
                                0xe1,
                                &mkv::cat(&[
                                    mkv::float(0xb5, 48_000.0),
                                    mkv::uint(0x9f, 1),
                                    mkv::uint(0x6264, 16),
                                ]),
                            ),
                            extra,
                        ]),
                    ),
                ),
                mkv::cluster(
                    0,
                    &[mkv::el(
                        mkv::BLOCK_GROUP,
                        &mkv::cat(&[
                            mkv::el(0xa1, &[0x81, 0, 0, 4, 1, 11, 12, 21, 22]),
                            mkv::uint(0x9b, 35),
                            mkv::sint(0x75a2, 5_000_000),
                        ]),
                    )],
                ),
            ],
        );
        let mut selection = options();
        selection.matroska_source = true;
        let mut output = Vec::new();
        let result = write_output(
            SliceSource::new(&input),
            &selection,
            &Limits::default(),
            &coreav_mkv::Limits::default(),
            Replacements::<SliceSource<'_>> {
                video: None,
                audio: None,
            },
            &mut output,
        );
        if !default {
            assert!(matches!(result, Err(coreav_mkv::Error::Unsupported { .. })));
            continue;
        }
        result.unwrap();
        let segment = probe(&output);
        let packets = segment
            .packets(SliceSource::new(&output), &coreav_mkv::Limits::default())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].discard_padding, None);
        assert_eq!(packets[1].discard_padding, Some(5_000_000));
        assert_eq!(packets[1].block_duration, Some(15));
        assert_eq!(packets[1].pts_ns(&segment), Some(20_000_000));
        for (packet, bytes) in packets.iter().zip([[11, 12], [21, 22]]) {
            assert_eq!(
                &output[packet.offset as usize..packet.offset as usize + 2],
                bytes
            );
        }
    }
}

struct BeforePackets<'a> {
    bytes: &'a [u8],
    reads: &'a std::cell::Cell<usize>,
    limit: usize,
}
impl coreav_isobmff::Source for BeforePackets<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(
        &mut self,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<(), coreav_isobmff::Error> {
        self.reads.set(self.reads.get() + 1);
        assert!(
            self.reads.get() <= self.limit,
            "packet traversal happened before the metadata refusal"
        );
        coreav_isobmff::Source::read_exact_at(&mut SliceSource::new(self.bytes), offset, buffer)
    }
}

#[test]
fn repeated_track_metadata_is_refused_before_packet_traversal() {
    let input = mkv::file(
        "matroska",
        &[
            mkv::info(None),
            mkv::el(
                mkv::TRACKS,
                &mkv::video_track(
                    1,
                    "V_VP9",
                    320,
                    240,
                    &mkv::cat(&[
                        mkv::string(0x536e, &"x".repeat(4096)),
                        mkv::uint(0x23_E383, 40_000_000),
                    ]),
                ),
            ),
            mkv::cluster(0, &[mkv::simple_block(1, 0, true, &mkv::VP9_KEY_FRAME)]),
        ],
    );
    let reads = std::cell::Cell::new(0);
    let mut reference = BeforePackets {
        bytes: &input,
        reads: &reads,
        limit: usize::MAX,
    };
    coreav_mkv::probe(
        &mut reference,
        &coreav_mkv::Limits::default(),
        &Allowlist::any(),
    )
    .unwrap();
    let probe_reads = reads.get();
    reads.set(0);
    let mut selection = options();
    selection.matroska_source = true;
    selection.streams = Some(vec![0; 32]);
    let limits = coreav_mkv::Limits {
        max_retained_bytes: 16 * 1024,
        ..Default::default()
    };
    let result = write_output(
        BeforePackets {
            bytes: &input,
            reads: &reads,
            limit: probe_reads,
        },
        &selection,
        &Limits::default(),
        &limits,
        Replacements::<SliceSource<'_>> {
            video: None,
            audio: None,
        },
        Vec::new(),
    );
    assert!(matches!(
        result,
        Err(coreav_mkv::Error::LimitExceeded {
            limit: coreav_mkv::LimitKind::RetainedBytes,
            ..
        })
    ));
    let accepted = coreav_mkv::Limits {
        max_retained_bytes: 256 * 1024,
        ..limits
    };
    write_output(
        SliceSource::new(&input),
        &selection,
        &Limits::default(),
        &accepted,
        Replacements::<SliceSource<'_>> {
            video: None,
            audio: None,
        },
        Vec::new(),
    )
    .unwrap();
}

#[test]
fn iso_duplicate_headers_obey_the_same_retained_metadata_budget() {
    let input = common::movie(&common::Spec {
        elst: None,
        ctts: Some(Vec::new()),
        ..Default::default()
    });
    let mut selection = options();
    selection.streams = Some(vec![0; 32]);
    let limits = coreav_mkv::Limits {
        max_retained_bytes: 1024,
        ..Default::default()
    };
    let result = write_output(
        SliceSource::new(&input),
        &selection,
        &Limits::default(),
        &limits,
        Replacements::<SliceSource<'_>> {
            video: None,
            audio: None,
        },
        Vec::new(),
    );
    assert!(matches!(
        result,
        Err(coreav_mkv::Error::LimitExceeded {
            limit: coreav_mkv::LimitKind::RetainedBytes,
            ..
        })
    ));
}

#[test]
fn copied_audio_preflight_matches_the_writer_refusal() {
    let sowt = common::av_movie(b"sowt");
    let early = copied_audio(&mut SliceSource::new(&sowt), &[1]);
    let late = copy(&sowt);
    match (early, late) {
        (
            Err(coreav_mkv::Error::Unsupported { reason: early }),
            Err(coreav_mkv::Error::Unsupported { reason: late }),
        ) => {
            assert_eq!(early, late);
            assert_eq!(early, "an ISO codec without a Matroska mapping");
        }
        (early, late) => panic!("expected the same unsupported refusal, got {early:?} / {late:?}"),
    }
    let aac = movies::movie(&[movies::aac_audio(4, Vec::new())]);
    assert!(copied_audio(&mut SliceSource::new(&aac), &[0]).is_ok());
}

#[test]
fn unrepresentable_sampling_frequencies_are_refused_instead_of_changed() {
    for (rate, output) in [(24_000.5, None), (24_000.0, Some(48_000.0))] {
        let mut audio = mkv::cat(&[
            mkv::float(0xb5, rate),
            mkv::uint(0x9f, 1),
            mkv::uint(0x6264, 16),
        ]);
        if let Some(output) = output {
            audio.extend(mkv::float(0x78b5, output));
        }
        let track = mkv::el(
            mkv::TRACK_ENTRY,
            &mkv::cat(&[
                mkv::uint(0xd7, 1),
                mkv::uint(0x73c5, 1),
                mkv::uint(0x83, 2),
                mkv::string(0x86, "A_PCM/INT/LIT"),
                mkv::el(0xe1, &audio),
            ]),
        );
        let input = mkv::file(
            "matroska",
            &[
                mkv::info(None),
                mkv::el(mkv::TRACKS, &track),
                mkv::cluster(0, &[mkv::simple_block(1, 0, true, &[0, 1])]),
            ],
        );
        let mut selection = options();
        selection.matroska_source = true;
        assert!(
            matches!(write_output(SliceSource::new(&input),&selection,&Limits::default(),&coreav_mkv::Limits::default(),Replacements::<SliceSource<'_>>{video:None,audio:None},Vec::new()),Err(coreav_mkv::Error::Unsupported{reason}) if reason.contains("sampling frequency"))
        );
    }
}
