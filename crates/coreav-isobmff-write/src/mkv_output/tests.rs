#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::{Replacement, ReplacementSample};
use coreav_isobmff::SliceSource;
use coreav_mkv::write::{CodecId, plan};

const AVC: [u8; 7] = [1, 100, 0, 30, 255, 224, 0];

struct Fixture {
    input: Vec<u8>,
    entry: Vec<u8>,
    samples: Vec<ReplacementSample>,
    data: Vec<u8>,
    last_duration: u32,
}

impl Fixture {
    fn new(times: &[i64], last_duration: u32) -> Self {
        let header =
            config::description(CodecId::H264, Some((16, 16)), None, None, None, None, &AVC)
                .unwrap();
        let packets = [Packet {
            track: 0,
            pts_ns: 0,
            duration_ns: Some(1_000_000_000),
            size: 1,
            keyframe: true,
            invisible: false,
            discard_padding_ns: None,
        }];
        let mut input = Vec::new();
        plan(
            &[header],
            &packets,
            Options {
                timestamp_scale_ns: 1,
                ..Options::default()
            },
            &coreav_mkv::Limits::default(),
        )
        .unwrap()
        .write(&mut input, |_, out| {
            out.write_all(&[7]).map_err(|e| Error::Io(e.kind()))
        })
        .unwrap();
        Self {
            input,
            entry: crate::mux::avc1_sample_entry(16, 16, &AVC, None, None).unwrap(),
            samples: times
                .iter()
                .enumerate()
                .map(|(index, &time)| ReplacementSample {
                    size: 1,
                    sync: index == 0,
                    decode_time: time,
                    presentation_time: time,
                })
                .collect(),
            data: vec![7; times.len()],
            last_duration,
        }
    }

    fn write(
        &self,
        cadence: Option<VideoCadence>,
        stream: usize,
        options: &OutputOptions,
        out: &mut Vec<u8>,
    ) -> Result<OutputSummary, Error> {
        let replacements = Replacements {
            video: Some(Replacement {
                stream_index: stream,
                sample_entry: &self.entry,
                width: 16 << 16,
                height: 16 << 16,
                samples: &self.samples,
                last_duration: self.last_duration,
                data: SliceSource::new(&self.data),
            }),
            audio: None,
        };
        match cadence {
            Some(cadence) => write_output_with_video_cadence(
                SliceSource::new(&self.input),
                options,
                &Limits::default(),
                &coreav_mkv::Limits::default(),
                replacements,
                cadence,
                out,
            ),
            None => write_output(
                SliceSource::new(&self.input),
                options,
                &Limits::default(),
                &coreav_mkv::Limits::default(),
                replacements,
                out,
            ),
        }
    }
}

fn options() -> OutputOptions {
    OutputOptions {
        matroska_source: true,
        doc_type: DocType::Matroska,
        streams: None,
    }
}

fn probe(bytes: &[u8]) -> coreav_mkv::Segment {
    coreav_mkv::probe(
        SliceSource::new(bytes),
        &coreav_mkv::Limits::default(),
        &coreav_mkv::Allowlist::any(),
    )
    .unwrap()
}

#[test]
fn video_cadence_checks_rate_bounds_and_rounds_nanoseconds() {
    for (num, den, expected) in [
        (25, 1, 40_000_000),
        (30, 1, 33_333_333),
        (24000, 1001, 41_708_333),
        (30000, 1001, 33_366_667),
        (1000, 33, 33_000_000),
        (1000, 1, 1_000_000),
        (u32::MAX, u32::MAX, 1_000_000_000),
        (1, u32::MAX, 4_294_967_295_000_000_000),
    ] {
        assert_eq!(
            VideoCadence::new(7, num, den).unwrap(),
            VideoCadence {
                stream_index: 7,
                default_duration_ns: expected,
            }
        );
    }
    for (num, den) in [(0, 1), (1, 0), (0, 0), (1001, 1), (u32::MAX, 1)] {
        assert!(matches!(
            VideoCadence::new(0, num, den),
            Err(Error::Unsupported { .. })
        ));
    }
}

