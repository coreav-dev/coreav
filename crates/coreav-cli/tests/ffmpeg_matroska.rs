//! Command-level Matroska output checked with the pinned black-box oracle.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;

fn run(command: &mut Command) -> Vec<u8> {
    let Output {
        status,
        stdout,
        stderr,
    } = command.output().unwrap();
    assert!(
        status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&stderr)
    );
    stdout
}

fn tools() -> Option<(PathBuf, PathBuf)> {
    let pair = test_oracle::require();
    Some((pair.ffmpeg().to_path_buf(), pair.ffprobe().to_path_buf()))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ffmpeg-matroska-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(ffmpeg: &Path, path: &Path, video: &str, audio: &str) {
    run(Command::new(ffmpeg)
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=32x32:r=25:d=0.24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.24",
            "-c:v",
            video,
            "-threads:v",
            "1",
            "-c:a",
            audio,
            "-y",
        ])
        .arg(path));
}

fn core(
    input: &Path,
    output: &Path,
    args: &[&str],
    ffmpeg: &Path,
    strict: bool,
) -> std::process::Output {
    let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .arg("ffmpeg")
        .arg("-i")
        .arg(input)
        .args(args)
        .arg(output)
        .env("COREAV_FFMPEG", ffmpeg)
        .env("COREAV_STRICT_SAFE", if strict { "1" } else { "0" })
        .env("COREAV_FALLBACK", "off")
        .output()
        .unwrap();
    if std::env::var_os("COREAV_TEST_MUTATE_MATROSKA").is_some()
        && output.extension().is_none_or(|s| s != "mp4")
        && output.file_name().is_none_or(|s| s != "explicit.mkv")
    {
        if result.status.success() {
            let mut bytes = std::fs::read(output).unwrap();
            let segment = coreav_mkv::probe(
                coreav_mkv::SliceSource::new(&bytes),
                &coreav_mkv::Limits::default(),
                &coreav_mkv::Allowlist::any(),
            )
            .unwrap();
            let packet = segment
                .packets(
                    coreav_mkv::SliceSource::new(&bytes),
                    &coreav_mkv::Limits::default(),
                )
                .next()
                .unwrap()
                .unwrap();
            bytes[packet.offset as usize] ^= 1;
            std::fs::write(output, bytes).unwrap();
        } else {
            std::fs::write(output, b"unexpected output").unwrap();
        }
    }
    result
}

fn payloads(ffprobe: &Path, path: &Path) -> std::collections::BTreeMap<u64, Vec<String>> {
    let value: Value = serde_json::from_slice(&run(Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_packets",
            "-show_data_hash",
            "sha256",
            "-show_entries",
            "packet=stream_index,data_hash:packet_side_data=",
            "-of",
            "json",
        ])
        .arg(path)))
    .unwrap();
    let mut tracks = std::collections::BTreeMap::<u64, Vec<String>>::new();
    for packet in value["packets"].as_array().unwrap() {
        tracks
            .entry(packet["stream_index"].as_u64().unwrap())
            .or_default()
            .push(packet["data_hash"].as_str().unwrap().to_owned());
    }
    tracks
}

fn success(result: std::process::Output) {
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    assert!(
        result.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn decoded(ffmpeg: &Path, path: &Path, audio: bool) -> Vec<u8> {
    let mut command = Command::new(ffmpeg);
    command
        .args(["-v", "error"])
        .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
        .arg("-i")
        .arg(path);
    if audio {
        command.args(["-map", "0:a:0", "-c:a", "pcm_s16le", "-f", "s16le", "-"]);
    } else {
        command.args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "rawvideo",
            "-",
        ]);
    }
    run(&mut command)
}

fn fields(ffprobe: &Path, path: &Path) -> Value {
    serde_json::from_slice(&run(Command::new(ffprobe).args(["-v","error","-show_entries","stream=codec_name,codec_type,width,height,pix_fmt,sample_aspect_ratio,sample_fmt,sample_rate,channels,channel_layout,bits_per_raw_sample,extradata_size,time_base,r_frame_rate,color_range,color_space,color_transfer,color_primaries,chroma_location,field_order","-of","json"]).arg(path))).unwrap()
}

