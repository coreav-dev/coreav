//! Matroska AAC copy clocks match the pin or refuse before publication.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct FixtureDir(PathBuf);
impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixtures() -> FixtureDir {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "mkv-aac-copy-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    FixtureDir(path)
}

fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn generate(ffmpeg: &Path, dir: &Path, rate: u32, channels: u32, samples: u32) -> PathBuf {
    let mut raw = Vec::new();
    for i in 0..samples {
        for channel in 0..channels {
            let value = i16::try_from((i * 103 + channel * 2903) % 24001).unwrap() - 12000;
            raw.extend_from_slice(&value.to_le_bytes());
        }
    }
    let source = dir.join(format!("{rate}-{channels}-{samples}.mkv"));
    let input = dir.join("input.s16");
    std::fs::write(&input, raw).unwrap();
    success(
        Command::new(ffmpeg)
            .args(["-nostdin", "-v", "error", "-f", "s16le", "-ar"])
            .arg(rate.to_string())
            .arg("-ac")
            .arg(channels.to_string())
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .arg("-i")
            .arg(input)
            .args(["-c:a", "aac", "-b:a", "128k", "-write_crc32", "0"])
            .arg(&source)
            .output()
            .unwrap(),
    );
    source
}

fn clocks(ffprobe: &Path, path: &Path) -> Value {
    let output = success(Command::new(ffprobe).args([
        "-v", "error", "-show_packets", "-show_streams", "-show_format", "-show_data_hash", "sha256", "-show_entries",
        "packet=pts,dts,duration,size,data_hash,flags:packet_side_data:stream=codec_name,profile,sample_rate,channels,time_base,start_pts,duration_ts,initial_padding,extradata_hash:stream_tags=:stream_disposition=:format=start_time,duration:format_tags=", "-of", "json",
    ]).arg(path).output().unwrap());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!value["packets"].as_array().unwrap().is_empty());
    value
}

fn edits(path: &Path) -> (u32, u64, u32, u64, coreav_isobmff::EditList) {
    let bytes = std::fs::read(path).unwrap();
    let movie = coreav_isobmff::probe(
        coreav_isobmff::SliceSource::new(&bytes),
        &coreav_isobmff::Limits::default(),
        &coreav_isobmff::Allowlist::default(),
    )
    .unwrap();
    let track = &movie.tracks[0];
    (
        movie.timescale,
        movie.mvhd_duration,
        track.timescale,
        track.mdhd_duration,
        track.edit_list,
    )
}

fn entries(path: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    paths
}

fn payloads(path: &Path) -> Vec<Vec<u8>> {
    let bytes = std::fs::read(path).unwrap();
    let limits = coreav_isobmff::Limits::default();
    let movie = coreav_isobmff::probe(
        coreav_isobmff::SliceSource::new(&bytes),
        &limits,
        &coreav_isobmff::Allowlist::default(),
    )
    .unwrap();
    let mut budget = coreav_isobmff::Budget::new(&limits);
    let payloads: Vec<_> = movie
        .packets(
            0,
            coreav_isobmff::SliceSource::new(&bytes),
            &limits,
            &mut budget,
        )
        .unwrap()
        .map(|p| {
            let p = p.unwrap();
            let at = usize::try_from(p.offset).unwrap();
            bytes[at..at + usize::try_from(p.size).unwrap()].to_vec()
        })
        .collect();
    assert!(!payloads.is_empty());
    payloads
}

