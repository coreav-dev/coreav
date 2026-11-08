//! Decodes generated fixtures through an `ffmpeg` binary and compares the
//! frames with FFmpeg reading the file itself. Needs `ffmpeg` on `PATH`
//! (with libx264), so it is ignored by default; CI runs it in the fixtures
//! workflow.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use std::path::{Path, PathBuf};
use std::process::Command;

use coreav_backend_ffmpeg::{FfmpegDecoder, FfmpegOptions};
use coreav_decode::{Codec, CodecConfig, ConfigKind, Decoder, Limits, Packet, StreamConfig};
use coreav_isobmff::{Allowlist, Budget, FileSource, Source, probe};

fn ffmpeg() -> PathBuf {
    test_oracle::require().ffmpeg().to_path_buf()
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

fn decode(file: &Path, codec: Codec, kind: ConfigKind) -> (Vec<u8>, Vec<i64>) {
    let mut source = FileSource::open(file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(codec)
    };
    let options = FfmpegOptions {
        program: ffmpeg(),
        threads: None,
    };
    let mut decoder = FfmpegDecoder::open(&options, &stream, &Limits::default()).expect("open");
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    let mut out = Vec::new();
    let mut pts = Vec::new();
    for p in &packets {
        let mut data = vec![0u8; p.size as usize];
        source.read_exact_at(p.offset, &mut data).expect("read");
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(p.cts),
                dts: Some(p.dts),
                keyframe: p.keyframe,
                discard: false,
            })
            .expect("send");
        while let Some(frame) = decoder.receive_frame().expect("receive") {
            pts.push(frame.pts.expect("pts"));
            frame
                .to_cpu()
                .expect("cpu")
                .write_packed(&mut out)
                .expect("packed");
        }
    }
    decoder.send_eof().expect("eof");
    while let Some(frame) = decoder.receive_frame().expect("receive") {
        pts.push(frame.pts.expect("pts"));
        frame
            .to_cpu()
            .expect("cpu")
            .write_packed(&mut out)
            .expect("packed");
    }
    (out, pts)
}