fn packets(ffprobe: &Path, path: &Path) -> Vec<String> {
    let bytes = run(Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_packets",
            "-show_entries",
            "packet=stream_index,pts_time,duration_time,flags",
            "-of",
            "compact",
        ])
        .arg(path));
    let mut rows: Vec<_> = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    rows.sort();
    rows
}

fn equivalent(ffmpeg: &Path, ffprobe: &Path, actual: &Path, expected: &Path, audio: bool) {
    assert_eq!(
        fields(ffprobe, actual),
        fields(ffprobe, expected),
        "stream fields"
    );
    assert_eq!(
        packets(ffprobe, actual),
        packets(ffprobe, expected),
        "packet fields"
    );
    assert_eq!(
        decoded(ffmpeg, actual, false),
        decoded(ffmpeg, expected, false),
        "decoded video"
    );
    if audio {
        assert_eq!(
            decoded(ffmpeg, actual, true),
            decoded(ffmpeg, expected, true),
            "decoded audio"
        );
    }
}

#[test]
fn copy_to_matroska_and_webm_matches_the_oracle() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("copy");
    for (name, video, audio, extension) in [
        ("iso-aac", "libx264", "aac", "mp4"),
        ("mkv-aac", "libx264", "aac", "mkv"),
        ("mkv-opus", "libx264", "libopus", "mkv"),
        ("webm", "libvpx-vp9", "libopus", "webm"),
    ] {
        let input = dir.join(format!("{name}.{extension}"));
        fixture(&ffmpeg, &input, video, audio);
        let output_extension = if extension == "webm" { "webm" } else { "mkv" };
        let actual = dir.join(format!("{name}-actual.{output_extension}"));
        let expected = dir.join(format!("{name}-expected.{output_extension}"));
        success(core(&input, &actual, &["-c", "copy"], &ffmpeg, true));
        run(Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&input)
            .args(["-c", "copy"])
            .arg(&expected));
        equivalent(&ffmpeg, &ffprobe, &actual, &expected, true);
        assert_eq!(payloads(&ffprobe, &actual), payloads(&ffprobe, &expected));
    }
}

#[test]
fn aac_copy_keeps_interior_samples_when_the_container_clock_shortens_a_packet() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("aac-interior-clock");
    let source = dir.join("source.mkv");
    let input = dir.join("input.mp4");
    fixture(&ffmpeg, &source, "libx264", "aac");
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args(["-c", "copy"])
        .arg(&input));
    let bytes = std::fs::read(&input).unwrap();
    let movie = coreav_isobmff::probe(
        coreav_isobmff::SliceSource::new(&bytes),
        &coreav_isobmff::Limits::default(),
        &coreav_isobmff::Allowlist::listing(),
    )
    .unwrap();
    let audio = movie
        .tracks
        .iter()
        .find(|t| t.kind == coreav_isobmff::TrackKind::Audio)
        .unwrap();
    let mut budget = coreav_isobmff::Budget::new(&coreav_isobmff::Limits::default());
    let packets: Vec<_> = movie
        .packets(
            audio.index,
            &mut coreav_isobmff::SliceSource::new(&bytes),
            &coreav_isobmff::Limits::default(),
            &mut budget,
        )
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(packets[1].duration, 1016);
    assert_eq!(decoded(&ffmpeg, &input, true).len(), 11_520 * 2);
    let actual = dir.join("actual.mkv");
    let expected = dir.join("expected.mkv");
    success(core(&input, &actual, &["-c", "copy"], &ffmpeg, true));
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-c", "copy"])
        .arg(&expected));
    assert_eq!(decoded(&ffmpeg, &actual, true).len(), 11_520 * 2);
    equivalent(&ffmpeg, &ffprobe, &actual, &expected, true);
    assert_eq!(payloads(&ffprobe, &actual), payloads(&ffprobe, &input));
}

