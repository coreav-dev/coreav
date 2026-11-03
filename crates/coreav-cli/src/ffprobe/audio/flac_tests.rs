#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use coreav_isobmff::SliceSource;

fn streaminfo(channels: u32, samples: u64) -> Vec<u8> {
    let mut bytes = vec![0; 34];
    bytes[..2].copy_from_slice(&16u16.to_be_bytes());
    bytes[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let packed = (44_100u64 << 44) | (u64::from(channels - 1) << 41) | (15 << 36) | samples;
    bytes[10..18].copy_from_slice(&packed.to_be_bytes());
    bytes
}

fn comment(fields: &[&str]) -> Vec<u8> {
    let mut bytes = 3u32.to_le_bytes().to_vec();
    bytes.extend(b"own");
    bytes.extend((fields.len() as u32).to_le_bytes());
    for field in fields {
        bytes.extend((field.len() as u32).to_le_bytes());
        bytes.extend(field.as_bytes());
    }
    bytes
}

fn file(blocks: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = b"fLaC".to_vec();
    for (index, (kind, block)) in blocks.iter().enumerate() {
        bytes.push(kind | if index + 1 == blocks.len() { 128 } else { 0 });
        bytes.extend(&(block.len() as u32).to_be_bytes()[1..]);
        bytes.extend(block);
    }
    bytes
}

fn read(bytes: &[u8], limits: &Limits) -> Result<Report, Failure> {
    probe(
        &mut SliceSource::new(bytes),
        bytes.len() as u64,
        "own.flac",
        limits,
        0,
    )
}

fn entry<'a>(report: &'a Report, key: &str) -> &'a Value {
    &report.streams[0]
        .entries
        .iter()
        .find(|(k, _)| *k == key)
        .unwrap()
        .1
}

#[test]
fn streaminfo_supplies_fields_and_unknown_samples_stay_unknown() {
    let bytes = file(&[(0, streaminfo(3, 0))]);
    let report = read(&bytes, &Limits::default()).ok().unwrap();
    assert_eq!(entry(&report, "channel_layout"), &model::s("3.0"));
    assert_eq!(entry(&report, "duration_ts"), &Value::Na);
    assert_eq!(entry(&report, "duration"), &Value::Na);
    assert_eq!(entry(&report, "start_pts"), &Value::Na);
    assert_eq!(entry(&report, "extradata_size"), &Value::Int(34));
    assert_eq!(entry(&report, "bits_per_raw_sample"), &model::s("16"));
    assert_eq!(report.time_bases, [(1, 44_100)]);
}

#[test]
fn stream_start_is_unknown_for_one_frame_and_known_for_multiple_frames() {
    for (payload, expected) in [(10, Value::Na), (20, Value::Int(0))] {
        let mut info = streaminfo(1, 0);
        info[4..7].copy_from_slice(&[0, 0, 10]);
        info[7..10].copy_from_slice(&[0, 0, 10]);
        let mut bytes = file(&[(0, info)]);
        bytes.extend(vec![0; payload]);
        let report = read(&bytes, &Limits::default()).ok().unwrap();
        assert_eq!(entry(&report, "start_pts"), &expected);
        assert_eq!(entry(&report, "duration_ts"), &Value::Na);
    }
    let mut bytes = file(&[(0, streaminfo(1, 0))]);
    bytes.push(0);
    assert!(matches!(
        read(&bytes, &Limits::default()),
        Err(Failure::Unsupported(_))
    ));
}

#[test]
fn duplicate_comments_move_to_the_end_before_aliases_are_applied() {
    let bytes = file(&[
        (0, streaminfo(1, 4410)),
        (
            4,
            comment(&[
                "TITLE=one",
                "ARTIST=a",
                "DESCRIPTION=d",
                "TRACKNUMBER=2",
                "custom_key=x",
                "title=two",
                "TITLE=",
                "COMMENT=separate",
                "ALBUMARTIST=band",
                "DISCNUMBER=3",
            ]),
        ),
    ]);
    let report = read(&bytes, &Limits::default()).ok().unwrap();
    assert_eq!(
        report.format_tags,
        [
            ("custom_key".to_owned(), "x".to_owned()),
            ("ARTIST".to_owned(), "a".to_owned()),
            ("comment".to_owned(), "d".to_owned()),
            ("track".to_owned(), "2".to_owned()),
            ("title".to_owned(), "one;two".to_owned()),
            ("COMMENT".to_owned(), "separate".to_owned()),
            ("album_artist".to_owned(), "band".to_owned()),
            ("disc".to_owned(), "3".to_owned()),
        ]
    );
}

