//! The native backend against FFmpeg, on fixtures an `ffmpeg` binary
//! generates: every frame of a stream with B-frames, the frames the
//! caller asks for and the ones it discards, the keyframes-only mode,
//! and access units the caller skipped before the one it wants (what
//! `coreav-thumbnail` does for `--exact`, `docs/design/decode.md`).
//!
//! Needs `ffmpeg` on `PATH` (with libx264), so it is ignored by default;
//! CI runs it in the fixtures workflow.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use coreav_backend_h264::{NativeH264, NativeOptions};
use coreav_decode::{
    Backend, BackendKind, Codec, CodecConfig, ConfigKind, Decoder, Limits, Packet, Policy,
    StreamConfig, Support,
};
use coreav_isobmff::{Allowlist, Budget, FileSource, Source, probe};

fn ffmpeg() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|d| d.join("ffmpeg"))
        .find(|p| p.is_file())
        .expect("ffmpeg on PATH")
}

fn generate(dir: &Path, name: &str, args: &[&str]) -> PathBuf {
    let out = dir.join(name);
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success());
    out
}

/// Every frame of `file`, as FFmpeg decodes it.
fn reference(file: &Path) -> Vec<u8> {
    let out = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ignore_editlist",
            "1",
            "-i",
        ])
        .arg(file)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .expect("ffmpeg runs");
    assert!(out.status.success());
    out.stdout
}

/// One access unit of the file: its bytes and what the container says.
#[derive(Clone)]
struct Au {
    data: Vec<u8>,
    pts: i64,
    dts: i64,
    keyframe: bool,
}

/// The stream and its access units, read with `coreav-isobmff`.
fn read(file: &Path) -> (StreamConfig, Vec<Au>) {
    let mut source = FileSource::open(file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind: ConfigKind::Avcc,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(Codec::H264)
    };
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    let aus = packets
        .iter()
        .map(|p| {
            let mut data = vec![0u8; p.size as usize];
            source.read_exact_at(p.offset, &mut data).expect("read");
            Au {
                data,
                pts: p.cts,
                dts: p.dts,
                keyframe: p.keyframe,
            }
        })
        .collect();
    (stream, aus)
}

/// Opens the native backend through the boundary's own selection, so
/// that the test exercises what a caller does.
fn open(stream: &StreamConfig, options: NativeOptions) -> Box<dyn Decoder> {
    let backend = NativeH264::with_options(options);
    let opened = coreav_decode::open(
        stream,
        &Policy::default(),
        &Limits::default(),
        &[&backend as &dyn Backend],
    )
    .expect("the native backend opens the stream");
    assert_eq!(opened.decoder.info().backend, BackendKind::Native);
    opened.decoder
}

/// Decodes `aus`, discarding the ones `discard` picks; each frame's
/// timestamp and its packed samples.
fn decode(
    stream: &StreamConfig,
    aus: &[Au],
    options: NativeOptions,
    discard: impl Fn(usize) -> bool,
) -> Vec<(Option<i64>, Vec<u8>)> {
    let mut decoder = open(stream, options);
    let mut frames = Vec::new();
    fn take(decoder: &mut Box<dyn Decoder>, frames: &mut Vec<(Option<i64>, Vec<u8>)>) {
        while let Some(frame) = decoder.receive_frame().expect("receive") {
            let mut packed = Vec::new();
            frame
                .to_cpu()
                .expect("cpu")
                .write_packed(&mut packed)
                .expect("packed");
            frames.push((frame.pts, packed));
        }
    }
    for (index, au) in aus.iter().enumerate() {
        decoder
            .send_packet(&Packet {
                data: &au.data,
                pts: Some(au.pts),
                dts: Some(au.dts),
                keyframe: au.keyframe,
                discard: discard(index),
            })
            .expect("send");
        take(&mut decoder, &mut frames);
    }
    decoder.send_eof().expect("eof");
    take(&mut decoder, &mut frames);
    frames
}

/// A fixture with B-frames and a keyframe every twelve pictures.
fn fixture(dir: &Path) -> PathBuf {
    generate(
        dir,
        "b.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=176x144:rate=25",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-bf",
            "3",
            "-g",
            "12",
            "-pix_fmt",
            "yuv420p",
        ],
    )
}