#[test]
fn matroska_container_colour_survives_copy_without_changing_packets() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("colour-copy");
    for (range, extension) in [("tv", "mkv"), ("pc", "mkv"), ("tv", "mp4"), ("pc", "mp4")] {
        let name = format!("{range}-{extension}");
        let source = dir.join(format!("{name}-source.{extension}"));
        fixture(&ffmpeg, &source, "libx264", "aac");
        let input = dir.join(format!("{name}.{extension}"));
        run(Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&source)
            .args([
                "-c",
                "copy",
                "-color_range",
                range,
                "-colorspace",
                "bt709",
                "-color_primaries",
                "bt2020",
                "-color_trc",
                "smpte2084",
            ])
            .arg(&input));
        let declared = fields(&ffprobe, &input);
        assert_eq!(declared["streams"][0]["color_range"], range);
        assert_eq!(declared["streams"][0]["color_space"], "bt709");
        assert_eq!(declared["streams"][0]["color_primaries"], "bt2020");
        assert_eq!(declared["streams"][0]["color_transfer"], "smpte2084");
        assert_eq!(payloads(&ffprobe, &source), payloads(&ffprobe, &input));
        if extension == "mkv" {
            let bytes = std::fs::read(&input).unwrap();
            let segment = coreav_mkv::probe(
                coreav_isobmff::SliceSource::new(&bytes),
                &coreav_mkv::Limits::default(),
                &coreav_mkv::Allowlist::any(),
            )
            .unwrap();
            assert_eq!(
                segment.tracks[0].colour,
                coreav_isobmff::Color {
                    primaries: Some(9),
                    transfer: Some(16),
                    matrix: Some(1),
                    full_range: Some(range == "pc"),
                }
            );
        }
        let actual = dir.join(format!("{name}-actual.mkv"));
        let expected = dir.join(format!("{name}-expected.mkv"));
        success(core(&input, &actual, &["-c", "copy"], &ffmpeg, true));
        run(Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&input)
            .args(["-c", "copy"])
            .arg(&expected));
        equivalent(&ffmpeg, &ffprobe, &actual, &expected, true);
        assert_eq!(payloads(&ffprobe, &actual), payloads(&ffprobe, &input));
    }
}

#[test]
fn explicit_formats_and_audio_only_extension_select_the_container() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("formats");
    let input = dir.join("in.mp4");
    fixture(&ffmpeg, &input, "libx264", "aac");
    let actual = dir.join("out.bin");
    let expected = dir.join("expected.mkv");
    success(core(
        &input,
        &actual,
        &["-c", "copy", "-f", "matroska"],
        &ffmpeg,
        true,
    ));
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-c", "copy"])
        .arg(&expected));
    equivalent(&ffmpeg, &ffprobe, &actual, &expected, true);
    assert_eq!(payloads(&ffprobe, &actual), payloads(&ffprobe, &expected));
    let actual = dir.join("audio.mka");
    let expected = dir.join("expected.mka");
    success(core(&input, &actual, &["-c", "copy"], &ffmpeg, true));
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-c", "copy"])
        .arg(&expected));
    assert_eq!(fields(&ffprobe, &actual), fields(&ffprobe, &expected));
    assert_eq!(payloads(&ffprobe, &actual), payloads(&ffprobe, &expected));
    assert_eq!(
        decoded(&ffmpeg, &actual, true),
        decoded(&ffmpeg, &expected, true)
    );
}

