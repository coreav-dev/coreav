//! The libvpx backend in-process. The tests against FFmpeg encode VP9 with
//! an `ffmpeg` on `PATH` (libvpx-vp9) and decode it with FFmpeg's own VP9
//! decoder (`-c:v vp9`), so they are ignored by default; CI runs them in
//! the fixtures workflow. The others need only libvpx.
#![cfg(feature = "libvpx")]
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use coreav_backend_libvpx::{LibvpxDecoder, LibvpxOptions, frame_sizes};
use coreav_decode::{
    Codec, CodecConfig, ConfigKind, Decoder, ErrorKind, Limits, Packet, StreamConfig,
};
use coreav_isobmff::{Allowlist, Budget, FileSource, Source, probe};

fn ffmpeg() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|d| d.join("ffmpeg"))
        .find(|p| p.is_file())
        .expect("ffmpeg on PATH")
}

fn generate(dir: &Path, name: &str, size: &str, pix_fmt: &str, extra: &[&str]) -> PathBuf {
    let out = dir.join(name);
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", &format!("testsrc2=size={size}:rate=25"), "-t", "1"])
        .args(["-c:v", "libvpx-vp9", "-g", "10", "-b:v", "300k"])
        .args(["-pix_fmt", pix_fmt])
        .args(extra)
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "{name}: ffmpeg encodes");
    out
}

/// Every frame of the file's video, through FFmpeg's own VP9 decoder.
fn reference(file: &Path) -> Vec<u8> {
    let out = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-c:v", "vp9"])
        .args(["-ignore_editlist", "1", "-i"])
        .arg(file)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
        ])
        .arg("-")
        .output()
        .expect("ffmpeg runs");
    assert!(out.status.success());
    out.stdout
}

struct Decoded {
    bytes: Vec<u8>,
    pts: Vec<i64>,
    sizes: Vec<(u32, u32)>,
}

/// A packet of the file, read.
struct Sample {
    data: Vec<u8>,
    cts: i64,
    dts: i64,
    keyframe: bool,
}

/// The file's first track and its packets.
fn packets(file: &Path) -> (StreamConfig, Vec<Sample>) {
    let mut source = FileSource::open(file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind: ConfigKind::Vpcc,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(Codec::Vp9)
    };
    let mut budget = Budget::new(&Default::default());
    let list: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    let packets = list
        .iter()
        .map(|p| {
            let mut data = vec![0u8; p.size as usize];
            source.read_exact_at(p.offset, &mut data).expect("read");
            Sample {
                data,
                cts: p.cts,
                dts: p.dts,
                keyframe: p.keyframe,
            }
        })
        .collect();
    (stream, packets)
}

