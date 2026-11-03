//! Native standalone audio routing, failure propagation and pre-output refusals.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use coreav_cli::error::Failure;
use coreav_cli::ffprobe::{self, args};
use coreav_isobmff::{Error, LimitKind, Limits, SliceSource, Source};

#[path = "common/standalone_audio.rs"]
mod standalone_audio;
use standalone_audio::{flac, wave};

fn assert_limit(bytes: &[u8], limits: Limits, expected: LimitKind) {
    let mut source = SliceSource::new(bytes);
    match ffprobe::probe_limited(&mut source, bytes.len() as u64, "audio", None, &limits) {
        Err(Failure::Library(Error::LimitExceeded { limit, .. })) => assert_eq!(limit, expected),
        Err(other) => panic!("unexpected error: {}", other.to_json()),
        Ok(_) => panic!("probe ignored the caller's limit"),
    }
}

#[test]
fn native_audio_obeys_the_callers_stream_and_chunk_limits() {
    for bytes in [wave(), flac()] {
        assert_limit(
            &bytes,
            Limits {
                max_tracks: 0,
                ..Limits::default()
            },
            LimitKind::Tracks,
        );
        assert_limit(
            &bytes,
            Limits {
                max_total_boxes: 0,
                ..Limits::default()
            },
            LimitKind::TotalBoxes,
        );
    }
}

#[test]
fn native_audio_refuses_listing_before_writing_any_output() {
    for bytes in [wave(), flac()] {
        for operation in [
            "-show_packets",
            "-show_frames",
            "-count_packets",
            "-count_frames",
        ] {
            let mut source = SliceSource::new(&bytes);
            let mut probed = ffprobe::probe(&mut source, bytes.len() as u64, "audio", None)
                .unwrap_or_else(|e| panic!("{}", e.to_json()));
            let options = args::parse(&[
                operation.to_owned(),
                "-of".to_owned(),
                "json".to_owned(),
                "audio".to_owned(),
            ])
            .unwrap();
            let mut output = Vec::new();
            assert!(matches!(
                ffprobe::emit(&options, &mut probed, &mut output),
                Err(Failure::Unsupported(_))
            ));
            assert!(output.is_empty());
        }
    }
}

#[test]
fn forced_audio_formats_do_not_silently_accept_another_container() {
    for (bytes, force) in [
        (wave(), args::Container::Flac),
        (flac(), args::Container::Wav),
    ] {
        let mut source = SliceSource::new(&bytes);
        assert!(ffprobe::probe(&mut source, bytes.len() as u64, "audio", Some(force)).is_err());
    }
}

struct BrokenSource;

impl Source for BrokenSource {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(1024)
    }
    fn read_exact_at(&mut self, _offset: u64, _buf: &mut [u8]) -> Result<(), Error> {
        Err(Error::LimitExceeded {
            limit: LimitKind::ReadBytes,
            offset: 0,
        })
    }
}

#[test]
fn signature_read_limits_fail_instead_of_becoming_a_fallback() {
    let mut source = BrokenSource;
    assert!(matches!(
        ffprobe::probe(&mut source, 1024, "audio", None),
        Err(Failure::Library(Error::LimitExceeded {
            limit: LimitKind::ReadBytes,
            ..
        }))
    ));
}

#[test]
fn malformed_recognized_headers_are_not_unsupported_formats() {
    for bytes in [b"RIFF\x04\0\0\0WAVE".to_vec(), b"fLaC\x80\0\0\x22".to_vec()] {
        let mut source = SliceSource::new(&bytes);
        let result = ffprobe::probe(&mut source, bytes.len() as u64, "audio", None);
        assert!(result.is_err());
        assert!(!matches!(result, Err(Failure::Unsupported(_))));
    }
}

#[test]
fn empty_id3_prefixes_and_matching_footers_keep_flac_native() {
    for prefix in [
        b"ID3\x03\0\0\0\0\0\0".as_slice(),
        b"ID3\x04\0\0\0\0\0\0".as_slice(),
        b"ID3\x04\0\x10\0\0\0\0\x33DI\x04\0\x10\0\0\0\0".as_slice(),
    ] {
        let mut bytes = prefix.to_vec();
        bytes.extend(flac());
        let mut source = SliceSource::new(&bytes);
        assert!(ffprobe::probe(&mut source, bytes.len() as u64, "audio", None).is_ok());
    }
    let mut bytes = b"ID3\x04\0\x10\0\0\0\0bad\x04\0\x10\0\0\0\0".to_vec();
    bytes.extend(flac());
    let mut source = SliceSource::new(&bytes);
    assert!(matches!(
        ffprobe::probe(&mut source, bytes.len() as u64, "audio", None),
        Err(Failure::Invalid(_))
    ));
}

#[test]
fn nonempty_id3_metadata_is_refused_instead_of_losing_tags() {
    let mut bytes = b"ID3\x04\0\0\0\0\0\x01\0".to_vec();
    bytes.extend(flac());
    let mut source = SliceSource::new(&bytes);
    assert!(matches!(
        ffprobe::probe(&mut source, bytes.len() as u64, "audio", None),
        Err(Failure::Unsupported(_))
    ));
}