#[test]
fn iso_opus_keeps_the_declared_edit_end_instead_of_decoding_the_padded_tail() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("iso-opus");
    let input = dir.join("in.mp4");
    fixture(&ffmpeg, &input, "libx264", "libopus");
    let actual = dir.join("actual.mkv");
    let copied = dir.join("oracle.mkv");
    success(core(&input, &actual, &["-c", "copy"], &ffmpeg, true));
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-c", "copy"])
        .arg(&copied));
    let bytes = std::fs::read(&input).unwrap();
    let movie = coreav_isobmff::probe(
        coreav_isobmff::SliceSource::new(&bytes),
        &coreav_isobmff::Limits::default(),
        &coreav_isobmff::Allowlist::listing(),
    )
    .unwrap();
    let audio = movie
        .tracks
        .iter()
        .find(|t| t.kind == coreav_isobmff::TrackKind::Audio)
        .unwrap();
    let (_, end) = audio.presentation(movie.timescale).unwrap();
    assert_eq!(end.num * 48_000, i128::from(end.den) * 11_520);
    let output = decoded(&ffmpeg, &actual, true);
    let oracle = decoded(&ffmpeg, &copied, true);
    assert_eq!(output.len(), 11_520 * 2);
    assert_eq!(oracle.len(), output.len() + 648 * 2);
    assert_eq!(output, oracle[..output.len()]);
    assert_eq!(fields(&ffprobe, &actual), fields(&ffprobe, &copied));
    assert_eq!(payloads(&ffprobe, &actual), payloads(&ffprobe, &copied));
    assert_eq!(
        decoded(&ffmpeg, &actual, false),
        decoded(&ffmpeg, &copied, false)
    );
    assert!(
        packets(&ffprobe, &actual)
            .last()
            .unwrap()
            .contains("discard_padding=648")
    );
}

#[test]
fn iso_opus_edit_starting_before_preskip_keeps_its_leading_samples() {
    let Some((ffmpeg, _)) = tools() else {
        return;
    };
    let dir = scratch("iso-opus-media-time");
    let source = dir.join("source.mp4");
    run(Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.24",
            "-c:a",
            "libopus",
            "-y",
        ])
        .arg(&source));
    let original = std::fs::read(&source).unwrap();
    let body = |name: &[u8]| original.windows(4).position(|w| w == name).unwrap() + 4;
    let (elst, dops) = (body(b"elst"), body(b"dOps"));
    assert_eq!(original[elst..elst + 8], [0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(original[elst + 12..elst + 16], 312u32.to_be_bytes());
    assert_eq!(original[dops + 2..dops + 4], 312u16.to_be_bytes());
    let variant = |name: &str, edit: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = original.clone();
        edit(&mut bytes);
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let edited = variant("edited.mp4", &|b| b[elst + 12..elst + 16].fill(0));
    let complete = variant("complete.mp4", &|b| {
        b[elst - 12..elst - 8].copy_from_slice(b"free");
        b[dops + 2..dops + 4].fill(0);
    });
    let complete = decoded(&ffmpeg, &complete, true);
    let copy = |input: &Path, name: &str| {
        let output = dir.join(name);
        success(core(input, &output, &["-c", "copy"], &ffmpeg, true));
        decoded(&ffmpeg, &output, true)
    };
    assert_eq!(copy(&source, "control.mkv"), complete[312 * 2..11_832 * 2]);
    let actual = copy(&edited, "actual.mkv");
    assert_eq!(actual, complete[..11_520 * 2]);
    let copied = dir.join("oracle.mkv");
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&edited)
        .args(["-c", "copy"])
        .arg(&copied));
    assert_eq!(decoded(&ffmpeg, &copied, true), actual[312 * 2..]);
}

#[cfg(feature = "native-h264")]
#[test]
fn h264_transcode_writes_matroska_with_copied_audio() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("encode");
    let input = dir.join("in.mp4");
    fixture(&ffmpeg, &input, "libx264", "aac");
    let args = [
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "23",
        "-threads",
        "1",
        "-c:a",
        "copy",
    ];
    let actual = dir.join("actual.mkv");
    let expected = dir.join("expected.mkv");
    success(core(&input, &actual, &args, &ffmpeg, false));
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(args)
        .arg(&expected));
    let mut actual_fields = fields(&ffprobe, &actual);
    let mut expected_fields = fields(&ffprobe, &expected);
    actual_fields["streams"][0]
        .as_object_mut()
        .unwrap()
        .remove("extradata_size");
    expected_fields["streams"][0]
        .as_object_mut()
        .unwrap()
        .remove("extradata_size");
    assert_eq!(actual_fields, expected_fields);
    assert_eq!(
        payloads(&ffprobe, &actual).get(&1),
        payloads(&ffprobe, &expected).get(&1)
    );
    assert_eq!(packets(&ffprobe, &actual), packets(&ffprobe, &expected));
    assert_eq!(
        decoded(&ffmpeg, &actual, false),
        decoded(&ffmpeg, &expected, false)
    );
    assert_eq!(
        decoded(&ffmpeg, &actual, true),
        decoded(&ffmpeg, &expected, true)
    );
}