#[test]
fn explicit_video_cadence_serializes_default_duration_without_block_overrides() {
    for (num, den, times, last, expected, pts) in [
        (
            30,
            1,
            vec![0, 33_333_333, 66_666_667],
            33_333_333,
            33_333_333,
            vec![0, 33, 67],
        ),
        (
            30000,
            1001,
            vec![0, 33_366_667, 66_733_333],
            33_366_667,
            33_366_667,
            vec![0, 33, 67],
        ),
        (
            24000,
            1001,
            vec![0, 41_708_333, 83_416_667],
            41_708_333,
            41_708_333,
            vec![0, 42, 83],
        ),
        (25, 1, vec![70_000_000], 40_000_000, 40_000_000, vec![70]),
    ] {
        let fixture = Fixture::new(&times, last);
        let mut bytes = Vec::new();
        fixture
            .write(
                Some(VideoCadence::new(0, num, den).unwrap()),
                0,
                &options(),
                &mut bytes,
            )
            .unwrap();
        let segment = probe(&bytes);
        assert_eq!(segment.timestamp_scale, 1_000_000);
        assert_eq!(segment.tracks[0].default_duration, Some(expected));
        let packets = segment
            .packets(SliceSource::new(&bytes), &coreav_mkv::Limits::default())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            packets.iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>(),
            pts
        );
        assert!(packets.iter().all(|p| p.block_duration.is_none()));
        assert!(packets[0].keyframe);
        assert!(packets.iter().skip(1).all(|p| !p.keyframe));
        assert!(packets.iter().all(|p| bytes[p.offset as usize] == 7));
    }
}

#[test]
fn ordinary_output_keeps_its_serialized_packet_duration_policy() {
    let fixture = Fixture::new(&[0, 33_333_333, 66_666_667], 33_333_333);
    let mut actual = Vec::new();
    fixture.write(None, 0, &options(), &mut actual).unwrap();
    let header = config::entry(&fixture.entry, false).unwrap();
    let packets: Vec<_> = [0, 33_000_000, 67_000_000]
        .into_iter()
        .enumerate()
        .map(|(index, pts_ns)| Packet {
            track: 0,
            pts_ns,
            duration_ns: Some(if index == 1 { 33_333_334 } else { 33_333_333 }),
            size: 1,
            keyframe: index == 0,
            invisible: false,
            discard_padding_ns: None,
        })
        .collect();
    let mut expected = Vec::new();
    plan(
        &[header],
        &packets,
        Options::default(),
        &coreav_mkv::Limits::default(),
    )
    .unwrap()
    .write(&mut expected, |_, out| {
        out.write_all(&[7]).map_err(|e| Error::Io(e.kind()))
    })
    .unwrap();
    assert_eq!(actual, expected);
    let segment = probe(&actual);
    assert_eq!(segment.tracks[0].default_duration, None);
    assert!(
        segment
            .packets(SliceSource::new(&actual), &coreav_mkv::Limits::default())
            .all(|p| p.unwrap().block_duration == Some(33))
    );
}

#[test]
fn video_cadence_refuses_millisecond_collisions_and_reversed_presentation_before_writing() {
    for (decode, present) in [
        (vec![0, 400_000], vec![0, 400_000]),
        (vec![0, 1_000_000], vec![1_000_000, 1_000_000]),
        (vec![0, 1_000_000], vec![2_000_000, 1_000_000]),
    ] {
        let mut fixture = Fixture::new(&decode, 1_000_000);
        for (sample, pts) in fixture.samples.iter_mut().zip(present) {
            sample.presentation_time = pts;
        }
        let mut bytes = Vec::new();
        assert!(matches!(
            fixture.write(
                Some(VideoCadence::new(0, 1000, 1).unwrap()),
                0,
                &options(),
                &mut bytes
            ),
            Err(Error::Unsupported { .. })
        ));
        assert!(bytes.is_empty());
    }
}

