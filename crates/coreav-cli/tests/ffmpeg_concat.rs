//! Complete native concat output and atomic refusals against the pinned oracle.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "support/concat_output.rs"]
mod concat_output;
#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

struct FixtureDir(PathBuf);

impl std::ops::Deref for FixtureDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for FixtureDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn oracle_scratch(name: &str) -> FixtureDir {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    FixtureDir(path)
}

struct Run {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

struct MetadataFault<'a> {
    bytes: &'a [u8],
    offset: u64,
    error: coreav_isobmff::Error,
}

impl coreav_isobmff::Source for MetadataFault<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        Ok(self.bytes.len() as u64)
    }

    fn read_exact_at(
        &mut self,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<(), coreav_isobmff::Error> {
        if offset == self.offset && bytes.len() == 2 {
            return Err(self.error.clone());
        }
        coreav_isobmff::SliceSource::new(self.bytes).read_exact_at(offset, bytes)
    }
}

fn with_field_payload(bytes: &[u8], field: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && at + size <= bytes.len());
        let kind: &[u8; 4] = bytes[at + 4..at + 8].try_into().unwrap();
        let payload = &bytes[at + 8..at + size];
        let body = if kind == b"fiel" {
            field.to_vec()
        } else {
            let prefix = match kind {
                b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => 0,
                b"stsd" => 8,
                b"av01" | b"vp09" | b"hvc1" => 78,
                _ => payload.len(),
            };
            let mut body = payload[..prefix].to_vec();
            if prefix < payload.len() {
                body.extend(with_field_payload(&payload[prefix..], field));
            }
            body
        };
        output.extend_from_slice(&u32::try_from(body.len() + 8).unwrap().to_be_bytes());
        output.extend_from_slice(kind);
        output.extend(body);
        at += size;
    }
    output
}