#[test]
fn unsupported_webm_codecs_and_trimming_leave_no_output() {
    let Some((ffmpeg, _)) = tools() else {
        return;
    };
    let dir = scratch("refuse");
    let input = dir.join("in.mp4");
    fixture(&ffmpeg, &input, "libx264", "aac");
    for (name, args) in [
        ("codec", vec!["-c", "copy"]),
        ("trim", vec!["-c", "copy", "-t", "0.1"]),
    ] {
        let output = dir.join(format!("{name}.webm"));
        let result = core(&input, &output, &args, &ffmpeg, true);
        assert!(!result.status.success());
        assert!(!output.exists());
    }
}

#[cfg(all(feature = "native-h264", feature = "libopus"))]
#[test]
fn audio_replacements_keep_sample_exact_priming_and_padding_in_matroska() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("audio-encode");
    let input = dir.join("in.mkv");
    fixture(&ffmpeg, &input, "libx264", "flac");
    for codec in ["libopus", "aac"] {
        let actual = dir.join(format!("{codec}.mkv"));
        let baseline = dir.join(format!("{codec}.mp4"));
        let args = [
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-threads",
            "1",
            "-c:a",
            codec,
        ];
        success(core(&input, &actual, &args, &ffmpeg, false));
        let audio = decoded(&ffmpeg, &actual, true);
        success(core(&input, &baseline, &args, &ffmpeg, false));
        let baseline_audio = decoded(&ffmpeg, &baseline, true);
        assert_eq!(audio, baseline_audio[..11_520 * 2], "{codec}");
        assert_eq!(audio.len(), 11_520 * 2, "{codec}");
        let video = decoded(&ffmpeg, &actual, false);
        assert_eq!(video.len(), 6 * 32 * 32 * 3 / 2);
        let streams = fields(&ffprobe, &actual);
        assert_eq!(
            streams["streams"][1]["codec_name"],
            if codec == "libopus" { "opus" } else { "aac" }
        );
    }
}

#[cfg(feature = "native-h264")]
#[test]
fn matroska_default_video_encoder_matches_the_explicit_selection() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("defaults");
    let input = dir.join("in.mp4");
    fixture(&ffmpeg, &input, "libx264", "aac");
    let implicit = dir.join("implicit.mkv");
    let explicit = dir.join("explicit.mkv");
    success(core(
        &input,
        &implicit,
        &["-c:a", "copy", "-preset", "ultrafast", "-threads", "1"],
        &ffmpeg,
        false,
    ));
    success(core(
        &input,
        &explicit,
        &[
            "-c:v",
            "libx264",
            "-c:a",
            "copy",
            "-preset",
            "ultrafast",
            "-threads",
            "1",
        ],
        &ffmpeg,
        false,
    ));
    equivalent(&ffmpeg, &ffprobe, &implicit, &explicit, true);
    assert_eq!(payloads(&ffprobe, &implicit), payloads(&ffprobe, &explicit));
    for extension in ["mkv", "mka", "webm"] {
        let out = dir.join(format!("refused.{extension}"));
        let result = core(&input, &out, &[], &ffmpeg, true);
        assert!(!result.status.success());
        assert!(!out.exists());
    }
}