#[test]
#[ignore = "needs pinned ffmpeg"]
fn ffv1_odd_dimensions_match_source_bytes() {
    let dir = std::env::temp_dir().join(format!("coreav-ffv1-decode-{}", std::process::id()));
    std::fs::create_dir(&dir).expect("dir");
    for (format, vertical) in [("yuv420p", 2usize), ("yuv422p", 1)] {
        for (width, height) in [(854usize, 480usize), (640, 483), (855, 483)] {
            let frame_bytes = width * height + 2 * width.div_ceil(2) * height.div_ceil(vertical);
            let original: Vec<u8> = (0..frame_bytes * 2)
                .map(|index| (index.wrapping_mul(73) ^ (index >> 9)) as u8)
                .collect();
            let raw = dir.join("source.raw");
            std::fs::write(&raw, &original).expect("source");
            let file = generate(
                &dir,
                &format!("{format}-{width}-{height}.mkv"),
                &[
                    "-f",
                    "rawvideo",
                    "-pixel_format",
                    format,
                    "-video_size",
                    &format!("{width}x{height}"),
                    "-framerate",
                    "25",
                    "-i",
                    raw.to_str().expect("path"),
                    "-frames:v",
                    "2",
                    "-c:v",
                    "ffv1",
                    "-level",
                    "3",
                    "-coder",
                    "1",
                    "-slices",
                    "9",
                    "-slicecrc",
                    "1",
                    "-threads",
                    "1",
                ],
            );
            let bytes = std::fs::read(file).expect("encoded file");
            let segment = coreav_mkv::probe(
                coreav_mkv::SliceSource::new(&bytes),
                &Default::default(),
                &coreav_mkv::Allowlist::new(&[coreav_mkv::Codec::Ffv1]),
            )
            .expect("Matroska");
            assert_eq!(segment.tracks.len(), 1);
            let stream = StreamConfig {
                width: Some(width as u32),
                height: Some(height as u32),
                config: Some(CodecConfig {
                    kind: ConfigKind::MatroskaPrivate,
                    data: segment.tracks[0]
                        .codec_private
                        .clone()
                        .expect("configuration"),
                }),
                ..StreamConfig::new(Codec::Ffv1)
            };
            let options = FfmpegOptions {
                program: ffmpeg(),
                threads: Some(2),
            };
            let mut decoder =
                FfmpegDecoder::open(&options, &stream, &Limits::default()).expect("open");
            let packets = segment
                .packets(coreav_mkv::SliceSource::new(&bytes), &Default::default())
                .collect::<Result<Vec<_>, _>>()
                .expect("packets");
            assert_eq!(packets.len(), 2);
            let mut decoded = Vec::new();
            for packet in packets.iter().map(Some).chain(std::iter::once(None)) {
                if let Some(packet) = packet {
                    assert_eq!(packet.stripped, 0);
                    let start = packet.offset as usize;
                    decoder
                        .send_packet(&Packet {
                            data: &bytes[start..start + packet.stored_size as usize],
                            pts: packet.pts,
                            dts: packet.dts,
                            keyframe: packet.keyframe,
                            discard: false,
                        })
                        .expect("send");
                } else {
                    decoder.send_eof().expect("eof");
                }
                while let Some(frame) = decoder.receive_frame().expect("receive") {
                    let picture = frame.to_cpu().expect("cpu");
                    assert_eq!(
                        (picture.width(), picture.height()),
                        (width as u32, height as u32)
                    );
                    picture.write_packed(&mut decoded).expect("packed");
                }
            }
            assert_eq!(decoded.len(), original.len());
            assert!(
                decoded == original,
                "{format} {width}x{height}: decoded pixels differ"
            );
        }
    }
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
#[ignore = "needs ffmpeg"]
fn h264_with_b_frames_matches_ffmpeg() {
    let dir = std::env::temp_dir().join(format!("coreav-ffmpeg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = generate(
        &dir,
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
    );
    let (ours, pts) = decode(&file, Codec::H264, ConfigKind::Avcc);
    let theirs = reference(&file);
    assert_eq!(ours.len(), theirs.len());
    assert!(ours == theirs, "decoded bytes differ");
    let mut sorted = pts.clone();
    sorted.sort_unstable();
    assert_eq!(pts, sorted, "frames come out in presentation order");
    let ten = generate(
        &dir,
        "ten.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x96:rate=25",
            "-t",
            "1",
            "-c:v",
            "libx265",
            "-x265-params",
            "log-level=error",
            "-pix_fmt",
            "yuv420p10le",
            "-tag:v",
            "hvc1",
        ],
    );
    let (ours, _) = decode(&ten, Codec::Hevc, ConfigKind::Hvcc);
    assert!(ours == reference(&ten), "10-bit HEVC differs");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The coded pictures of `file`, without the bitstream's cropping, as
/// planar 4:2:0 at `width` by `height`.
fn uncropped(file: &Path) -> Vec<u8> {
    let out = Command::new(ffmpeg())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-apply_cropping",
            "0",
            "-i",
        ])
        .arg(file)
        .args(["-map", "0:v:0", "-fps_mode", "passthrough"])
        .args(["-pix_fmt", "yuv420p", "-f", "rawvideo", "-"])
        .output()
        .expect("ffmpeg runs");
    assert!(out.status.success());
    out.stdout
}

#[test]
#[ignore = "needs ffmpeg"]
fn h264_left_crop_is_applied_as_the_bitstream_says() {
    // A left crop of 48 samples (frame_crop_left_offset 24, clause
    // 7.4.2.1.1), which FFmpeg 7 skips by default to keep its planes
    // aligned: the frames must be columns 48 to 175 of the coded picture.
    let dir = std::env::temp_dir().join(format!("coreav-ffmpeg-crop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let coded = generate(
        &dir,
        "coded.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=176x144:rate=25",
            "-t",
            "0.4",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let coded = coded.to_str().expect("utf-8 path").to_owned();
    let file = generate(
        &dir,
        "crop.mp4",
        &[
            "-i",
            &coded,
            "-c",
            "copy",
            "-bsf:v",
            "h264_metadata=crop_left=48",
        ],
    );
    let (ours, _) = decode(&file, Codec::H264, ConfigKind::Avcc);
    let full = uncropped(&file);
    let (w, h, left) = (176, 144, 48);
    let frame = w * h * 3 / 2;
    assert!(!full.is_empty() && full.len() % frame == 0);
    let mut expected = Vec::new();
    for f in full.chunks(frame) {
        let (y, uv) = f.split_at(w * h);
        for row in y.chunks(w) {
            expected.extend_from_slice(&row[left..]);
        }
        for row in uv.chunks(w / 2) {
            expected.extend_from_slice(&row[left / 2..]);
        }
    }
    assert_eq!(ours.len(), expected.len(), "frames 128 samples wide");
    assert!(ours == expected, "frames are the cropped window");
    let _ = std::fs::remove_dir_all(&dir);
}