#[test]
#[ignore = "needs ffmpeg"]
fn every_frame_of_a_stream_with_b_frames_matches_ffmpeg() {
    let dir = std::env::temp_dir().join(format!("coreav-native-h264-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = fixture(&dir);
    let (stream, aus) = read(&file);
    let full = decode(&stream, &aus, NativeOptions::default(), |_| false);

    // Every sample of every frame, in display order.
    let ours: Vec<u8> = full.iter().flat_map(|(_, f)| f.clone()).collect();
    let theirs = reference(&file);
    assert_eq!(ours.len(), theirs.len());
    assert!(ours == theirs, "decoded samples differ from FFmpeg's");

    // Display order, each frame's timestamp its packet's.
    let pts: Vec<i64> = full.iter().map(|(p, _)| p.expect("pts")).collect();
    let mut sorted = pts.clone();
    sorted.sort_unstable();
    assert_eq!(pts, sorted, "frames come out in presentation order");
    let mut packet_pts: Vec<i64> = aus.iter().map(|au| au.pts).collect();
    packet_pts.sort_unstable();
    assert_eq!(pts, packet_pts, "every packet's frame, once");

    // The same frames whatever the thread count: the default above is the
    // host's cores.
    for threads in [1, 3] {
        let options = NativeOptions {
            threads: Some(threads),
            ..NativeOptions::default()
        };
        assert!(
            decode(&stream, &aus, options, |_| false) == full,
            "{threads} threads give other frames"
        );
    }

    // A discarded packet is decoded but need not come out; the frames
    // that do are the same samples as before.
    let target = aus.len() - 1;
    let target_pts = aus[target].pts;
    let some = decode(&stream, &aus, NativeOptions::default(), |i| {
        i != target && aus[i].pts != target_pts
    });
    assert!(
        some.len() < full.len(),
        "discarded frames are not returned: {} of {}",
        some.len(),
        full.len()
    );
    for (pts, samples) in &some {
        let same = full
            .iter()
            .find(|(p, _)| p == pts)
            .expect("a frame the full run also returned");
        assert!(&same.1 == samples, "a frame of the partial run differs");
    }
    assert!(
        some.iter().any(|(p, _)| *p == Some(target_pts)),
        "the frame the caller asked for"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Frames as a test compares them: each one's timestamp and its packed
/// samples.
type Frames = Vec<(Option<i64>, Vec<u8>)>;

/// Decodes `aus` with `threads`, for a caller that carries on after an
/// error (`stop` false) or stops at the first (`stop` true): the frames it
/// receives, and how many calls failed.
fn decode_broken(stream: &StreamConfig, aus: &[Au], threads: usize, stop: bool) -> (Frames, usize) {
    let mut decoder = open(
        stream,
        NativeOptions {
            threads: Some(threads),
            ..NativeOptions::default()
        },
    );
    let mut frames = Vec::new();
    let mut failed = 0;
    let take = |decoder: &mut Box<dyn Decoder>, frames: &mut Frames| {
        while let Ok(Some(frame)) = decoder.receive_frame() {
            let mut packed = Vec::new();
            frame
                .to_cpu()
                .expect("cpu")
                .write_packed(&mut packed)
                .expect("packed");
            frames.push((frame.pts, packed));
        }
    };
    for au in aus {
        let sent = decoder.send_packet(&Packet {
            data: &au.data,
            pts: Some(au.pts),
            dts: Some(au.dts),
            keyframe: au.keyframe,
            discard: false,
        });
        if sent.is_err() {
            failed += 1;
            if stop {
                return (frames, failed);
            }
        }
        take(&mut decoder, &mut frames);
    }
    if decoder.send_eof().is_err() {
        failed += 1;
        if stop {
            return (frames, failed);
        }
    }
    take(&mut decoder, &mut frames);
    (frames, failed)
}

#[test]
#[ignore = "needs ffmpeg"]
fn a_broken_stream_gives_the_same_frames_whatever_the_thread_count() {
    // With threads a slice's error is reported by a later call than with
    // one. A caller that carries on after errors still receives the same
    // frames; one that stops at the first error receives a prefix of what
    // one thread gives it.
    let dir = std::env::temp_dir().join(format!("coreav-native-broken-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    // Four slices a picture: a packet of several NAL units is where an
    // error reported by an earlier NAL unit than the one that failed
    // could leave the rest of the packet out.
    let file = generate(
        &dir,
        "slices.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=352x288:rate=25",
            "-t",
            "3",
            "-c:v",
            "libx264",
            "-bf",
            "3",
            "-g",
            "24",
            "-x264-params",
            "slices=4",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let (stream, clean) = read(&file);
    let mut noticed = 0;
    for seed in 1..=6u64 {
        let mut aus = clean.clone();
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for au in aus.iter_mut().skip(1) {
            if next() % 4 == 0 && au.data.len() > 16 {
                // Past the length prefix and the slice header's start.
                let at = 12 + (next() as usize) % (au.data.len() - 12);
                au.data[at] ^= 1 << (next() % 8);
            }
        }
        let (want, failed) = decode_broken(&stream, &aus, 1, false);
        noticed += failed;
        for threads in [2, 3] {
            let (got, _) = decode_broken(&stream, &aus, threads, false);
            assert!(got == want, "seed {seed}, {threads} threads: other frames");
        }
        let (first, _) = decode_broken(&stream, &aus, 1, true);
        for threads in [2, 3] {
            let (got, _) = decode_broken(&stream, &aus, threads, true);
            assert!(
                first.starts_with(&got),
                "seed {seed}, {threads} threads: {} frames, not a prefix of one thread's {}",
                got.len(),
                first.len()
            );
        }
    }
    assert!(
        noticed > 0,
        "no corruption was noticed: the test tests nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg"]
fn the_keyframe_mode_returns_the_keyframes_of_the_full_run() {
    let dir = std::env::temp_dir().join(format!("coreav-native-key-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = fixture(&dir);
    let (stream, aus) = read(&file);
    let full = decode(&stream, &aus, NativeOptions::default(), |_| false);
    let keys = decode(
        &stream,
        &aus,
        NativeOptions {
            keyframes_only: true,
            ..NativeOptions::default()
        },
        |_| false,
    );
    assert!(!keys.is_empty(), "the stream has keyframes");
    assert!(keys.len() < full.len(), "and not every picture is one");
    // A subsequence of the full run, sample for sample.
    let mut rest = full.iter();
    for key in &keys {
        let same = rest
            .find(|(p, _)| p == &key.0)
            .expect("in the full run, in order");
        assert!(same.1 == key.1, "a keyframe differs from the full decode");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg"]
fn skipping_the_non_reference_access_units_leaves_the_later_frames_alone() {
    let dir = std::env::temp_dir().join(format!("coreav-native-skip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = fixture(&dir);
    let (stream, aus) = read(&file);
    let full = decode(&stream, &aus, NativeOptions::default(), |_| false);

    // What the caller may skip (`docs/design/decode.md`): an access unit
    // whose every slice NAL has nal_ref_idc = 0 and whose other NAL
    // units are SEI, access unit delimiters or filler.
    let length_size = length_size(&stream);
    let kept: Vec<Au> = aus
        .iter()
        .filter(|au| !skippable(&au.data, length_size))
        .cloned()
        .collect();
    assert!(
        kept.len() < aus.len(),
        "the stream has non-reference pictures"
    );
    let after = decode(&stream, &kept, NativeOptions::default(), |_| false);
    for (pts, samples) in &after {
        let same = full
            .iter()
            .find(|(p, _)| p == pts)
            .expect("a frame the full run also returned");
        assert!(
            &same.1 == samples,
            "a frame decoded without the non-reference access units differs"
        );
    }
    assert_eq!(after.len(), kept.len(), "one frame per access unit kept");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg"]
fn a_container_that_trims_the_padding_is_followed() {
    let dir = std::env::temp_dir().join(format!("coreav-native-trim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    // A stream whose parameter set crops nothing: 864 and 480 are whole
    // macroblocks, so the picture the decoder makes is 864 wide and only
    // the container can say that ten of those columns are padding — as
    // an MP4 written by a camera that codes 864 and shows 854 does, and
    // as FFmpeg's demuxer reads it.
    let file = generate(
        &dir,
        "padded.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=864x480:rate=25",
            "-t",
            "1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let (coded, aus) = read(&file);
    assert_eq!((coded.width, coded.height), (Some(864), Some(480)));
    let whole = decode(&coded, &aus, NativeOptions::default(), |_| false);
    let (_, first) = whole.first().expect("a frame");
    assert_eq!(first.len(), 864 * 480 * 3 / 2, "the coded picture");

    // The same stream, with the container declaring the smaller size.
    let trimmed = StreamConfig {
        width: Some(854),
        height: Some(480),
        ..coded.clone()
    };
    let frames = decode(&trimmed, &aus, NativeOptions::default(), |_| false);
    let (_, first) = frames.first().expect("a frame");
    assert_eq!(first.len(), 854 * 480 * 3 / 2, "what the container shows");
    // And the samples are the coded picture's, less the padding.
    let mut want = Vec::new();
    for (plane, (w, h)) in [
        (0usize, (864usize, 480usize)),
        (1, (432, 240)),
        (2, (432, 240)),
    ] {
        let start: usize = [0, 864 * 480, 864 * 480 + 432 * 240][plane];
        let shown = if plane == 0 { 854 } else { 427 };
        for y in 0..h {
            want.extend_from_slice(&whole[0].1[start + y * w..start + y * w + shown]);
        }
    }
    assert!(
        frames[0].1 == want,
        "the trimmed samples are the coded ones"
    );

    // A stream a container describes as the bitstream does is untouched.
    let mut same = coded.clone();
    same.width = None;
    same.height = None;
    let frames = decode(&same, &aus, NativeOptions::default(), |_| false);
    assert_eq!(frames[0].1.len(), 864 * 480 * 3 / 2);

    // And the same file with the declaration written into it, which is
    // what a camera writes and what FFmpeg reads: the two decoders must
    // agree on a real file, not only on a StreamConfig a test built.
    let patched = dir.join("declared.mp4");
    let mut bytes = std::fs::read(&file).expect("read");
    let at = find_visual_width(&bytes).expect("a sample entry");
    bytes[at] = 854u16.to_be_bytes()[0];
    bytes[at + 1] = 854u16.to_be_bytes()[1];
    std::fs::write(&patched, &bytes).expect("write");
    let (declared, aus) = read(&patched);
    assert_eq!(declared.width, Some(854));
    let ours: Vec<u8> = decode(&declared, &aus, NativeOptions::default(), |_| false)
        .iter()
        .flat_map(|(_, f)| f.clone())
        .collect();
    let theirs = reference(&patched);
    assert_eq!(ours.len(), theirs.len(), "the size FFmpeg decodes to");
    assert!(ours == theirs, "the samples FFmpeg decodes");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The offset of the `width` field of the `avc1` sample entry of the
/// sample description: its four-character code, then six reserved bytes,
/// the data reference index, and sixteen bytes of pre-defined and
/// reserved fields (ISO/IEC 14496-12, `VisualSampleEntry`).
///
/// The search starts at `stsd`, because `avc1` is also one of the
/// brands a file type box lists.
fn find_visual_width(data: &[u8]) -> Option<usize> {
    let stsd = data.windows(4).position(|w| w == b"stsd")?;
    let entry = data
        .get(stsd..)?
        .windows(4)
        .position(|w| w == b"avc1")
        .map(|at| stsd + at)?;
    Some(entry + 4 + 6 + 2 + 16)
}

#[test]
#[ignore = "needs ffmpeg"]
fn an_interlaced_stream_is_left_to_another_backend_before_a_packet() {
    let dir = std::env::temp_dir().join(format!("coreav-native-ilme-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    // Macroblock-adaptive frame/field coding: Main profile, 8-bit,
    // 4:2:0, so nothing the profile or the format says tells it apart —
    // only the sequence parameter set does, and the decoder would
    // otherwise find out at the first slice, after selection had
    // committed to it.
    let file = generate(
        &dir,
        "mbaff.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=352x288:rate=50",
            "-t",
            "1",
            "-vf",
            "interlace",
            "-c:v",
            "libx264",
            "-flags",
            "+ilme+ildct",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let (stream, _) = read(&file);
    let backend = NativeH264::new();
    let support = backend.supports(&stream, &stream.traits());
    assert!(
        matches!(&support, Support::No(reason) if reason.contains("frame/field")),
        "{support:?}"
    );
    // And opening it refuses too, for a caller that did not ask.
    let error = backend
        .open(&stream, &Limits::default())
        .err()
        .expect("no decoder");
    assert_eq!(error.kind, coreav_decode::ErrorKind::Unsupported);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg"]
fn a_stream_over_the_limits_is_refused_before_a_packet() {
    let dir = std::env::temp_dir().join(format!("coreav-native-limits-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = fixture(&dir);
    let (stream, aus) = read(&file);
    let backend = NativeH264::new();
    // A stream that keeps more reference pictures than the caller
    // allows, and one whose picture is larger: both are in the sequence
    // parameter set, so `open` answers them rather than the first
    // packet, and `coreav_decode::open` can try the next backend.
    for limits in [
        Limits {
            max_reference_frames: 1,
            ..Limits::default()
        },
        Limits {
            max_width: 64,
            max_height: 64,
            max_pixels: 64 * 64,
            ..Limits::default()
        },
    ] {
        let error = backend
            .open(&stream, &limits)
            .err()
            .expect("a stream over the limits");
        assert_eq!(error.kind, coreav_decode::ErrorKind::Limit, "{error}");
    }
    // The same stream opens and decodes under the defaults.
    let mut decoder = backend
        .open(&stream, &Limits::default())
        .expect("the defaults");
    let first = aus.first().expect("a packet");
    decoder
        .send_packet(&Packet {
            data: &first.data,
            pts: Some(first.pts),
            dts: Some(first.dts),
            keyframe: first.keyframe,
            discard: false,
        })
        .expect("send");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg"]
fn a_stream_outside_the_decoder_is_left_to_another_backend() {
    let dir = std::env::temp_dir().join(format!("coreav-native-high10-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    // High 10 (profile_idc 110), which this decoder does not decode.
    let file = generate(
        &dir,
        "high10.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x96:rate=25",
            "-t",
            "1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p10le",
        ],
    );
    let (stream, _) = read(&file);
    let backend = NativeH264::new();
    let support = backend.supports(&stream, &stream.traits());
    assert!(
        matches!(support, Support::No(_)),
        "High 10 is left to another backend: {support:?}"
    );
    // Selection says so rather than opening a decoder that then fails.
    let error = coreav_decode::open(
        &stream,
        &Policy::default(),
        &Limits::default(),
        &[&backend as &dyn Backend],
    )
    .err()
    .expect("no decoder");
    assert_eq!(error.kind, coreav_decode::ErrorKind::Unsupported);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The NAL unit length size of the stream's `avcC`.
fn length_size(stream: &StreamConfig) -> usize {
    let config = stream.config.as_ref().expect("an avcC");
    usize::from(config.data[4] & 3) + 1
}

/// Whether the caller may skip this access unit before the frame it
/// wants: every slice is non-reference, and nothing else carries state.
fn skippable(data: &[u8], length_size: usize) -> bool {
    let mut at = 0usize;
    let mut slices = 0;
    while at + length_size <= data.len() {
        let mut length = 0usize;
        for byte in &data[at..at + length_size] {
            length = (length << 8) | usize::from(*byte);
        }
        at += length_size;
        let Some(unit) = data.get(at..at + length) else {
            return false;
        };
        at += length;
        let Some(header) = unit.first() else {
            return false;
        };
        let kind = header & 0x1f;
        let nal_ref_idc = (header >> 5) & 3;
        match kind {
            // A coded slice of a non-IDR picture.
            1 => {
                if nal_ref_idc != 0 {
                    return false;
                }
                slices += 1;
            }
            // SEI, access unit delimiter, filler.
            6 | 9 | 12 => {}
            // Anything else (parameter sets, IDR slices) is never skipped.
            _ => return false,
        }
    }
    slices > 0
}
