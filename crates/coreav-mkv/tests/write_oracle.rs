//! Generated media written back and decoded by the pinned FFmpeg oracle.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use coreav_mkv::write::{CodecId, Media, Options, Packet, Track, plan};
use coreav_mkv::{Allowlist, DocType, Error, Limits, SliceSource, probe};
use std::path::Path;
use std::process::{Command, Output};

fn run(command: &mut Command) -> Vec<u8> {
    let Output {
        status,
        stdout,
        stderr,
    } = command.output().expect("run oracle");
    assert!(
        status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&stderr)
    );
    stdout
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

fn fields(ffprobe: &Path, path: &Path) -> Vec<u8> {
    run(Command::new(ffprobe).args(["-v", "error", "-show_entries",
        "stream=codec_name,codec_type,width,height,pix_fmt,sample_aspect_ratio,sample_fmt,sample_rate,channels,channel_layout,bits_per_raw_sample,extradata_size,time_base,r_frame_rate,color_range,color_space,color_transfer,color_primaries,chroma_location,field_order",
        "-of", "flat"]).arg(path))
}

fn packet_fields(ffprobe: &Path, path: &Path) -> Vec<String> {
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

fn rewrite(source: &[u8], doc_type: DocType) -> Vec<u8> {
    let limits = Limits::default();
    let segment = probe(SliceSource::new(source), &limits, &Allowlist::any()).unwrap();
    let tracks: Vec<_> = segment
        .tracks
        .iter()
        .map(|track| {
            let codec = match track.codec {
                coreav_mkv::Codec::H264 => CodecId::H264,
                coreav_mkv::Codec::Vp9 => CodecId::Vp9,
                coreav_mkv::Codec::Aac => CodecId::Aac,
                coreav_mkv::Codec::Opus => CodecId::Opus,
                _ => panic!("unexpected generated codec"),
            };
            let media = if let Some(v) = &track.video {
                Media::Video {
                    width: v.pixel_width.unwrap() as u32,
                    height: v.pixel_height.unwrap() as u32,
                    colour: track.colour,
                    interlaced: u8::try_from(v.interlaced).unwrap(),
                    field_order: u8::try_from(v.field_order).unwrap(),
                    display: v
                        .display_width
                        .zip(v.display_height)
                        .map(|(w, h)| (w as u32, h as u32)),
                }
            } else {
                let a = track.audio.unwrap();
                Media::Audio {
                    sample_rate: a.sampling_frequency as u32,
                    channels: a.channels as u32,
                    bit_depth: a.bit_depth.map(|d| d as u32),
                }
            };
            Track {
                codec,
                private: track.codec_private.clone().unwrap_or_default(),
                media,
                default_duration_ns: track.default_duration,
                codec_delay_ns: track.codec_delay,
                seek_pre_roll_ns: track.seek_pre_roll,
                language: track.language.clone(),
                name: track.name.clone(),
                default: track.default,
            }
        })
        .collect();
    let mut original: Vec<_> = segment
        .packets(SliceSource::new(source), &limits)
        .collect::<Result<_, _>>()
        .unwrap();
    if doc_type == DocType::Webm {
        original.sort_by_key(|p| p.block_timestamp);
    }
    let packets: Vec<_> = original
        .iter()
        .map(|p| {
            assert_eq!(p.frames_in_block, 1);
            assert_eq!(p.stripped, 0);
            let track = &tracks[p.track];
            let duration = p
                .block_duration
                .map(|d| d * segment.timestamp_scale + p.discard_padding.unwrap_or(0).max(0) as u64)
                .or(match track.codec {
                    CodecId::Aac => match track.media {
                        Media::Audio { sample_rate, .. } => {
                            Some(1_024_000_000_000 / u64::from(sample_rate))
                        }
                        _ => None,
                    },
                    CodecId::Opus => Some(20_000_000),
                    _ => None,
                })
                .or(track.default_duration_ns);
            Packet {
                track: p.track,
                pts_ns: p.block_timestamp * segment.timestamp_scale as i64
                    - track.codec_delay_ns as i64,
                duration_ns: duration,
                size: p.stored_size,
                keyframe: p.keyframe,
                invisible: p.invisible,
                discard_padding_ns: p.discard_padding,
            }
        })
        .collect();
    let layout = plan(
        &tracks,
        &packets,
        Options {
            doc_type,
            timestamp_scale_ns: segment.timestamp_scale,
        },
        &limits,
    )
    .unwrap();
    let mut out = Vec::new();
    layout
        .write(&mut out, |index, out| {
            let p = &original[index];
            out.write_all(&source[p.offset as usize..(p.offset + p.stored_size) as usize])
                .map_err(|e| Error::Io(e.kind()))
        })
        .unwrap();
    out
}

#[test]
fn encoded_video_and_audio_decode_unchanged() {
    let pair = test_oracle::require();
    let ffmpeg = pair.ffmpeg().to_path_buf();
    let ffprobe = pair.ffprobe().to_path_buf();
    let dir = std::env::temp_dir().join(format!("coreav-mkv-write-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (video, audio, doc_type, ext) in [
        ("libx264", "aac", DocType::Matroska, "mkv"),
        ("libx264", "libopus", DocType::Matroska, "mkv"),
        ("libvpx-vp9", "libopus", DocType::Webm, "webm"),
    ] {
        let input = dir.join(format!("{video}-{audio}-input.{ext}"));
        let output = dir.join(format!("{video}-{audio}-output.{ext}"));
        run(Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=25:duration=0.4",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=731:sample_rate=48000:duration=0.4",
                "-c:v",
                video,
                "-c:a",
                audio,
                "-threads",
                "1",
            ])
            .arg(&input));
        let source = std::fs::read(&input).unwrap();
        std::fs::write(&output, rewrite(&source, doc_type)).unwrap();
        assert_eq!(
            fields(&ffprobe, &input),
            fields(&ffprobe, &output),
            "{video}/{audio} stream fields"
        );
        assert_eq!(
            packet_fields(&ffprobe, &input),
            packet_fields(&ffprobe, &output),
            "{video}/{audio} packet fields"
        );
        let reference_video = decoded(&ffmpeg, &input, false);
        let actual_video = decoded(&ffmpeg, &output, false);
        assert_eq!(reference_video.len(), 10 * 64 * 48 * 3 / 2);
        assert_eq!(
            actual_video, reference_video,
            "{video}/{audio} decoded frames"
        );
        let reference_audio = decoded(&ffmpeg, &input, true);
        assert!(!reference_audio.is_empty());
        assert_eq!(
            decoded(&ffmpeg, &output, true),
            reference_audio,
            "{video}/{audio} decoded samples"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