#[test]
fn video_cadence_requires_its_selected_avc_replacement() {
    let fixture = Fixture::new(&[0], 33_333_333);
    for (cadence_stream, replacement_stream, selected, doc_type) in [
        (1, 0, None, DocType::Matroska),
        (1, 1, None, DocType::Matroska),
        (0, 0, Some(Vec::new()), DocType::Matroska),
        (0, 0, None, DocType::Webm),
    ] {
        let mut bytes = Vec::new();
        let options = OutputOptions {
            streams: selected,
            doc_type,
            ..options()
        };
        assert!(matches!(
            fixture.write(
                Some(VideoCadence::new(cadence_stream, 30, 1).unwrap()),
                replacement_stream,
                &options,
                &mut bytes
            ),
            Err(Error::Unsupported { .. })
        ));
        assert!(bytes.is_empty());
    }
    let mut bytes = Vec::new();
    assert!(matches!(
        write_output_with_video_cadence(
            SliceSource::new(&fixture.input),
            &options(),
            &Limits::default(),
            &coreav_mkv::Limits::default(),
            Replacements::<SliceSource<'_>> {
                video: None,
                audio: None
            },
            VideoCadence::new(0, 30, 1).unwrap(),
            &mut bytes,
        ),
        Err(Error::Unsupported { .. })
    ));
    assert!(bytes.is_empty());
    let mut fixture = fixture;
    fixture.entry[4..8].copy_from_slice(b"hvc1");
    assert!(matches!(
        fixture.write(
            Some(VideoCadence::new(0, 30, 1).unwrap()),
            0,
            &options(),
            &mut bytes
        ),
        Err(Error::Unsupported { .. })
    ));
    assert!(bytes.is_empty());
}

fn opus_edit_fixture(media_time: u32) -> Fixture {
    use crate::mux::{
        Edit, FileType, MovieHeader, Muxer, RawBox, Sample, TrackHeader, TrackMedia,
        audio_sample_entry,
    };
    use coreav_isobmff::FourCc;

    let mut fixture = Fixture::new(&[0, 1, 2, 3, 4], 1);
    let mut dops = vec![0, 2];
    dops.extend(312u16.to_be_bytes());
    dops.extend(48_000u32.to_be_bytes());
    dops.extend([0, 0, 0]);
    let audio_entry = audio_sample_entry(
        FourCc(*b"Opus"),
        2,
        16,
        48_000,
        &[RawBox {
            kind: FourCc(*b"dOps"),
            payload: dops,
        }],
    )
    .unwrap();
    let mut muxer = Muxer::new(
        FileType {
            major: FourCc(*b"isom"),
            minor: 0,
            compatible: vec![FourCc(*b"isom")],
        },
        MovieHeader {
            timescale: 48_000,
            creation_time: 0,
            boxes: Vec::new(),
        },
        &Limits::default(),
    )
    .unwrap();
    for (media, timescale, entry, edits, count, duration) in [
        (
            TrackMedia::Video {
                matrix: coreav_isobmff::IDENTITY_MATRIX,
                width: 16 << 16,
                height: 16 << 16,
            },
            25,
            fixture.entry.clone(),
            Vec::new(),
            5,
            1,
        ),
        (
            TrackMedia::Audio { volume: 0x100 },
            48_000,
            audio_entry,
            vec![Edit {
                duration: 7992,
                media_time: i64::from(media_time),
                rate: Edit::UNIT_RATE,
            }],
            10,
            960,
        ),
    ] {
        let track = muxer
            .add_track(TrackHeader {
                media,
                timescale,
                language: *b"und",
                creation_time: 0,
                media_creation_time: 0,
                flags: 3,
                layer: 0,
                alternate_group: 0,
                edits,
                duration: None,
                media_duration: None,
                sample_entry: entry,
                sample_table_boxes: Vec::new(),
                boxes: Vec::new(),
            })
            .unwrap();
        for _ in 0..count {
            muxer
                .push_sample(
                    track,
                    Sample {
                        size: 1,
                        duration,
                        sync: true,
                        composition_offset: 0,
                    },
                )
                .unwrap();
        }
    }
    let mut bytes = Vec::new();
    muxer.write_head(&mut bytes).unwrap();
    muxer
        .for_each_chunk::<coreav_isobmff::Error>(|track, count| {
            bytes.extend(std::iter::repeat_n(
                if track == 0 { 7 } else { 31 << 3 },
                count as usize,
            ));
            Ok(())
        })
        .unwrap();
    fixture.input = bytes;
    fixture
}