#[test]
fn explicit_masks_preserve_back_speakers_and_accept_fewer_speakers() {
    for (channels, mask, expected, kept) in [
        (6, "0x3F", "5.1", false),
        (6, "0x60f", "5.1(side)", false),
        (2, "0x30", "2 channels (BL+BR)", false),
        (1, "0X0008", "1 channels (LFE)", false),
        (2, "0x4", "stereo", false),
        (2, "0x0", "stereo", true),
    ] {
        let tag = format!("WAVEFORMATEXTENSIBLE_CHANNEL_MASK={mask}");
        let bytes = file(&[(0, streaminfo(channels, 0)), (4, comment(&[&tag]))]);
        let report = read(&bytes, &Limits::default()).ok().unwrap();
        assert_eq!(entry(&report, "channel_layout"), &model::s(expected));
        assert_eq!(!report.format_tags.is_empty(), kept);
    }
}

#[test]
fn metadata_structure_and_unsupported_reports_are_distinct() {
    for blocks in [
        vec![(1, vec![])],
        vec![(0, streaminfo(1, 0)), (0, streaminfo(1, 0))],
        vec![(0, streaminfo(1, 0)), (4, comment(&[])), (4, comment(&[]))],
        vec![(0, streaminfo(1, 0)), (3, vec![0; 17])],
        vec![(0, streaminfo(1, 0)), (3, vec![]), (3, vec![])],
        vec![(0, streaminfo(1, 0)), (127, vec![])],
        vec![(0, vec![0; 33])],
    ] {
        assert!(matches!(
            read(&file(&blocks), &Limits::default()),
            Err(Failure::Invalid(_))
        ));
    }
    for typ in [2, 5, 6, 7] {
        let bytes = file(&[(0, streaminfo(1, 0)), (typ, vec![])]);
        assert!(matches!(
            read(&bytes, &Limits::default()),
            Err(Failure::Unsupported(_))
        ));
    }
    for key in [
        "METADATA_BLOCK_PICTURE",
        "COVERART",
        "REPLAYGAIN_TRACK_GAIN",
        "CHAPTER001",
    ] {
        let field = format!("{key}=value");
        let bytes = file(&[(0, streaminfo(1, 0)), (4, comment(&[&field]))]);
        assert!(matches!(
            read(&bytes, &Limits::default()),
            Err(Failure::Unsupported(_))
        ));
    }
}

#[test]
fn truncated_blocks_and_missing_last_flags_fail() {
    let mut bytes = file(&[(0, streaminfo(1, 0)), (4, comment(&["TITLE=own"]))]);
    bytes.pop();
    assert!(matches!(
        read(&bytes, &Limits::default()),
        Err(Failure::Library(Error::Truncated { .. }))
    ));
    let mut bytes = file(&[(0, streaminfo(1, 0))]);
    bytes[4] &= 127;
    assert!(matches!(
        read(&bytes, &Limits::default()),
        Err(Failure::Library(Error::Truncated { .. }))
    ));
}