fn ffmpeg(args: &[&str]) -> Run {
    let out = Command::new(bin())
        .arg("ffmpeg")
        .args(args)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: out.stdout,
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn oracle() -> Option<(PathBuf, PathBuf)> {
    let pin = test_oracle::require();
    Some((pin.ffmpeg().to_path_buf(), pin.ffprobe().to_path_buf()))
}

#[test]
fn native_video_metadata_compares_complete_records_and_all_packet_bytes() {
    let Some((reference, probe)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("native-video-metadata");
    let policy: Vec<_> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    for (codec, options) in [
        (
            "av1",
            vec!["-c:v", "libaom-av1", "-cpu-used", "8", "-g", "12"],
        ),
        (
            "vp9",
            vec![
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
                "-g",
                "12",
            ],
        ),
        (
            "hevc",
            vec![
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-tag:v",
                "hvc1",
                "-x265-params",
                "pools=none:frame-threads=1:keyint=12:min-keyint=12:scenecut=0:bframes=0:log-level=error",
            ],
        ),
    ] {
        let source = dir.join(format!("{codec}.mp4"));
        let generated = Command::new(&reference)
            .args(["-v", "error", "-f", "lavfi"])
            .args(&policy)
            .args([
                "-i",
                "testsrc2=size=80x64:rate=24:duration=0.5",
                "-an",
                "-pix_fmt",
                "yuv420p",
                "-threads:v",
                "1",
            ])
            .args(options)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let bytes = std::fs::read(&source).unwrap();
        let fields: Vec<_> = bytes
            .windows(8)
            .enumerate()
            .filter_map(|(at, bytes)| (bytes == b"\0\0\0\x0afiel").then_some(at))
            .collect();
        assert_eq!(fields.len(), 1);
        let at = fields[0];
        assert_eq!(&bytes[at + 8..at + 10], &[1, 0]);
        let offset = (at + 8) as u64;
        for error in [
            coreav_isobmff::Error::Io(std::io::ErrorKind::PermissionDenied),
            coreav_isobmff::Error::Truncated { offset },
            coreav_isobmff::Error::LimitExceeded {
                limit: coreav_isobmff::LimitKind::ReadBytes,
                offset,
            },
        ] {
            let mut output = Vec::new();
            let result = coreav_isobmff_write::join_mp4(
                (0..2)
                    .map(|_| MetadataFault {
                        bytes: &bytes,
                        offset,
                        error: error.clone(),
                    })
                    .collect(),
                &Default::default(),
                &Default::default(),
                &coreav_isobmff_write::JoinOptions {
                    video: coreav_isobmff_write::VideoChoice::First,
                    audio: coreav_isobmff_write::JoinAudio::None,
                    placement: coreav_isobmff_write::Placement::AfterVideo,
                    compatibility: None,
                },
                &mut output,
            );
            assert!(
                matches!(result, Err(coreav_isobmff_write::JoinError::Input {
                input: 0, error: coreav_isobmff_write::ExtractError::Source(ref actual),
            }) if actual == &error)
            );
            assert!(output.is_empty());
        }
        let absent = dir.join(format!("{codec}-absent.mp4"));
        let mut changed = bytes.clone();
        changed[at + 4..at + 8].copy_from_slice(b"free");
        std::fs::write(&absent, &changed).unwrap();
        assert!(
            bytes.windows(4).position(|x| x == b"mdat").unwrap()
                < bytes.windows(4).position(|x| x == b"moov").unwrap()
        );
        for control in [
            changed,
            with_field_payload(&bytes, &[]),
            with_field_payload(&bytes, &[1]),
        ] {
            let source = coreav_isobmff::probe(
                coreav_isobmff::SliceSource::new(&control),
                &Default::default(),
                &Default::default(),
            )
            .unwrap();
            let mut output = Vec::new();
            let summary = coreav_isobmff_write::join_mp4(
                (0..2)
                    .map(|_| coreav_isobmff::SliceSource::new(&control))
                    .collect(),
                &Default::default(),
                &Default::default(),
                &coreav_isobmff_write::JoinOptions {
                    video: coreav_isobmff_write::VideoChoice::First,
                    audio: coreav_isobmff_write::JoinAudio::None,
                    placement: coreav_isobmff_write::Placement::AfterVideo,
                    compatibility: None,
                },
                &mut output,
            )
            .unwrap();
            assert_eq!(
                summary.boxes_left_out,
                source.tracks[0].description.other_boxes
            );
            let written = coreav_isobmff::probe(
                coreav_isobmff::SliceSource::new(&output),
                &Default::default(),
                &Default::default(),
            )
            .unwrap();
            assert!(
                !written.tracks[0]
                    .description
                    .other_boxes
                    .contains(&coreav_isobmff::FourCc(*b"fiel"))
            );
        }
        let alternate = dir.join(format!("{codec}-alternate.mp4"));
        let mut changed = bytes.clone();
        changed[at + 8..at + 10].copy_from_slice(&[2, 9]);
        std::fs::write(&alternate, &changed).unwrap();
        let blank = dir.join(format!("{codec}-blank.mp4"));
        let remuxed = Command::new(&reference)
            .args(["-v", "error"])
            .args(&policy)
            .arg("-i")
            .arg(&source)
            .args(["-c", "copy", "-metadata:s:v:0", "encoder="])
            .arg(&blank)
            .output()
            .unwrap();
        assert!(remuxed.status.success());
        for (name, inputs, flags) in [
            ("natural", vec![&source, &source], vec![]),
            ("three", vec![&source, &source, &source], vec![]),
            (
                "faststart",
                vec![&source, &source],
                vec!["-movflags", "+faststart"],
            ),
            (
                "mapped",
                vec![&source, &source],
                vec!["-map", "0:v:0", "-an"],
            ),
            ("absent", vec![&absent, &absent], vec![]),
            ("first-present", vec![&source, &absent], vec![]),
            ("first-absent", vec![&absent, &source], vec![]),
            ("alternate", vec![&alternate, &source], vec![]),
            ("blank", vec![&blank, &blank], vec![]),
        ] {
            let list = dir.join(format!("{codec}-{name}.txt"));
            let entries: String = inputs
                .iter()
                .map(|input| format!("file '{}'\n", input.file_name().unwrap().to_str().unwrap()))
                .collect();
            std::fs::write(&list, entries).unwrap();
            let expected = dir.join(format!("{codec}-{name}-reference.mp4"));
            let copied = Command::new(&reference)
                .args(["-v", "error", "-f", "concat"])
                .args(&policy)
                .arg("-i")
                .arg(&list)
                .args(["-c", "copy"])
                .args(&flags)
                .arg(&expected)
                .output()
                .unwrap();
            assert!(
                copied.status.success(),
                "{}",
                String::from_utf8_lossy(&copied.stderr)
            );
            for strict in [false, true] {
                let candidate = dir.join(format!("{codec}-{name}-{strict}.mp4"));
                let log = candidate.with_extension("fallback.log");
                let mut command = Command::new(bin());
                if strict {
                    command.arg("--strict-safe");
                }
                let result = command
                    .args(["ffmpeg", "-f", "concat", "-i"])
                    .arg(&list)
                    .args(["-c", "copy"])
                    .args(&flags)
                    .arg(&candidate)
                    .env_remove("COREAV_STRICT_SAFE")
                    .env("COREAV_FALLBACK", dir.join("unavailable-ffmpeg"))
                    .env("COREAV_FALLBACK_LOG", &log)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{codec} {name}: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                assert!(!log.exists());
                concat_output::compare(
                    &reference,
                    &probe,
                    &candidate,
                    &expected,
                    &list,
                    if name == "natural" && strict {
                        "native-mutations"
                    } else {
                        "native"
                    },
                );
            }
            if matches!(name, "natural" | "three") {
                let joined = dir.join(format!("{codec}-{name}-joined.mp4"));
                let result = Command::new(bin())
                    .args(["join", "--audio", "none", "-o"])
                    .arg(&joined)
                    .args(&inputs)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                let summary: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
                assert!(
                    !summary["boxes_left_out"]
                        .as_array()
                        .unwrap()
                        .contains(&serde_json::Value::from("fiel")),
                    "retained field metadata is reported as omitted"
                );
                assert!(
                    std::fs::read(&joined)
                        .unwrap()
                        .windows(10)
                        .any(|bytes| bytes == b"\0\0\0\x0afiel\x01\0")
                );
                concat_output::compare(&reference, &probe, &joined, &expected, &list, "native");
            }
        }
        let multiple = dir.join(format!("{codec}-multiple.mp4"));
        let remuxed = Command::new(&reference)
            .args(["-v", "error"])
            .args(&policy)
            .arg("-i")
            .arg(&source)
            .args(&policy)
            .arg("-i")
            .arg(&alternate)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "1:v:0",
                "-c",
                "copy",
                "-metadata:s:v:1",
                "encoder=selected",
            ])
            .arg(&multiple)
            .output()
            .unwrap();
        assert!(remuxed.status.success());
        let list = dir.join(format!("{codec}-selected.txt"));
        std::fs::write(&list, format!("file '{codec}-multiple.mp4'\n").repeat(2)).unwrap();
        let expected = dir.join(format!("{codec}-selected-reference.mp4"));
        let copied = Command::new(&reference)
            .args(["-v", "error", "-f", "concat"])
            .args(&policy)
            .arg("-i")
            .arg(&list)
            .args(["-map", "0:v:1", "-c", "copy"])
            .arg(&expected)
            .output()
            .unwrap();
        assert!(copied.status.success());
        let mut bytes = Vec::new();
        coreav_isobmff_write::join_mp4(
            vec![
                coreav_isobmff::FileSource::open(&multiple).unwrap(),
                coreav_isobmff::FileSource::open(&multiple).unwrap(),
            ],
            &Default::default(),
            &Default::default(),
            &coreav_isobmff_write::JoinOptions {
                video: coreav_isobmff_write::VideoChoice::Stream(1),
                audio: coreav_isobmff_write::JoinAudio::None,
                placement: coreav_isobmff_write::Placement::AfterVideo,
                compatibility: None,
            },
            &mut bytes,
        )
        .unwrap();
        let selected = dir.join(format!("{codec}-selected.mp4"));
        std::fs::write(&selected, bytes).unwrap();
        concat_output::compare(&reference, &probe, &selected, &expected, &list, "native");
    }
}