#[test]
fn native_video_cadence_and_ordinary_copy_preserve_iso_opus_edit_presentation() {
    for media_time in [0, 960] {
        let fixture = opus_edit_fixture(media_time);
        let options = OutputOptions {
            matroska_source: false,
            ..options()
        };
        let source = coreav_isobmff::probe(
            SliceSource::new(&fixture.input),
            &Limits::default(),
            &coreav_isobmff::Allowlist::listing(),
        )
        .unwrap();
        let audio = &source.tracks[1];
        assert!(matches!(
            audio.edit_list,
            coreav_isobmff::EditList::Simple { media_time: time, .. }
                if time == i64::from(media_time)
        ));
        assert_eq!(
            audio.presentation(source.timescale).unwrap(),
            (
                coreav_isobmff::Rational::from_int(0),
                coreav_isobmff::Rational::new(7992, 48_000).unwrap()
            )
        );
        let mut ordinary = Vec::new();
        fixture.write(None, 0, &options, &mut ordinary).unwrap();
        let mut native = Vec::new();
        fixture
            .write(
                Some(VideoCadence::new(0, 25, 1).unwrap()),
                0,
                &options,
                &mut native,
            )
            .unwrap();
        assert_eq!(ordinary, native);
        for (bytes, trim, presented_samples) in
            [(&ordinary, media_time, 7992), (&native, media_time, 7992)]
        {
            let segment = probe(bytes);
            let header = &segment.tracks[1];
            let delay = u64::from(trim) * 1_000_000_000 / 48_000;
            assert_eq!(header.codec_delay, delay);
            let opus_head = header.codec_private.as_ref().unwrap();
            assert_eq!(&opus_head[..8], b"OpusHead");
            assert_eq!(
                u16::from_le_bytes(opus_head[10..12].try_into().unwrap()),
                trim as u16
            );
            let packets: Vec<_> = segment
                .packets(SliceSource::new(bytes), &coreav_mkv::Limits::default())
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .into_iter()
                .filter(|p| p.track == 1)
                .collect();
            assert_eq!(packets.len(), if media_time == 0 { 9 } else { 10 });
            assert_eq!(packets[0].block_timestamp, 0);
            assert_eq!(packets[0].pts_ns(&segment), Some(-i128::from(delay)));
            assert_eq!(packets.last().unwrap().discard_padding, Some(13_500_000));
            assert!(
                packets
                    .iter()
                    .take(packets.len() - 1)
                    .all(|p| p.discard_padding.is_none())
            );
            for (index, packet) in packets.iter().enumerate() {
                let ideal = index as i128 * 20_000_000 - i128::from(delay);
                let stored = packet.pts_ns(&segment).unwrap();
                assert!(stored.abs_diff(ideal) <= 1_000_000);
                assert_eq!(bytes[packet.offset as usize], 31 << 3);
            }
            let padding_samples =
                packets.last().unwrap().discard_padding.unwrap() as u64 * 48_000 / 1_000_000_000;
            assert_eq!(
                packets.len() as u64 * 960 - u64::from(trim) - padding_samples,
                presented_samples
            );
        }
    }
}