#[test]
fn aac_copy_is_exactly_clocked_or_preserved_fallback() {
    let oracle = test_oracle::require();
    let ffmpeg = oracle.ffmpeg();
    let ffprobe = oracle.ffprobe();
    let owner = fixtures();
    let dir = &owner.0;
    let variants: &[&[&str]] = &[
        &["-c:a", "copy", "-vn"],
        &["-c", "copy", "-map", "0:a:0"],
        &["-map", "0:0", "-map", "-0:0", "-map", "0:a", "-c", "copy"],
        &[
            "-c:a:0",
            "copy",
            "-vn",
            "-f",
            "mp4",
            "-movflags",
            "+faststart",
        ],
    ];
    for rate in [32_000, 44_100, 48_000] {
        for channels in [1, 2] {
            for samples in [1024, 4096, 4097] {
                let source = generate(ffmpeg, dir, rate, channels, samples);
                for &options in variants {
                    let extension = if options.contains(&"-f") {
                        "m4a"
                    } else {
                        "mp4"
                    };
                    let expected = dir.join(format!("expected.{extension}"));
                    let actual = dir.join(format!("actual.{extension}"));
                    let mut args = vec!["-v", "error", "-y", "-i", source.to_str().unwrap()];
                    args.extend_from_slice(options);
                    let pin = success(
                        Command::new(ffmpeg)
                            .args(["-v", "error", "-y"])
                            .args(
                                include_str!("../../../scripts/oracle-audio-args.txt")
                                    .split_whitespace(),
                            )
                            .arg("-i")
                            .arg(&source)
                            .args(options)
                            .arg(&expected)
                            .output()
                            .unwrap(),
                    );
                    if rate == 32_000 {
                        success(
                            Command::new(env!("CARGO_BIN_EXE_coreav"))
                                .args(["--strict-safe", "ffmpeg"])
                                .args(&args)
                                .arg(&actual)
                                .env_remove("COREAV_FALLBACK_ACTIVE")
                                .output()
                                .unwrap(),
                        );
                        assert_eq!(
                            clocks(ffprobe, &actual),
                            clocks(ffprobe, &expected),
                            "{rate}/{channels}/{samples}/{options:?}"
                        );
                        assert_eq!(edits(&actual), edits(&expected));
                        assert_eq!(payloads(&actual), payloads(&expected));
                        std::fs::remove_file(&actual).unwrap();
                    } else {
                        for existing in [false, true] {
                            if existing {
                                std::fs::write(&actual, b"preserve this destination").unwrap();
                            }
                            let before = entries(dir);
                            let refused = Command::new(env!("CARGO_BIN_EXE_coreav"))
                                .args(["--strict-safe", "ffmpeg"])
                                .args(&args)
                                .arg(&actual)
                                .env_remove("COREAV_FALLBACK_ACTIVE")
                                .output()
                                .unwrap();
                            assert_eq!(
                                refused.status.code(),
                                Some(4),
                                "{rate}/{channels}/{samples}/{options:?}: {}",
                                String::from_utf8_lossy(&refused.stderr)
                            );
                            assert!(refused.stdout.is_empty());
                            assert_eq!(entries(dir), before);
                            if existing {
                                assert_eq!(
                                    std::fs::read(&actual).unwrap(),
                                    b"preserve this destination"
                                );
                            }
                            let fallback = Command::new(env!("CARGO_BIN_EXE_coreav"))
                                .arg("ffmpeg")
                                .args(&args)
                                .arg(&actual)
                                .env("COREAV_FALLBACK", ffmpeg)
                                .env_remove("COREAV_STRICT_SAFE")
                                .env_remove("COREAV_FALLBACK_ACTIVE")
                                .output()
                                .unwrap();
                            assert_eq!(fallback.status.code(), pin.status.code());
                            assert_eq!(fallback.stdout, pin.stdout);
                            assert_eq!(
                                std::fs::read(&actual).unwrap(),
                                std::fs::read(&expected).unwrap()
                            );
                            let after = entries(dir);
                            assert_eq!(after.len(), before.len() + usize::from(!existing));
                            std::fs::remove_file(&actual).unwrap();
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn unselected_aac_and_iso_copy_keep_their_native_paths() {
    let oracle = test_oracle::require();
    let ffmpeg = oracle.ffmpeg();
    let ffprobe = oracle.ffprobe();
    let owner = fixtures();
    let dir = &owner.0;
    let source = generate(ffmpeg, dir, 48_000, 1, 4096);
    let av = dir.join("av.mkv");
    success(
        Command::new(ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=25:duration=0.16",
            ])
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .arg("-i")
            .arg(&source)
            .args([
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                "0",
                "-g",
                "4",
                "-c:a",
                "copy",
            ])
            .arg(&av)
            .output()
            .unwrap(),
    );
    let native = dir.join("native.mp4");
    success(
        Command::new(env!("CARGO_BIN_EXE_coreav"))
            .arg("remux")
            .arg(&av)
            .arg(&native)
            .args(["--streams", "0"])
            .output()
            .unwrap(),
    );
    for options in [
        &["-c", "copy", "-an"][..],
        &["-map", "0:v:0", "-c:v", "copy"][..],
    ] {
        let actual = dir.join("video-only.mp4");
        success(
            Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(["--strict-safe", "ffmpeg", "-y", "-i"])
                .arg(&av)
                .args(options)
                .arg(&actual)
                .output()
                .unwrap(),
        );
        assert_eq!(
            std::fs::read(&actual).unwrap(),
            std::fs::read(&native).unwrap()
        );
    }
    let refused = dir.join("av-refused.mp4");
    let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["--strict-safe", "ffmpeg", "-i"])
        .arg(&av)
        .args(["-c", "copy"])
        .arg(&refused)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    assert!(!refused.exists());

    let aligned = generate(ffmpeg, dir, 32_000, 2, 4096);
    let iso = dir.join("iso.mp4");
    success(
        Command::new(ffmpeg)
            .args(["-v", "error"])
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .arg("-i")
            .arg(aligned)
            .args(["-c", "copy"])
            .arg(&iso)
            .output()
            .unwrap(),
    );
    let expected = dir.join("iso-reference.mp4");
    success(
        Command::new(ffmpeg)
            .args(["-v", "error"])
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .arg("-i")
            .arg(&iso)
            .args(["-c", "copy"])
            .arg(&expected)
            .output()
            .unwrap(),
    );
    let actual = dir.join("iso-actual.mp4");
    success(
        Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["--strict-safe", "ffmpeg", "-i"])
            .arg(&iso)
            .args(["-c", "copy"])
            .arg(&actual)
            .output()
            .unwrap(),
    );
    assert_eq!(clocks(ffprobe, &actual), clocks(ffprobe, &expected));
    assert_eq!(payloads(&actual), payloads(&expected));
    let a = edits(&actual);
    let b = edits(&expected);
    assert_eq!((a.2, a.3), (b.2, b.3));
    assert_eq!(
        u128::from(a.1) * u128::from(b.0),
        u128::from(b.1) * u128::from(a.0)
    );
    match (a.4, b.4) {
        (
            coreav_isobmff::EditList::Simple {
                empty: ae,
                media_time: am,
                segment: ad,
            },
            coreav_isobmff::EditList::Simple {
                empty: be,
                media_time: bm,
                segment: bd,
            },
        ) => {
            assert_eq!(am, bm);
            assert_eq!(
                u128::from(ae) * u128::from(b.0),
                u128::from(be) * u128::from(a.0)
            );
            assert_eq!(
                u128::from(ad) * u128::from(b.0),
                u128::from(bd) * u128::from(a.0)
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn one_tick_source_changes_refuse_without_changing_fallback() {
    let ffmpeg = test_oracle::require().ffmpeg();
    let owner = fixtures();
    let dir = &owner.0;
    let source = generate(ffmpeg, dir, 32_000, 1, 4096);
    let bytes = std::fs::read(source).unwrap();
    let demux = coreav_mkv::Limits::default();
    let segment = coreav_mkv::probe(
        coreav_mkv::SliceSource::new(&bytes),
        &demux,
        &coreav_mkv::Allowlist::default(),
    )
    .unwrap();
    let first = segment
        .packets(coreav_mkv::SliceSource::new(&bytes), &demux)
        .next()
        .unwrap()
        .unwrap();
    for change in ["origin", "delay", "default", "endpoint"] {
        let mut changed = bytes.clone();
        if change == "origin" {
            let at = usize::try_from(first.block_offset).unwrap();
            assert_eq!(changed[at], 0x81);
            changed[at + 1..at + 3].copy_from_slice(&1i16.to_be_bytes());
        } else {
            let (needle, replacement) = match change {
                "delay" => (
                    [vec![0x56, 0xaa, 0x84], 32_000_000u32.to_be_bytes().to_vec()].concat(),
                    33_000_000u32.to_be_bytes().to_vec(),
                ),
                "default" => (
                    [
                        vec![0x23, 0xe3, 0x83, 0x84],
                        32_000_000u32.to_be_bytes().to_vec(),
                    ]
                    .concat(),
                    31_000_000u32.to_be_bytes().to_vec(),
                ),
                _ => (
                    [vec![0x44, 0x89, 0x88], 160f64.to_be_bytes().to_vec()].concat(),
                    161f64.to_be_bytes().to_vec(),
                ),
            };
            let matches: Vec<_> = changed
                .windows(needle.len())
                .enumerate()
                .filter(|(_, b)| *b == needle)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(matches.len(), 1);
            let at = matches[0] + needle.len() - replacement.len();
            changed[at..at + replacement.len()].copy_from_slice(&replacement);
        }
        let input = dir.join(format!("{change}.mkv"));
        std::fs::write(&input, changed).unwrap();
        let expected = dir.join("reference.mp4");
        let actual = dir.join("actual.mp4");
        let pin = success(
            Command::new(ffmpeg)
                .args(["-v", "error", "-y"])
                .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
                .arg("-i")
                .arg(&input)
                .args(["-c", "copy"])
                .arg(&expected)
                .output()
                .unwrap(),
        );
        for existing in [false, true] {
            if existing {
                std::fs::write(&actual, b"preserve").unwrap();
            }
            let before = entries(dir);
            let refused = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(["--strict-safe", "ffmpeg", "-v", "error", "-y", "-i"])
                .arg(&input)
                .args(["-c", "copy"])
                .arg(&actual)
                .output()
                .unwrap();
            assert_eq!(
                refused.status.code(),
                Some(4),
                "{change}: {}",
                String::from_utf8_lossy(&refused.stderr)
            );
            assert!(refused.stdout.is_empty());
            assert_eq!(entries(dir), before);
            if existing {
                assert_eq!(std::fs::read(&actual).unwrap(), b"preserve");
            }
            let fallback = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(["ffmpeg", "-v", "error", "-y", "-i"])
                .arg(&input)
                .args(["-c", "copy"])
                .arg(&actual)
                .env("COREAV_FALLBACK", ffmpeg)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(fallback.status.code(), pin.status.code());
            assert_eq!(fallback.stdout, pin.stdout);
            assert_eq!(
                std::fs::read(&actual).unwrap(),
                std::fs::read(&expected).unwrap()
            );
            std::fs::remove_file(&actual).unwrap();
        }
    }
}