#[test]
fn concat_audio_gaps_are_refused_with_exact_pinned_fallback() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-concat-audio-gaps");
    for (name, duration, rate) in [("short", "3", "24"), ("fractional", "3.25", "30000/1001")] {
        let edited = dir.join(format!("{name}-edited.mp4"));
        let source = dir.join(format!("{name}.mp4"));
        let generated = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size=64x48:rate={rate}:duration=3.25"))
            .args(["-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency=751:sample_rate=44100:duration={duration}"
            ))
            .args([
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-g",
                "24",
                "-bf",
                "2",
                "-c:a",
                "aac",
            ])
            .arg(&edited)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let unedited = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&edited)
            .args(["-c", "copy", "-use_editlist", "0"])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            unedited.status.success(),
            "{}",
            String::from_utf8_lossy(&unedited.stderr)
        );
        let movie = coreav_isobmff::probe(
            coreav_isobmff::FileSource::open(&source).unwrap(),
            &Default::default(),
            &coreav_isobmff::Allowlist::listing(),
        )
        .unwrap();
        assert!(movie.tracks.iter().all(|track| track.edits.is_empty()));
        let path = std::env::join_paths(
            std::iter::once(ffmpeg_bin.parent().unwrap().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ),
        )
        .unwrap();
        for inputs in [2, 3] {
            let list = dir.join("list.txt");
            std::fs::write(&list, format!("file '{name}.mp4'\n").repeat(inputs)).unwrap();
            for (safe, options) in [
                (true, vec!["-c", "copy"]),
                (false, vec!["-c", "copy"]),
                (true, vec!["-map", "0", "-c", "copy"]),
                (true, vec!["-c", "copy", "-movflags", "+faststart"]),
            ] {
                let output = dir.join("output.mp4");
                let reference = dir.join("reference.mp4");
                let mut args = vec!["-y", "-f", "concat"];
                if safe {
                    args.extend(["-safe", "0"]);
                }
                args.extend(["-i", list.to_str().unwrap()]);
                args.extend(&options);
                args.push(output.to_str().unwrap());
                for existing in [false, true] {
                    if existing {
                        std::fs::write(&output, b"existing output").unwrap();
                    }
                    let before = std::fs::read_dir(&dir).unwrap().count();
                    let result = ffmpeg(&args);
                    assert_eq!(
                        result.code, 4,
                        "{name}, {inputs}, {options:?}: {}",
                        result.stderr
                    );
                    assert!(
                        result.stderr.contains("several audio edits"),
                        "{}",
                        result.stderr
                    );
                    assert!(result.stdout.is_empty());
                    if existing {
                        assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
                    } else {
                        assert!(!output.exists());
                    }
                    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), before);
                }
                let copied = Command::new(&ffmpeg_bin)
                    .args(["-nostdin", "-v", "error"])
                    .args(&args[..args.len() - 1])
                    .arg(&reference)
                    .output()
                    .unwrap();
                assert!(
                    copied.status.success(),
                    "{}",
                    String::from_utf8_lossy(&copied.stderr)
                );
                let fallback = Command::new(bin())
                    .arg("ffmpeg")
                    .args(&args)
                    .env("PATH", &path)
                    .env_remove("COREAV_STRICT_SAFE")
                    .env_remove("COREAV_FALLBACK_ACTIVE")
                    .output()
                    .unwrap();
                assert!(
                    fallback.status.success(),
                    "{}",
                    String::from_utf8_lossy(&fallback.stderr)
                );
                assert_eq!(
                    std::fs::read(&output).unwrap(),
                    std::fs::read(&reference).unwrap()
                );
                std::fs::remove_file(&output).unwrap();
            }
            let video = dir.join("video.mp4");
            let result = ffmpeg(&[
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                list.to_str().unwrap(),
                "-an",
                "-c",
                "copy",
                video.to_str().unwrap(),
            ]);
            assert_eq!(result.code, 0, "{}", result.stderr);
            assert!(result.stdout.is_empty());
            let reference = dir.join("video-reference.mp4");
            let reference_run = Command::new(&ffmpeg_bin)
                .args([
                    "-nostdin", "-v", "error", "-y", "-f", "concat", "-safe", "0",
                ])
                .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
                .arg("-i")
                .arg(&list)
                .args(["-an", "-c", "copy"])
                .arg(&reference)
                .output()
                .unwrap();
            assert!(
                reference_run.status.success(),
                "{}",
                String::from_utf8_lossy(&reference_run.stderr)
            );
            concat_output::compare(
                &ffmpeg_bin,
                &ffprobe_bin,
                &video,
                &reference,
                &list,
                &format!("gap-{name}-{inputs}-video"),
            );
            std::fs::remove_file(video).unwrap();
        }
    }
}

