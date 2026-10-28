//! The dav1d backend in-process. The tests against FFmpeg generate AV1
//! with an `ffmpeg` on `PATH` (libaom-av1 and libdav1d), so they are
//! ignored by default; CI runs them in the fixtures workflow. The others
//! need only libdav1d.
#![cfg(feature = "dav1d")]
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use coreav_backend_dav1d::{Dav1dDecoder, Dav1dOptions};
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
        .args(["-c:v", "libaom-av1", "-cpu-used", "8", "-g", "10"])
        .args(["-pix_fmt", pix_fmt])
        .args(extra)
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success());
    out
}

/// Every frame of the file's video, through FFmpeg's libdav1d.
fn reference(file: &Path) -> Vec<u8> {
    let out = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-c:v", "libdav1d"])
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
}

/// Decodes the file's first track; packets whose index `discard` accepts
/// are sent marked to discard.
fn decode(
    file: &Path,
    limits: &Limits,
    discard: impl Fn(usize) -> bool,
) -> Result<Decoded, ErrorKind> {
    let mut source = FileSource::open(file).expect("open");
    let movie = probe(&mut source, &Default::default(), &Allowlist::default()).expect("probe");
    let track = movie.tracks.first().expect("track");
    let stream = StreamConfig {
        config: track
            .description
            .codec_config
            .as_ref()
            .map(|c| CodecConfig {
                kind: ConfigKind::Av1c,
                data: c.data.clone(),
            }),
        width: track.description.width.map(u32::from),
        height: track.description.height.map(u32::from),
        ..StreamConfig::new(Codec::Av1)
    };
    let mut decoder =
        Dav1dDecoder::open(&Dav1dOptions::default(), &stream, limits).map_err(|e| e.kind)?;
    let mut budget = Budget::new(&Default::default());
    let packets: Vec<_> = movie
        .packets(0, &mut source, &Default::default(), &mut budget)
        .expect("packets")
        .collect::<Result<_, _>>()
        .expect("packet list");
    let mut out = Decoded {
        bytes: Vec::new(),
        pts: Vec::new(),
    };
    let take = |decoder: &mut Dav1dDecoder, out: &mut Decoded| -> Result<(), ErrorKind> {
        while let Some(frame) = decoder.receive_frame().map_err(|e| e.kind)? {
            out.pts.push(frame.pts.expect("pts"));
            frame
                .to_cpu()
                .expect("cpu")
                .write_packed(&mut out.bytes)
                .expect("packed");
        }
        Ok(())
    };
    for (index, p) in packets.iter().enumerate() {
        let mut data = vec![0u8; p.size as usize];
        source.read_exact_at(p.offset, &mut data).expect("read");
        decoder
            .send_packet(&Packet {
                data: &data,
                pts: Some(p.cts),
                dts: Some(p.dts),
                keyframe: p.keyframe,
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
    let dir = std::env::temp_dir().join(format!("coreav-dav1d-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

#[test]
#[ignore = "needs ffmpeg with libaom-av1 and libdav1d"]
fn frames_match_ffmpeg_libdav1d() {
    let dir = scratch("frames");
    for (name, size, pix_fmt, extra) in [
        ("420.mp4", "176x144", "yuv420p", &[][..]),
        ("420-10.mp4", "128x96", "yuv420p10le", &[]),
        ("444.mp4", "128x96", "yuv444p", &[]),
        ("odd.mp4", "75x51", "yuv420p", &[]),
        // Film grain parameters, which both sides apply.
        (
            "grain.mp4",
            "176x144",
            "yuv420p",
            &["-denoise-noise-level", "50"],
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
#[ignore = "needs ffmpeg with libaom-av1"]
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

    let small = Limits {
        max_pixels: 176 * 144 - 1,
        ..Limits::default()
    };
    assert!(
        decode(&file, &small, |_| false).is_err(),
        "frame size limit"
    );
    let decoded = Limits {
        max_decoded_bytes: (frame * 3) as u64,
        ..Limits::default()
    };
    assert_eq!(
        decode(&file, &decoded, |_| false).err(),
        Some(ErrorKind::Limit),
        "decoded-bytes limit"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn other_codecs_and_garbage_are_errors() {
    let h264 = StreamConfig::new(Codec::H264);
    assert_eq!(
        Dav1dDecoder::open(&Dav1dOptions::default(), &h264, &Limits::default())
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
        let stream = StreamConfig::new(Codec::Av1);
        let mut decoder = Dav1dDecoder::open(&Dav1dOptions::default(), &stream, &Limits::default())
            .expect("open");
        for i in 0..32 {
            let len = (next() % 256) as usize + round;
            let data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
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
