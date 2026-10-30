//! `Movie::chapter_tracks`: the `chap` track references (QuickTime File
//! Format, "Chapter lists"; ISO/IEC 14496-12 8.3.3).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

#[path = "common/text_movie.rs"]
mod text_movie;

use coreav_isobmff::{
    Allowlist, Codec, Error, Limits, MAX_CHAPTER_REFERENCES, SliceSource, TrackKind, probe,
};
use text_movie::{TrackSpec, movie, text_sample};

fn chapters() -> TrackSpec {
    TrackSpec::qt_text(
        2,
        vec![
            (text_sample(b"One", &[]), 500),
            (text_sample(b"Two", &[]), 500),
        ],
    )
}

fn video_with(chap: Vec<u32>) -> TrackSpec {
    TrackSpec {
        chap,
        ..TrackSpec::video(1, &[500, 500])
    }
}

fn probe_with(data: &[u8], allowlist: &Allowlist) -> coreav_isobmff::Movie {
    probe(SliceSource::new(data), &Limits::default(), allowlist).unwrap()
}

#[test]
fn references_are_listed_and_quicktime_text_is_dropped_by_default() {
    let data = movie(&[video_with(vec![2]), chapters()]);
    let movie = probe_with(&data, &Allowlist::default());
    assert_eq!(movie.chapter_tracks, vec![2]);
    assert_eq!(movie.tracks.len(), 1);
    assert_eq!(movie.dropped.len(), 1);
    assert_eq!(movie.dropped[0].fourcc.0, *b"text");
    // Allowed on request.
    let movie = probe_with(&data, &Allowlist::new(&[Codec::QuickTimeText]));
    assert_eq!(movie.tracks.len(), 1);
    let text = &movie.tracks[0];
    assert_eq!(text.track_id, 2);
    assert_eq!(text.stream_index, 1);
    assert_eq!(text.kind, TrackKind::Subtitle);
    assert_eq!(text.description.codec, Codec::QuickTimeText);
    // The video track was dropped; its reference is still read.
    assert_eq!(movie.chapter_tracks, vec![2]);
}

#[test]
fn references_from_a_dropped_track_after_its_media() {
    let video = TrackSpec {
        tkhd_last: true,
        ..video_with(vec![2])
    };
    let data = movie(&[video, chapters()]);
    let movie = probe_with(&data, &Allowlist::new(&[Codec::QuickTimeText]));
    assert_eq!(movie.chapter_tracks, vec![2]);
    assert_eq!(movie.dropped[0].track_id, 0);
}

#[test]
fn disabled_tracks_and_zero_ids_do_not_count() {
    let video = TrackSpec {
        enabled: false,
        ..video_with(vec![2])
    };
    let data = movie(&[video, chapters()]);
    assert!(
        probe_with(&data, &Allowlist::default())
            .chapter_tracks
            .is_empty()
    );
    let data = movie(&[video_with(vec![0, 2, 2, 7]), chapters()]);
    assert_eq!(
        probe_with(&data, &Allowlist::default()).chapter_tracks,
        vec![2, 7]
    );
}

#[test]
fn every_reference_counts_for_what_ffprobe_lists_as_data() {
    // `chapter_references` keeps a disabled track's reference, which
    // `chapter_tracks` leaves out.
    let video = TrackSpec {
        enabled: false,
        ..video_with(vec![2])
    };
    let parsed = probe_with(&movie(&[video, chapters()]), &Allowlist::default());
    assert!(parsed.chapter_tracks.is_empty());
    assert_eq!(parsed.chapter_references, vec![2]);
    let data = movie(&[video_with(vec![0, 2, 2, 7]), chapters()]);
    assert_eq!(
        probe_with(&data, &Allowlist::default()).chapter_references,
        vec![2, 7]
    );
}

#[test]
fn several_referrers_keep_file_order_without_repeats() {
    let audio = TrackSpec {
        id: 3,
        chap: vec![4, 2],
        ..TrackSpec::video(3, &[500])
    };
    let data = movie(&[video_with(vec![2]), chapters(), audio]);
    assert_eq!(
        probe_with(&data, &Allowlist::default()).chapter_tracks,
        vec![2, 4]
    );
}