#[test]
fn lossless_audiovisual_concat_preserves_complete_native_output() {
    let Some((reference, probe)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-concat-lossless");
    for codec in ["flac", "alac"] {
        let source = dir.join(format!("{codec}.mp4"));
        let mut command = Command::new(&reference);
        command
            .args(["-nostdin", "-v", "error", "-f", "lavfi"])
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .args([
                "-i",
                "testsrc2=size=320x240:rate=25:duration=2",
                "-f",
                "lavfi",
            ])
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .args([
                "-i",
                "sine=frequency=440:duration=2:sample_rate=48000",
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "25",
                "-ac",
                "2",
                "-c:a",
                codec,
            ]);
        if codec == "alac" {
            command.args(["-sample_fmt", "s16p"]);
        }
        let generated = command.arg("-shortest").arg(&source).output().unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let list = dir.join("list.txt");
        std::fs::write(&list, format!("file '{codec}.mp4'\n").repeat(2)).unwrap();
        let output = dir.join("native.mp4");
        let expected = dir.join("reference.mp4");
        let args = [
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
        ];
        let copied = Command::new(&reference)
            .args(["-nostdin", "-v", "error"])
            .args(args)
            .arg(&expected)
            .output()
            .unwrap();
        assert!(
            copied.status.success(),
            "{}",
            String::from_utf8_lossy(&copied.stderr)
        );
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let copied = ffmpeg(&[args.as_slice(), &[output.to_str().unwrap()]].concat());
            assert_eq!(copied.code, 0, "{}", copied.stderr);
            assert!(copied.stdout.is_empty());
            concat_output::compare(
                &reference,
                &probe,
                &output,
                &expected,
                &list,
                &format!("audio-copy-{codec}"),
            );
            std::fs::remove_file(&output).unwrap();
        }
        if codec == "flac" {
            std::fs::write(&list, "file 'flac.mp4'\n").unwrap();
            let copied = ffmpeg(&[args.as_slice(), &[output.to_str().unwrap()]].concat());
            assert_eq!(copied.code, 0, "{}", copied.stderr);
            let packets = |path: &Path| {
                let (document, stderr) = concat_output::probe(&probe, path);
                assert!(stderr.is_empty());
                let audio = document["streams"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|stream| stream["codec_type"] == "audio")
                    .unwrap()["index"]
                    .clone();
                document["packets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|packet| packet["stream_index"] == audio)
                    .map(|packet| {
                        (
                            packet["size"].clone(),
                            packet["data"].clone(),
                            packet["data_hash"].clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let expected = packets(&source);
            assert!(!expected.is_empty());
            assert_eq!(
                packets(&output),
                expected,
                "every original audio packet byte"
            );
            std::fs::remove_file(&output).unwrap();
        }
        let hostile = dir.join("inband.txt");
        std::fs::write(&hostile, "file 'reference.mp4'\nfile 'reference.mp4'\n").unwrap();
        let hostile_args = [
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            hostile.to_str().unwrap(),
            "-c",
            "copy",
        ];
        let fallback_reference = dir.join("fallback-reference.mp4");
        let pin = Command::new(&reference)
            .args(["-nostdin", "-v", "error"])
            .args(hostile_args)
            .arg(&fallback_reference)
            .output()
            .unwrap();
        assert!(
            pin.status.success(),
            "{}",
            String::from_utf8_lossy(&pin.stderr)
        );
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let entries = std::fs::read_dir(&dir).unwrap().count();
            let strict = ffmpeg(&[hostile_args.as_slice(), &[output.to_str().unwrap()]].concat());
            assert_eq!(strict.code, 4, "{}", strict.stderr);
            assert!(
                strict
                    .stderr
                    .contains("AVC parameter sets carried inside avc1 samples")
            );
            assert!(strict.stdout.is_empty());
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), entries);
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
            } else {
                assert!(!output.exists());
            }
            let fallback = Command::new(bin())
                .arg("ffmpeg")
                .args(hostile_args)
                .arg(&output)
                .env("COREAV_FALLBACK", &reference)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(fallback.status.code(), pin.status.code());
            assert_eq!(fallback.stdout, pin.stdout);
            assert_eq!(
                std::fs::read(&output).unwrap(),
                std::fs::read(&fallback_reference).unwrap()
            );
            std::fs::remove_file(&output).unwrap();
        }
    }
}

#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn c02_guard_concat_preserves_complete_native_output() {
    let Some((reference, probe)) = oracle() else {
        return;
    };
    let fixtures = std::env::var_os("COREAV_FIXTURES").expect("set COREAV_FIXTURES");
    let dir = oracle_scratch("ffmpeg-concat-c02-complete");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = std::env::join_paths(
        std::iter::once(reference.parent().unwrap().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    let prepared = Command::new("python3")
        .args(["-I", "-c", r#"
import json
import pathlib
import sys
sys.path.insert(0, str(pathlib.Path(sys.argv[1]) / 'scripts'))
import bench_guard
coreav = sys.argv[2]
fixtures = pathlib.Path(sys.argv[3])
work = pathlib.Path(sys.argv[4])
bench_guard.derive(fixtures, work)
fixture, template = bench_guard.CASES['ffmpeg-C02-join']
source = work / fixture
assert source.is_file(), 'C02 derived fixture is missing'
prepared = {}
for arg in template:
    if arg.startswith('{inputs:'):
        prepared[arg] = bench_guard.inputs_of(coreav, source, arg[len('{inputs:'):-1], work)
assert None not in prepared.values(), 'C02 input preparation failed'
output = work / 'native'
output.mkdir()
command = []
for arg in template:
    command.extend(prepared[arg] if arg in prepared else [bench_guard.argument(arg, source, None, str(output))])
print(json.dumps(command))
"#])
        .arg(&root).arg(bin()).arg(fixtures).arg(&*dir)
        .env("PATH", path)
        .env("COREAV_STRICT_SAFE", "1")
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .output().unwrap();
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let command: Vec<String> = serde_json::from_slice(&prepared.stdout).unwrap();
    assert_eq!(command[0], "ffmpeg");
    let candidate = Path::new(command.last().unwrap());
    let copied = Command::new(bin())
        .args(&command)
        .env("COREAV_STRICT_SAFE", "1")
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .output()
        .unwrap();
    assert!(
        copied.status.success(),
        "{}",
        String::from_utf8_lossy(&copied.stderr)
    );
    assert!(copied.stdout.is_empty());
    let expected = dir.join("reference.mp4");
    let pin = Command::new(&reference)
        .args(["-nostdin", "-v", "error"])
        .args(&command[1..command.len() - 1])
        .arg(&expected)
        .output()
        .unwrap();
    assert!(
        pin.status.success(),
        "{}",
        String::from_utf8_lossy(&pin.stderr)
    );
    concat_output::compare(
        &reference,
        &probe,
        candidate,
        &expected,
        Path::new(&command[4]),
        "guard-C02",
    );
}

#[test]
fn unedited_audiovisual_concat_matches_the_pinned_ffmpeg() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-concat-unedited-oracle");
    let audio_input: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let make = |args: &[&str]| {
        let mut command = Command::new(&ffmpeg_bin);
        command.args(["-nostdin", "-v", "error"]);
        for arg in args {
            if *arg == "-i" {
                command.args(&audio_input);
            }
            command.arg(arg);
        }
        let result = command.current_dir(&dir).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    make(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=96x64:rate=24:duration=3.25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=733:sample_rate=44100:duration=3.25",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "libx264",
        "-threads:v",
        "1",
        "-g",
        "24",
        "-bf",
        "2",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-ac",
        "2",
        "-b:a",
        "96k",
        "-shortest",
        "original.mp4",
    ]);
    make(&[
        "-i",
        "original.mp4",
        "-c",
        "copy",
        "-use_editlist",
        "0",
        "source.mp4",
    ]);
    let source = dir.join("source.mp4");
    let parsed = coreav_isobmff::probe(
        coreav_isobmff::FileSource::open(&source).unwrap(),
        &Default::default(),
        &coreav_isobmff::Allowlist::listing(),
    )
    .unwrap();
    assert!(parsed.tracks.iter().all(|track| track.edits.is_empty()));
    let list = dir.join("list.txt");
    std::fs::write(&list, "file source.mp4\nfile source.mp4\n").unwrap();
    let candidate = dir.join("candidate.mp4");
    let r = ffmpeg(&[
        "-f",
        "concat",
        "-i",
        list.to_str().unwrap(),
        "-c",
        "copy",
        candidate.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.is_empty());
    make(&[
        "-f",
        "concat",
        "-i",
        "list.txt",
        "-c",
        "copy",
        "reference.mp4",
    ]);
    concat_output::compare(
        &ffmpeg_bin,
        &ffprobe_bin,
        &candidate,
        &dir.join("reference.mp4"),
        &list,
        "01-unedited-av",
    );
    let mut changed = std::fs::read(&candidate).unwrap();
    let bitrate = changed
        .windows(4)
        .position(|bytes| bytes == b"btrt")
        .unwrap()
        + 12;
    changed[bitrate..bitrate + 4].copy_from_slice(&12345u32.to_be_bytes());
    std::fs::write(&candidate, changed).unwrap();
    assert!(
        std::panic::catch_unwind(|| concat_output::compare(
            &ffmpeg_bin,
            &ffprobe_bin,
            &candidate,
            &dir.join("reference.mp4"),
            &list,
            "01-unedited-av",
        ))
        .is_err(),
        "changed copied btrt must fail complete-output qualification"
    );
}