#[test]
fn bitmap_subtitles_follow_the_oracles_automatic_and_explicit_selection() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("bitmap-subtitles");
    let input = dir.join("in.mkv");
    let subtitle = mkv::el(
        mkv::TRACK_ENTRY,
        &mkv::cat(&[
            mkv::uint(0xd7, 2),
            mkv::uint(0x73c5, 2),
            mkv::uint(0x83, 17),
            mkv::string(0x86, "S_HDMV/PGS"),
        ]),
    );
    std::fs::write(
        &input,
        mkv::file(
            "matroska",
            &[
                mkv::info(Some(20.0)),
                mkv::el(
                    mkv::TRACKS,
                    &mkv::cat(&[
                        mkv::el(
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
                            ]),
                        ),
                        subtitle,
                    ]),
                ),
                mkv::cluster(
                    0,
                    &[
                        mkv::simple_block(1, 0, true, &vec![0; 1920]),
                        mkv::simple_block(2, 0, true, &[0x80, 0, 0]),
                    ],
                ),
            ],
        ),
    )
    .unwrap();
    let expected = dir.join("oracle.mkv");
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-c", "copy"])
        .arg(&expected));
    let declared = fields(&ffprobe, &expected);
    assert_eq!(declared["streams"].as_array().unwrap().len(), 1);
    let explicit = dir.join("explicit.mkv");
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-c", "copy", "-c:s", "copy"])
        .arg(&explicit));
    let declared = fields(&ffprobe, &explicit);
    assert_eq!(declared["streams"].as_array().unwrap().len(), 2);
    assert_eq!(declared["streams"][1]["codec_name"], "hdmv_pgs_subtitle");
    for extension in ["mkv", "mka", "webm"] {
        let output = dir.join(format!("refused.{extension}"));
        let result = core(
            &input,
            &output,
            &["-c", "copy", "-c:s", "copy"],
            &ffmpeg,
            true,
        );
        assert_eq!(result.status.code(), Some(4));
        assert!(!output.exists());
        assert!(String::from_utf8_lossy(&result.stderr).contains("subtitle"));
    }
    for (name, args) in [
        ("automatic", vec!["-c", "copy"]),
        ("indexed-kind", vec!["-c", "copy", "-c:s:0", "copy"]),
        ("indexed-stream", vec!["-c", "copy", "-c:1", "copy"]),
        ("disabled", vec!["-c", "copy", "-sn"]),
        ("mapped", vec!["-map", "0:a", "-c", "copy"]),
    ] {
        let output = dir.join(format!("{name}.mkv"));
        success(core(&input, &output, &args, &ffmpeg, true));
        assert_eq!(
            fields(&ffprobe, &output)["streams"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            decoded(&ffmpeg, &output, true),
            decoded(&ffmpeg, &input, true)
        );
    }
}

#[test]
fn automatic_text_subtitles_are_refused_unless_explicitly_excluded() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let dir = scratch("subtitles");
    let subtitles = dir.join("in.srt");
    std::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:00,200\ntext\n").unwrap();
    let input = dir.join("in.mkv");
    run(Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000:duration=0.24",
            "-i",
        ])
        .arg(&subtitles)
        .args(["-c:a", "pcm_s16le", "-c:s", "srt"])
        .arg(&input));
    let expected = dir.join("oracle.mkv");
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args(["-c", "copy"])
        .arg(&expected));
    assert_eq!(
        fields(&ffprobe, &expected)["streams"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for extension in ["mkv", "mka", "webm"] {
        let output = dir.join(format!("refused.{extension}"));
        let result = core(&input, &output, &["-c", "copy"], &ffmpeg, true);
        assert_eq!(result.status.code(), Some(4));
        assert!(!output.exists());
        assert!(String::from_utf8_lossy(&result.stderr).contains("subtitle"));
    }
    for (name, args) in [
        ("disabled", vec!["-c", "copy", "-sn"]),
        ("mapped", vec!["-map", "0:a", "-c", "copy"]),
    ] {
        let output = dir.join(format!("{name}.mkv"));
        success(core(&input, &output, &args, &ffmpeg, true));
        assert_eq!(
            fields(&ffprobe, &output)["streams"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            decoded(&ffmpeg, &output, true),
            decoded(&ffmpeg, &input, true)
        );
    }
}