/// Decodes the file's first track; packets whose index `discard` accepts
/// are sent marked to discard.
fn decode(
    file: &Path,
    limits: &Limits,
    discard: impl Fn(usize) -> bool,
) -> Result<Decoded, ErrorKind> {
    let (stream, packets) = packets(file);
    let mut decoder =
        LibvpxDecoder::open(&LibvpxOptions::default(), &stream, limits).map_err(|e| e.kind)?;
    let mut out = Decoded {
        bytes: Vec::new(),
        pts: Vec::new(),
        sizes: Vec::new(),
    };
    let take = |decoder: &mut LibvpxDecoder, out: &mut Decoded| -> Result<(), ErrorKind> {
        while let Some(frame) = decoder.receive_frame().map_err(|e| e.kind)? {
            out.pts.push(frame.pts.expect("pts"));
            let picture = frame.to_cpu().expect("cpu");
            out.sizes.push((picture.width(), picture.height()));
            picture.write_packed(&mut out.bytes).expect("packed");
        }
        Ok(())
    };
    for (index, sample) in packets.iter().enumerate() {
        decoder
            .send_packet(&Packet {
                data: &sample.data,
                pts: Some(sample.cts),
                dts: Some(sample.dts),
                keyframe: sample.keyframe,
                discard: discard(index),
            })
            .map_err(|e| e.kind)?;
        take(&mut decoder, &mut out)?;
    }
    decoder.send_eof().map_err(|e| e.kind)?;
    take(&mut decoder, &mut out)?;
    Ok(out)
}

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("coreav-libvpx-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

#[test]
#[ignore = "needs ffmpeg with libvpx-vp9 and its own vp9 decoder"]
fn frames_match_ffmpeg_vp9() {
    let dir = scratch("frames");
    // Hidden alternate reference frames in superframes, as the two-pass
    // encodes of most VP9 files have them.
    let altref = &["-auto-alt-ref", "1", "-lag-in-frames", "16"][..];
    for (name, size, pix_fmt, extra) in [
        ("420.mp4", "176x144", "yuv420p", &[][..]),
        ("420-altref.mp4", "176x144", "yuv420p", altref),
        ("420-10.mp4", "128x96", "yuv420p10le", &[]),
        ("420-12.mp4", "128x96", "yuv420p12le", &[]),
        ("422.mp4", "128x96", "yuv422p", &[]),
        ("444.mp4", "128x96", "yuv444p", &[]),
        ("444-10.mp4", "128x96", "yuv444p10le", altref),
        ("odd.mp4", "75x51", "yuv420p", &[]),
        (
            "tiles.mp4",
            "1280x720",
            "yuv420p",
            &["-tile-columns", "2", "-cpu-used", "8"],
        ),
    ] {
        let file = generate(&dir, name, size, pix_fmt, extra);
        let ours = decode(&file, &Limits::default(), |_| false).expect("decodes");
        let theirs = reference(&file);
        assert_eq!(ours.bytes.len(), theirs.len(), "{name}: frame bytes");
        assert!(ours.bytes == theirs, "{name}: decoded bytes differ");
        assert_eq!(ours.pts.len(), 25, "{name}: frames");
        let mut sorted = ours.pts.clone();
        sorted.sort_unstable();
        assert_eq!(
            ours.pts, sorted,
            "{name}: frames come out in presentation order"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg with libvpx-vp9"]
fn stated_sizes_are_the_decoded_sizes() {
    let dir = scratch("sizes");
    let file = generate(
        &dir,
        "s.mp4",
        "176x144",
        "yuv420p",
        &["-auto-alt-ref", "1", "-lag-in-frames", "16"],
    );
    let (_, packets) = packets(&file);
    let decoded = decode(&file, &Limits::default(), |_| false).expect("decodes");
    let mut keyframes = 0;
    for sample in &packets {
        let stated = frame_sizes(&sample.data).expect("headers");
        if sample.keyframe {
            keyframes += 1;
            assert!(!stated.is_empty(), "a key frame states its size");
        }
        assert!(stated.iter().all(|s| (s.width, s.height) == (176, 144)));
    }
    assert_eq!(keyframes, 3);
    assert!(decoded.sizes.iter().all(|&s| s == (176, 144)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg with libvpx-vp9"]
fn discarded_packets_give_no_frame_and_limits_hold() {
    let dir = scratch("discard");
    let file = generate(&dir, "d.mp4", "176x144", "yuv420p", &[]);
    let all = decode(&file, &Limits::default(), |_| false).expect("decodes");
    let some = decode(&file, &Limits::default(), |i| i < 10).expect("decodes");
    assert_eq!(some.pts.len(), 15);
    assert_eq!(some.pts[..], all.pts[10..]);
    let frame = all.bytes.len() / 25;
    assert!(
        some.bytes[..] == all.bytes[10 * frame..],
        "the frames kept are the same"
    );
    // Refused from the stated size, before libvpx decodes the frame.
    for small in [
        Limits {
            max_pixels: 176 * 144 - 1,
            ..Limits::default()
        },
        Limits {
            max_width: 175,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            decode(&file, &small, |_| false).err(),
            Some(ErrorKind::Limit),
            "frame size limit"
        );
    }
    let decoded = Limits {
        max_decoded_bytes: (frame * 3) as u64,
        ..Limits::default()
    };
    assert_eq!(
        decode(&file, &decoded, |_| false).err(),
        Some(ErrorKind::Limit),
        "decoded-bytes limit"
    );
    let memory = Limits {
        max_memory_bytes: 64 << 20,
        ..Limits::default()
    };
    assert_eq!(
        decode(&file, &memory, |_| false).err(),
        Some(ErrorKind::Limit),
        "memory estimate"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_key_frame_too_large_for_the_limits_is_refused_before_libvpx() {
    // A key frame header stating 65,536 x 65,536 and nothing else.
    let header = [0x82, 0x49, 0x83, 0x42, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    let mut decoder = LibvpxDecoder::open(
        &LibvpxOptions::default(),
        &StreamConfig::new(Codec::Vp9),
        &Limits::default(),
    )
    .expect("open");
    let packet = Packet {
        data: &header,
        pts: Some(0),
        dts: Some(0),
        keyframe: true,
        discard: false,
    };
    assert_eq!(
        decoder.send_packet(&packet).err().map(|e| e.kind),
        Some(ErrorKind::Limit)
    );
}

#[test]
fn other_codecs_and_garbage_are_errors() {
    let h264 = StreamConfig::new(Codec::H264);
    assert_eq!(
        LibvpxDecoder::open(&LibvpxOptions::default(), &h264, &Limits::default())
            .err()
            .map(|e| e.kind),
        Some(ErrorKind::Unsupported)
    );
    // Bytes from a fixed generator, in packets of every small size: each
    // ends in frames or an error, never a panic or a hang.
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for round in 0..64 {
        let stream = StreamConfig::new(Codec::Vp9);
        let mut decoder =
            LibvpxDecoder::open(&LibvpxOptions::default(), &stream, &Limits::default())
                .expect("open");
        for i in 0..32 {
            let len = (next() % 256) as usize + round;
            let mut data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Half the packets start like a key frame, so that libvpx reads
            // past the header.
            if i % 2 == 0 && data.len() > 4 {
                data[..4].copy_from_slice(&[0x82, 0x49, 0x83, 0x42]);
            }
            let packet = Packet {
                data: &data,
                pts: Some(i),
                dts: Some(i),
                keyframe: i == 0,
                discard: false,
            };
            if decoder.send_packet(&packet).is_err() {
                break;
            }
            while let Ok(Some(_)) = decoder.receive_frame() {}
        }
        let _ = decoder.send_eof();
        while let Ok(Some(_)) = decoder.receive_frame() {}
    }
}