#[test]
fn references_are_capped() {
    let ids: Vec<u32> = (1..=200).collect();
    let data = movie(&[video_with(ids), chapters()]);
    let movie = probe_with(&data, &Allowlist::default());
    assert_eq!(movie.chapter_tracks.len(), MAX_CHAPTER_REFERENCES);
    assert_eq!(movie.chapter_tracks[0], 1);
}

#[test]
fn large_or_malformed_track_references_are_ignored() {
    // Over 4 KiB.
    let ids: Vec<u32> = (1..=1100).collect();
    let data = movie(&[video_with(ids), chapters()]);
    assert!(
        probe_with(&data, &Allowlist::default())
            .chapter_tracks
            .is_empty()
    );
    // A reference type box that claims more than the `tref` holds.
    let mut data = movie(&[video_with(vec![2]), chapters()]);
    let at = data.windows(4).position(|w| w == b"chap").unwrap() - 4;
    data[at..at + 4].copy_from_slice(&64u32.to_be_bytes());
    assert!(
        probe_with(&data, &Allowlist::default())
            .chapter_tracks
            .is_empty()
    );
    // A type box smaller than its header.
    data[at..at + 4].copy_from_slice(&4u32.to_be_bytes());
    assert!(
        probe_with(&data, &Allowlist::default())
            .chapter_tracks
            .is_empty()
    );
}

#[test]
fn a_defect_after_the_media_of_a_dropped_track_still_drops_it() {
    // A child box smaller than its header after `mdia` (and `tref`) in the
    // dropped QuickTime text track: probe drops the track as before.
    let mut data = movie(&[video_with(vec![2]), chapters()]);
    let traks: Vec<usize> = data
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"trak")
        .map(|(i, _)| i - 4)
        .collect();
    let at = traks[1];
    let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
    data.splice(at + size..at + size, [0, 0, 0, 4, b'j', b'u', b'n', b'k']);
    for container in [b"moov", b"trak"] {
        let pos = if container == b"trak" {
            at
        } else {
            data.windows(4).position(|w| w == b"moov").unwrap() - 4
        };
        let old = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
        data[pos..pos + 4].copy_from_slice(&(old + 8).to_be_bytes());
    }
    let movie = probe_with(&data, &Allowlist::default());
    assert_eq!(movie.dropped.len(), 1);
    assert_eq!(movie.chapter_tracks, vec![2]);
    // Allowed, the same track is parsed and the defect rejects the file.
    let result = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::new(&[Codec::QuickTimeText]),
    );
    assert!(result.is_err());
}

#[test]
fn strict_mode_still_rejects_the_dropped_stream() {
    let data = movie(&[video_with(vec![2]), chapters()]);
    let result = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default().strict(),
    );
    assert!(matches!(
        result,
        Err(Error::CodecNotAllowed { track_id: 2, .. })
    ));
}

/// A source whose `fail_at`-th read fails with an I/O error.
struct FailingAt<'a> {
    data: SliceSource<'a>,
    reads: usize,
    fail_at: usize,
}

impl coreav_isobmff::Source for FailingAt<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.reads += 1;
        if self.reads == self.fail_at {
            return Err(Error::Io(std::io::ErrorKind::ConnectionReset));
        }
        self.data.read_exact_at(offset, buf)
    }
}

/// A read that fails is not a damaged box: a failed read of a track
/// reference box, or of a box after a dropped stream's media, fails the
/// probe with the I/O error instead of leaving the reference out.
#[test]
fn a_read_failure_in_a_track_fails_the_probe() {
    let video = TrackSpec {
        tkhd_last: true,
        ..video_with(vec![2])
    };
    let data = movie(&[video, chapters()]);
    // By default the text track is dropped; with text allowed, the video
    // track is, and its `tkhd` and `tref` are read after its media.
    for allowlist in [
        Allowlist::default(),
        Allowlist::new(&[Codec::QuickTimeText]),
    ] {
        let mut clean = FailingAt {
            data: SliceSource::new(&data),
            reads: 0,
            fail_at: 0,
        };
        probe(&mut clean, &Limits::default(), &allowlist).unwrap();
        for fail_at in 1..=clean.reads {
            let mut source = FailingAt {
                data: SliceSource::new(&data),
                reads: 0,
                fail_at,
            };
            let result = probe(&mut source, &Limits::default(), &allowlist);
            assert_eq!(
                result.err(),
                Some(Error::Io(std::io::ErrorKind::ConnectionReset)),
                "read {fail_at} of {}",
                clean.reads
            );
        }
    }
}