#[test]
fn malformed_comments_and_streaminfo_fail() {
    let vendor_without_count = vec![3, 0, 0, 0, b'o', b'w', b'n'];
    let bytes = file(&[(0, streaminfo(1, 0)), (4, vendor_without_count)]);
    assert!(matches!(
        read(&bytes, &Limits::default()),
        Err(Failure::Invalid(_))
    ));
    for fields in [
        vec!["no equals"],
        vec!["=empty name"],
        vec!["bad\nname=value"],
        vec!["WAVEFORMATEXTENSIBLE_CHANNEL_MASK=not-hex"],
    ] {
        let bytes = file(&[(0, streaminfo(1, 0)), (4, comment(&fields))]);
        assert!(matches!(
            read(&bytes, &Limits::default()),
            Err(Failure::Invalid(_))
        ));
    }
    let mut info = streaminfo(1, 0);
    info[..2].copy_from_slice(&15u16.to_be_bytes());
    assert!(matches!(
        read(&file(&[(0, info)]), &Limits::default()),
        Err(Failure::Invalid(_))
    ));
}

#[test]
fn caller_limits_bound_headers_comments_and_report_storage() {
    let bytes = file(&[
        (0, streaminfo(1, 0)),
        (4, comment(&["TITLE=own", "ARTIST=own"])),
    ]);
    for (limits, kind) in [
        (
            Limits {
                max_tracks: 0,
                ..Limits::default()
            },
            LimitKind::Tracks,
        ),
        (
            Limits {
                max_depth: 0,
                ..Limits::default()
            },
            LimitKind::Depth,
        ),
        (
            Limits {
                max_top_level_boxes: 1,
                ..Limits::default()
            },
            LimitKind::TopLevelBoxes,
        ),
        (
            Limits {
                max_total_boxes: 1,
                ..Limits::default()
            },
            LimitKind::TotalBoxes,
        ),
        (
            Limits {
                max_box_bytes: 33,
                ..Limits::default()
            },
            LimitKind::BoxBytes,
        ),
        (
            Limits {
                max_sample_work: 2,
                ..Limits::default()
            },
            LimitKind::SampleWork,
        ),
        (
            Limits {
                max_retained_bytes: 1,
                ..Limits::default()
            },
            LimitKind::RetainedBytes,
        ),
        (
            Limits {
                max_table_bytes: 1,
                ..Limits::default()
            },
            LimitKind::TableBytes,
        ),
    ] {
        assert!(
            matches!(read(&bytes, &limits), Err(Failure::Library(Error::LimitExceeded { limit, .. })) if limit == kind)
        );
    }
}

#[test]
fn positioned_skip_does_not_read_padding_or_seektable_payload() {
    struct Checked {
        bytes: Vec<u8>,
        reads: Vec<(u64, usize)>,
    }
    impl Source for Checked {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.bytes.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, target: &mut [u8]) -> Result<(), Error> {
            self.reads.push((offset, target.len()));
            SliceSource::new(&self.bytes).read_exact_at(offset, target)
        }
    }
    let bytes = file(&[(0, streaminfo(1, 0)), (1, vec![0; 8192]), (3, vec![0; 36])]);
    let size = bytes.len() as u64;
    let mut source = Checked {
        bytes,
        reads: vec![],
    };
    let limits = Limits {
        max_box_bytes: 34,
        ..Limits::default()
    };
    assert!(probe(&mut source, size, "own.flac", &limits, 0).is_ok());
    assert_eq!(source.reads, [(0, 4), (4, 4), (8, 34), (42, 4), (8238, 4)]);
}

#[test]
fn source_io_and_limit_errors_are_never_optional_metadata() {
    struct Failing {
        bytes: Vec<u8>,
        error: Error,
    }
    impl Source for Failing {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.bytes.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, target: &mut [u8]) -> Result<(), Error> {
            if offset >= 46 {
                return Err(self.error.clone());
            }
            SliceSource::new(&self.bytes).read_exact_at(offset, target)
        }
    }
    for error in [
        Error::Io(std::io::ErrorKind::PermissionDenied),
        Error::LimitExceeded {
            limit: LimitKind::ReadBytes,
            offset: 46,
        },
    ] {
        let bytes = file(&[(0, streaminfo(1, 0)), (4, comment(&["TITLE=own"]))]);
        let size = bytes.len() as u64;
        let mut source = Failing {
            bytes,
            error: error.clone(),
        };
        assert!(
            matches!(probe(&mut source, size, "own.flac", &Limits::default(), 0), Err(Failure::Library(found)) if found == error)
        );
    }
}
