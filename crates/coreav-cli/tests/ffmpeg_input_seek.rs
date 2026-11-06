//! Input-seek copies retain exact packet timing or refuse before publication.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn snapshot(path: &Path, ffprobe: &Path) -> Value {
    let result = Command::new(ffprobe)
        .args(["-v", "error", "-show_streams", "-show_packets", "-show_frames", "-show_format", "-show_data_hash", "sha256", "-show_entries",
            "stream:stream_tags=language,handler_name:packet=stream_index,pts,dts,duration,size,flags,data_hash:frame=stream_index,best_effort_timestamp,pkt_dts,duration:format=start_time,duration:format_tags=", "-of", "json"])
        .arg(path).output().unwrap();
    assert!(
        result.status.success() && result.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

fn pixels(path: &Path, ffmpeg: &Path) -> Vec<u8> {
    let result = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error"])
        .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
        .arg("-i")
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(result.status.success() && result.stderr.is_empty());
    assert!(!result.stdout.is_empty());
    result.stdout
}

fn pcm(path: &Path, ffmpeg: &Path) -> Vec<u8> {
    let result = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error"])
        .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
        .arg("-i")
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-c:a",
            "pcm_f32le",
            "-f",
            "f32le",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(result.status.success() && result.stderr.is_empty());
    assert!(!result.stdout.is_empty());
    result.stdout
}

mod iso {
    #[derive(Clone, Copy)]
    struct BoxSpan {
        kind: [u8; 4],
        body: usize,
        end: usize,
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn u64_at(bytes: &[u8], at: usize) -> u64 {
        u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    fn children(bytes: &[u8], start: usize, end: usize) -> Vec<BoxSpan> {
        let mut boxes = Vec::new();
        let mut at = start;
        while at < end {
            assert!(at + 8 <= end);
            let kind = bytes[at + 4..at + 8].try_into().unwrap();
            let (header, size) = match u32_at(bytes, at) {
                0 => (8, end - at),
                1 => (16, usize::try_from(u64_at(bytes, at + 8)).unwrap()),
                n => (8, n as usize),
            };
            assert!(size >= header && size <= end - at);
            boxes.push(BoxSpan {
                kind,
                body: at + header,
                end: at + size,
            });
            at += size;
        }
        boxes
    }

    fn child(bytes: &[u8], parent: BoxSpan, kind: &[u8; 4]) -> BoxSpan {
        let found: Vec<_> = children(bytes, parent.body, parent.end)
            .into_iter()
            .filter(|b| &b.kind == kind)
            .collect();
        assert_eq!(found.len(), 1, "one {:?} box", kind);
        found[0]
    }

    fn tracks(bytes: &[u8]) -> Vec<BoxSpan> {
        let root = BoxSpan {
            kind: *b"root",
            body: 0,
            end: bytes.len(),
        };
        let moov = child(bytes, root, b"moov");
        children(bytes, moov.body, moov.end)
            .into_iter()
            .filter(|b| &b.kind == b"trak")
            .collect()
    }

    fn stbl(bytes: &[u8], track: BoxSpan) -> BoxSpan {
        let mdia = child(bytes, track, b"mdia");
        child(bytes, child(bytes, mdia, b"minf"), b"stbl")
    }

    fn edit(bytes: &[u8], track: BoxSpan) -> BoxSpan {
        child(bytes, child(bytes, track, b"edts"), b"elst")
    }

    fn clock_duration(bytes: &[u8], header: BoxSpan) -> (u32, u64) {
        match bytes[header.body] {
            0 => (
                u32_at(bytes, header.body + 12),
                u64::from(u32_at(bytes, header.body + 16)),
            ),
            1 => (
                u32_at(bytes, header.body + 20),
                u64_at(bytes, header.body + 24),
            ),
            _ => panic!("time header version"),
        }
    }

    fn runs(bytes: &[u8], table: BoxSpan, samples: usize, signed: bool) -> Vec<i64> {
        let count = u32_at(bytes, table.body + 4) as usize;
        assert_eq!(table.end - table.body, 8 + count * 8);
        let mut result = Vec::new();
        for index in 0..count {
            let at = table.body + 8 + index * 8;
            let n = u32_at(bytes, at) as usize;
            assert!(n > 0 && n <= samples - result.len());
            let value = if signed {
                i64::from(u32_at(bytes, at + 4) as i32)
            } else {
                i64::from(u32_at(bytes, at + 4))
            };
            result.extend(std::iter::repeat_n(value, n));
        }
        assert_eq!(result.len(), samples);
        result
    }

    pub(super) fn validate(bytes: &[u8]) {
        let root = BoxSpan {
            kind: *b"root",
            body: 0,
            end: bytes.len(),
        };
        let moov = child(bytes, root, b"moov");
        let mdat = child(bytes, root, b"mdat");
        let (movie_clock, movie_duration) = clock_duration(bytes, child(bytes, moov, b"mvhd"));
        assert!(movie_clock > 0);
        let tracks = tracks(bytes);
        assert_eq!(tracks.len(), 2);
        let mut movie_end = 0;
        let mut extents = Vec::new();
        for (index, track) in tracks.into_iter().enumerate() {
            let mdia = child(bytes, track, b"mdia");
            let (clock, media_duration) = clock_duration(bytes, child(bytes, mdia, b"mdhd"));
            assert!(clock > 0);
            let table = stbl(bytes, track);
            let sizes = child(bytes, table, b"stsz");
            let constant = u32_at(bytes, sizes.body + 4);
            let n = u32_at(bytes, sizes.body + 8) as usize;
            assert!(n > 0 && n <= bytes.len());
            assert_eq!(
                sizes.end - sizes.body,
                12 + if constant == 0 { n * 4 } else { 0 }
            );
            let sizes: Vec<_> = (0..n)
                .map(|i| {
                    if constant != 0 {
                        constant
                    } else {
                        u32_at(bytes, sizes.body + 12 + i * 4)
                    }
                })
                .collect();
            assert!(sizes.iter().all(|size| *size > 0));
            let durations = runs(bytes, child(bytes, table, b"stts"), n, false);
            assert!(durations.iter().all(|delta| *delta > 0));
            assert_eq!(durations.iter().sum::<i64>() as u64, media_duration);
            let offsets = match children(bytes, table.body, table.end)
                .into_iter()
                .find(|b| &b.kind == b"ctts")
            {
                Some(ctts) => {
                    assert!(bytes[ctts.body] <= 1);
                    runs(bytes, ctts, n, bytes[ctts.body] == 1)
                }
                None => vec![0; n],
            };
            assert!(offsets.iter().all(|offset| *offset >= 0));
            let mut decode = 0;
            let mut composition = std::collections::HashSet::new();
            let mut end = 0;
            for (delta, offset) in durations.iter().zip(offsets) {
                let time = decode + offset;
                assert!(composition.insert(time), "unique composition times");
                end = end.max(time + delta);
                decode += delta;
            }
            let elst = edit(bytes, track);
            assert_eq!(u32_at(bytes, elst.body + 4), 1);
            let (duration, media_time, rate) = match bytes[elst.body] {
                0 => (
                    u64::from(u32_at(bytes, elst.body + 8)),
                    i64::from(u32_at(bytes, elst.body + 12) as i32),
                    u32_at(bytes, elst.body + 16),
                ),
                1 => (
                    u64_at(bytes, elst.body + 8),
                    u64_at(bytes, elst.body + 16) as i64,
                    u32_at(bytes, elst.body + 24),
                ),
                _ => panic!("edit version"),
            };
            assert!(duration > 0 && media_time >= 0);
            assert_eq!(rate, 65_536);
            assert_eq!(
                i128::from(end - media_time) * i128::from(movie_clock),
                i128::from(duration) * i128::from(clock),
                "complete presentation span"
            );
            let tkhd = child(bytes, track, b"tkhd");
            let volume_at = tkhd.body + if bytes[tkhd.body] == 0 { 36 } else { 48 };
            assert_eq!(
                u16::from_be_bytes(bytes[volume_at..volume_at + 2].try_into().unwrap()),
                if index == 0 { 0 } else { 256 }
            );
            let track_duration = if bytes[tkhd.body] == 0 {
                u64::from(u32_at(bytes, tkhd.body + 20))
            } else {
                u64_at(bytes, tkhd.body + 28)
            };
            assert_eq!(track_duration, duration);
            movie_end = movie_end.max(duration);
            let offsets_box = children(bytes, table.body, table.end)
                .into_iter()
                .find(|b| matches!(&b.kind, b"stco" | b"co64"))
                .unwrap();
            let chunks = u32_at(bytes, offsets_box.body + 4) as usize;
            let width = if &offsets_box.kind == b"stco" { 4 } else { 8 };
            assert_eq!(offsets_box.end - offsets_box.body, 8 + width * chunks);
            let stsc = child(bytes, table, b"stsc");
            let entries = u32_at(bytes, stsc.body + 4) as usize;
            assert!(entries > 0 && entries <= chunks);
            assert_eq!(stsc.end - stsc.body, 8 + entries * 12);
            let runs: Vec<_> = (0..entries)
                .map(|i| {
                    let at = stsc.body + 8 + i * 12;
                    assert_eq!(u32_at(bytes, at + 8), 1, "sample description");
                    (u32_at(bytes, at) as usize, u32_at(bytes, at + 4) as usize)
                })
                .collect();
            assert_eq!(runs[0].0, 1);
            assert!(runs.windows(2).all(|w| w[0].0 < w[1].0));
            assert!(
                runs.iter()
                    .all(|(first, count)| *first <= chunks && *count > 0)
            );
            let mut run = 0;
            let mut sample = 0;
            for chunk in 1..=chunks {
                while run + 1 < runs.len() && runs[run + 1].0 <= chunk {
                    run += 1;
                }
                let at = offsets_box.body + 8 + (chunk - 1) * width;
                let mut offset = if width == 4 {
                    u64::from(u32_at(bytes, at))
                } else {
                    u64_at(bytes, at)
                };
                for _ in 0..runs[run].1 {
                    assert!(sample < n);
                    let end = offset + u64::from(sizes[sample]);
                    assert!(
                        offset >= mdat.body as u64 && end <= mdat.end as u64,
                        "sample extent"
                    );
                    extents.push((offset, end));
                    offset = end;
                    sample += 1;
                }
            }
            assert_eq!(sample, n);
            if index == 1 {
                let sgpd = child(bytes, table, b"sgpd");
                assert_eq!(
                    &bytes[sgpd.body..sgpd.end],
                    &[
                        1, 0, 0, 0, b'r', b'o', b'l', b'l', 0, 0, 0, 2, 0, 0, 0, 1, 255, 255
                    ]
                );
                let sbgp = child(bytes, table, b"sbgp");
                assert_eq!(sbgp.end - sbgp.body, 20);
                assert_eq!(
                    &bytes[sbgp.body..sbgp.body + 8],
                    &[0, 0, 0, 0, b'r', b'o', b'l', b'l']
                );
                assert_eq!(u32_at(bytes, sbgp.body + 8), 1);
                assert_eq!(u32_at(bytes, sbgp.body + 12), n as u32, "roll membership");
                assert_eq!(u32_at(bytes, sbgp.body + 16), 1);
            }
        }
        assert_eq!(movie_duration, movie_end);
        extents.sort_unstable();
        assert_eq!(extents.first().unwrap().0, mdat.body as u64);
        assert_eq!(extents.last().unwrap().1, mdat.end as u64);
        assert!(
            extents.windows(2).all(|pair| pair[0].1 == pair[1].0),
            "complete nonoverlapping extents"
        );
    }

    pub(super) fn mutations(bytes: &[u8]) {
        let tracks = tracks(bytes);
        let video = stbl(bytes, tracks[0]);
        let audio = stbl(bytes, tracks[1]);
        let elst = edit(bytes, tracks[0]);
        assert_eq!(bytes[elst.body], 0);
        let ctts = child(bytes, video, b"ctts");
        assert_eq!(u32_at(bytes, ctts.body + 8), 1);
        let delta = u32_at(bytes, child(bytes, video, b"stts").body + 12);
        let duplicate = delta + u32_at(bytes, ctts.body + 20);
        let video_offset = child(bytes, video, b"stco").body + 8;
        for (at, value) in [
            (elst.body + 12, 0),
            (elst.body + 8, u32_at(bytes, elst.body + 8) + 1),
            (ctts.body + 12, duplicate),
            (ctts.body + 12, u32::MAX),
            (
                child(bytes, video, b"stsz").body + 8,
                u32_at(bytes, child(bytes, video, b"stsz").body + 8) + 1,
            ),
            (child(bytes, video, b"stsc").body + 16, 2),
            (
                child(bytes, audio, b"stco").body + 8,
                u32_at(bytes, video_offset),
            ),
            (child(bytes, audio, b"sbgp").body + 12, 1),
        ] {
            let mut changed = bytes.to_vec();
            changed[at..at + 4].copy_from_slice(&value.to_be_bytes());
            assert!(
                std::panic::catch_unwind(|| validate(&changed)).is_err(),
                "mutation at {at}"
            );
        }
    }
}

fn exact_copy(
    ffmpeg: &Path,
    ffprobe: &Path,
    args: &[&str],
    output: &Path,
    reference: &Path,
    existing: bool,
) -> Vec<u8> {
    let oracle = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error"])
        .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
        .args(args)
        .arg(reference)
        .output()
        .unwrap();
    assert!(
        oracle.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    if existing {
        std::fs::write(output, b"old output").unwrap();
    }
    let entries = std::fs::read_dir(output.parent().unwrap()).unwrap().count();
    let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .arg("ffmpeg")
        .args(args)
        .arg(output)
        .env("COREAV_STRICT_SAFE", "1")
        .env("COREAV_FALLBACK", output.with_extension("missing"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty() && result.stderr.is_empty());
    assert_eq!(
        std::fs::read_dir(output.parent().unwrap()).unwrap().count(),
        entries + usize::from(!existing)
    );
    assert_eq!(
        snapshot(output, ffprobe),
        snapshot(reference, ffprobe),
        "{args:?}"
    );
    assert_eq!(
        pixels(output, ffmpeg),
        pixels(reference, ffmpeg),
        "{args:?}"
    );
    assert_eq!(pcm(output, ffmpeg), pcm(reference, ffmpeg), "{args:?}");
    let bytes = std::fs::read(output).unwrap();
    iso::validate(&bytes);
    iso::validate(&std::fs::read(reference).unwrap());
    bytes
}

#[test]
fn closed_gop_input_seek_witnesses_match_complete_outputs() {
    let pair = test_oracle::require();
    let ffmpeg = pair.ffmpeg();
    let owner = FixtureDir(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("ffmpeg-input-seek-c18-{}", std::process::id())),
    );
    std::fs::create_dir(&owner.0).unwrap();
    for (name, parameters) in [
        (
            "hls_h264.mp4",
            "keyint=50:min-keyint=50:scenecut=0:bframes=3:b-pyramid=normal",
        ),
        ("hls_long_gop.mp4", "keyint=125:min-keyint=125:scenecut=0"),
    ] {
        let input = owner.0.join(name);
        let generated = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=25:duration=20",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=20:sample_rate=48000",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-x264-params",
                parameters,
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let output = owner.0.join("native.mp4");
        let reference = owner.0.join("reference.mp4");
        let args = [
            "-ss",
            "00:00:09",
            "-i",
            input.to_str().unwrap(),
            "-t",
            "00:00:03",
            "-vcodec",
            "copy",
            "-acodec",
            "copy",
            "-y",
        ];
        for existing in [false, true] {
            exact_copy(ffmpeg, pair.ffprobe(), &args, &output, &reference, existing);
            std::fs::remove_file(&output).unwrap();
        }
    }
}

#[test]
fn audio_bearing_input_seek_matches_complete_packets_and_presented_samples() {
    let pair = test_oracle::require();
    let ffmpeg = pair.ffmpeg();
    let ffprobe = pair.ffprobe().to_path_buf();
    let owner = FixtureDir(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("ffmpeg-input-seek-av-{}", std::process::id())),
    );
    std::fs::create_dir(&owner.0).unwrap();
    for (rate, audio_rate, between_keys) in [
        ("24", "44100", "0.5"),
        ("30", "48000", "0.95"),
        ("10", "44100", "0.2"),
        ("10", "48000", "0.2"),
    ] {
        for bframes in ["0", "2"] {
            let input = owner
                .0
                .join(format!("source-{rate}-{audio_rate}-{bframes}.mp4"));
            let video_input = format!("testsrc2=size=96x64:rate={rate}:duration=4");
            let audio_input = format!("sine=frequency=713:sample_rate={audio_rate}:duration=4");
            let generated = Command::new(ffmpeg)
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &video_input,
                    "-f",
                    "lavfi",
                    "-i",
                    &audio_input,
                    "-c:v",
                    "libx264",
                    "-threads:v",
                    "1",
                    "-bf",
                    bframes,
                    "-g",
                    rate,
                    "-keyint_min",
                    rate,
                    "-sc_threshold",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                ])
                .arg(&input)
                .output()
                .unwrap();
            assert!(
                generated.status.success(),
                "{}",
                String::from_utf8_lossy(&generated.stderr)
            );
            for seek in ["0", between_keys, "1"] {
                for bound in ["-t", "-to"] {
                    for existing in [false, true] {
                        let output = owner.0.join("native.mp4");
                        let reference = owner.0.join("reference.mp4");
                        let args = [
                            "-y",
                            "-ss",
                            seek,
                            "-i",
                            input.to_str().unwrap(),
                            "-c",
                            "copy",
                            bound,
                            "1.25",
                        ];
                        let bytes =
                            exact_copy(ffmpeg, &ffprobe, &args, &output, &reference, existing);
                        if rate == "24"
                            && bframes == "2"
                            && seek == "0.5"
                            && bound == "-t"
                            && !existing
                        {
                            iso::mutations(&bytes);
                        }
                        std::fs::remove_file(output).unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn input_seek_copy_keeps_only_exact_packet_timelines() {
    let pair = test_oracle::require();
    let ffmpeg = pair.ffmpeg().to_path_buf();
    let ffprobe = pair.ffprobe().to_path_buf();
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ffmpeg-input-seek-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let owner = FixtureDir(path);
    let dir = &owner.0;
    let mut sources = Vec::new();
    for (name, bframes, gop) in [
        ("reordered", "2", "250"),
        ("inter", "0", "12"),
        ("intra", "0", "1"),
    ] {
        let path = dir.join(format!("{name}.mp4"));
        let result = Command::new(&ffmpeg)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=24:duration=3.25",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=751:sample_rate=44100:duration=3.25",
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                bframes,
                "-g",
                gop,
                "-keyint_min",
                gop,
                "-sc_threshold",
                "0",
                "-c:a",
                "aac",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        sources.push((path, bframes));
    }
    let bin = env!("CARGO_BIN_EXE_coreav");
    let fallback_path = std::env::join_paths(
        std::iter::once(ffmpeg.parent().unwrap().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    let mut index = 0;
    let mut check = |input: &Path, seek: &str, before: &[&str], after: &[&str], native: bool| {
        index += 1;
        let output = dir.join(format!("output-{index}.mp4"));
        let reference = dir.join(format!("reference-{index}.mp4"));
        let mut args = vec!["-y", "-ss", seek];
        args.extend(before);
        args.extend(["-i", input.to_str().unwrap(), "-c", "copy"]);
        args.extend(after);
        let pin = Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error"])
            .args(&args)
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            pin.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&pin.stderr)
        );
        args.push(output.to_str().unwrap());
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let entries = std::fs::read_dir(dir).unwrap().count();
            let strict = Command::new(bin)
                .args(["--strict-safe", "ffmpeg"])
                .args(&args)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(
                strict.status.code(),
                Some(if native { 0 } else { 4 }),
                "{args:?}: {}",
                String::from_utf8_lossy(&strict.stderr)
            );
            assert!(strict.stdout.is_empty());
            if native {
                assert!(strict.stderr.is_empty());
                assert_eq!(
                    snapshot(&output, &ffprobe),
                    snapshot(&reference, &ffprobe),
                    "{args:?}"
                );
                assert_eq!(
                    pixels(&output, &ffmpeg),
                    pixels(&reference, &ffmpeg),
                    "{args:?}"
                );
                if !after.contains(&"-an") {
                    assert_eq!(pcm(&output, &ffmpeg), pcm(&reference, &ffmpeg), "{args:?}");
                }
            } else {
                assert!(
                    String::from_utf8_lossy(&strict.stderr).contains(if seek.starts_with('-') {
                        "a negative time"
                    } else {
                        "input-seek copy"
                    }),
                    "{}",
                    String::from_utf8_lossy(&strict.stderr)
                );
                if existing {
                    assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
                } else {
                    assert!(!output.exists());
                }
                assert_eq!(std::fs::read_dir(dir).unwrap().count(), entries);
                let fallback = Command::new(bin)
                    .arg("ffmpeg")
                    .args(&args)
                    .env("PATH", &fallback_path)
                    .env_remove("COREAV_STRICT_SAFE")
                    .env_remove("COREAV_FALLBACK_ACTIVE")
                    .output()
                    .unwrap();
                assert!(
                    fallback.status.success(),
                    "{}",
                    String::from_utf8_lossy(&fallback.stderr)
                );
                assert!(fallback.stdout.is_empty());
                assert_eq!(
                    std::fs::read(&output).unwrap(),
                    std::fs::read(&reference).unwrap(),
                    "{args:?}"
                );
            }
            std::fs::remove_file(&output).unwrap();
        }
        #[cfg(unix)]
        if !native && seek == "0.2" {
            use std::os::unix::fs::PermissionsExt;
            let mock = dir.join("fallback-bin");
            std::fs::create_dir_all(&mock).unwrap();
            let program = mock.join("ffmpeg");
            std::fs::write(&program, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 19\n").unwrap();
            std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let replay = Command::new(bin)
                .arg("ffmpeg")
                .args(&args)
                .env_clear()
                .env("PATH", &mock)
                .output()
                .unwrap();
            assert_eq!(replay.status.code(), Some(19));
            assert_eq!(
                replay.stdout,
                std::iter::once("-hide_banner")
                    .chain(args.iter().copied())
                    .flat_map(|arg| arg.bytes().chain([0]))
                    .collect::<Vec<_>>()
            );
            assert!(!output.exists());
        }
    };
    for (source, bframes) in &sources {
        for seek in ["0", "0.2", "0.5"] {
            for selection in ["av", "video", "audio"] {
                let mut after = vec!["-t", "1"];
                match selection {
                    "video" => after.push("-an"),
                    "audio" => after.push("-vn"),
                    _ => {}
                }
                check(
                    source,
                    seek,
                    &[],
                    &after,
                    seek != "0.2"
                        && (selection == "av" || (*bframes == "0" && selection == "video")),
                );
            }
        }
    }
    let inter = &sources[1].0;
    check(inter, "0.5", &[], &["-an", "-t", "1.01"], false);
    check(inter, "0.5", &[], &["-an"], true);
    check(inter, "0.5", &[], &["-an", "-to", "1"], true);
    check(inter, "-0.2", &[], &["-an", "-t", "1"], false);
    check(inter, "0.2", &[], &["-an", "-ss", "0.3", "-t", "1"], false);
    check(inter, "0.5", &["-t", "1"], &["-an"], false);
    check(inter, "0.5", &["-to", "1"], &["-an"], false);
    let tagged = dir.join("tagged.mp4");
    let result = Command::new(&ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(inter)
        .args(["-c", "copy", "-metadata", "title=Source title"])
        .arg(&tagged)
        .output()
        .unwrap();
    assert!(result.status.success());
    check(&tagged, "0.5", &[], &["-an", "-t", "1"], false);
    check(&tagged, "0.5", &[], &["-t", "1"], false);
    let custom_handler = dir.join("audio-handler.mp4");
    let made = Command::new(&ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(inter)
        .args([
            "-c",
            "copy",
            "-metadata:s:a:0",
            "handler_name=Unpreserved audio handler",
        ])
        .arg(&custom_handler)
        .output()
        .unwrap();
    assert!(made.status.success());
    check(&custom_handler, "0.5", &[], &["-t", "1"], false);
    let volume = dir.join("audio-volume.mp4");
    let mut bytes = std::fs::read(inter).unwrap();
    let tkhd = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, kind)| *kind == b"tkhd")
        .nth(1)
        .unwrap()
        .0;
    assert_eq!(bytes[tkhd + 4], 0);
    bytes[tkhd + 40..tkhd + 42].copy_from_slice(&128u16.to_be_bytes());
    std::fs::write(&volume, bytes).unwrap();
    check(&volume, "0.5", &[], &["-t", "1"], false);
    for (scale, source, seek, native) in [
        ("1000", inter, "0.5", false),
        ("8000", inter, "0.5", false),
        ("10000", inter, "0.5", true),
        ("24000", inter, "0.5", true),
        ("1000000000", &sources[2].0, "0.541666667", false),
    ] {
        let clock = dir.join(format!("clock-{scale}.mp4"));
        let result = Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(source)
            .args(["-an", "-c", "copy", "-video_track_timescale", scale])
            .arg(&clock)
            .output()
            .unwrap();
        assert!(result.status.success());
        check(&clock, seek, &[], &["-an", "-t", "1"], native);
    }
    check(inter, "0.5", &[], &["-an", "-t", "1.000000001"], false);
    check(inter, "0.5", &[], &["-an", "-to", "1.000000001"], false);
}
