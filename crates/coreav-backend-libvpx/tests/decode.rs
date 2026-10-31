//! The libvpx backend in-process. The tests against FFmpeg encode VP9 and
//! VP8 with an `ffmpeg` on `PATH` (libvpx-vp9, libvpx) and decode them with
//! FFmpeg's own decoders (`-c:v vp9`, `-c:v vp8`), so they are ignored by
//! default; CI runs them in the fixtures workflow. The others need only
//! libvpx.
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

use coreav_backend_libvpx::{LibvpxDecoder, LibvpxOptions, frame_sizes, vp8_frame_size};
use coreav_decode::{Codec, Decoder, ErrorKind, Limits, Packet, StreamConfig};
use coreav_isobmff::{FileSource, FourCc, Rational};
use coreav_isobmff_write::mux::{
    FileType, MovieHeader, Muxer, RawBox, Sample as MuxSample, TrackHeader, TrackMedia,
    visual_sample_entry,
};
use coreav_thumbnail::{ThumbnailOptions, first_packets};
use md5::{Digest, Md5};

fn ffmpeg() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|d| d.join("ffmpeg"))
        .find(|p| p.is_file())
        .expect("ffmpeg on PATH")
}

/// One second of `testsrc2` at 25 frames a second, encoded with
/// `encoder` (`libvpx-vp9` or `libvpx`, VP8) into `name`. The pattern
/// is drawn in 4:4:4, so an odd `size` stays odd: drawn in 4:2:0 it
/// would be rounded down to even.
fn generate(
    dir: &Path,
    name: &str,
    encoder: &str,
    size: &str,
    pix_fmt: &str,
    extra: &[&str],
) -> PathBuf {
    let out = dir.join(name);
    let source = format!("testsrc2=size={size}:rate=25,format=yuv444p");
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", &source, "-t", "1"])
        .args(["-c:v", encoder, "-g", "10", "-b:v", "300k"])
        .args(["-pix_fmt", pix_fmt])
        .args(extra)
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "{name}: ffmpeg encodes");
    out
}

/// A VP8 WebM file with hidden alternate reference frames: libvpx makes
/// them in its second pass only.
fn vp8_altref(dir: &Path) -> PathBuf {
    let log = dir.join("altref-pass");
    let log = log.to_str().expect("path");
    let pass = |n: &str, out: &str| {
        let args = [
            "-auto-alt-ref",
            "1",
            "-lag-in-frames",
            "16",
            "-pass",
            n,
            "-passlogfile",
            log,
            "-f",
            "webm",
        ];
        generate(dir, out, "libvpx", "176x144", "yuv420p", &args)
    };
    pass("1", "altref-pass1.webm");
    pass("2", "altref.webm")
}

/// A VP8 WebM file whose frames grow from 176x144 to 320x240 at a key
/// frame: two encodes joined without re-encoding.
fn vp8_resized(dir: &Path) -> PathBuf {
    let small = generate(dir, "small.ivf", "libvpx", "176x144", "yuv420p", &[]);
    let large = generate(dir, "large.ivf", "libvpx", "320x240", "yuv420p", &[]);
    let list = dir.join("join.txt");
    let line = |p: &Path| format!("file '{}'\n", p.display());
    std::fs::write(&list, line(&small) + &line(&large)).expect("list");
    let out = dir.join("resized.webm");
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "concat", "-safe", "0", "-i"])
        .arg(&list)
        .args(["-c", "copy"])
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "resized.webm: ffmpeg joins");
    out
}

/// The VP8 stream of a Matroska file in an MP4 file (`vp08`, the VP codec
/// ISOBMFF binding), written by CoreAV's MP4 writer: FFmpeg writes no VP8
/// in MP4. One sample per frame, times in milliseconds as in the source.
fn vp8_mp4(source: &Path, out: &Path) -> PathBuf {
    let (stream, packets) = packets(source);
    let (width, height) = (stream.width.expect("width"), stream.height.expect("height"));
    // `vpcC` version 1: profile 0, level 10, 8 bits, 4:2:0 colocated,
    // studio range, colour unspecified, no initialization data.
    let vpcc = vec![1, 0, 0, 0, 0, 10, 0x82, 2, 2, 2, 0, 0];
    let entry = visual_sample_entry(
        FourCc(*b"vp08"),
        u16::try_from(width).expect("width"),
        u16::try_from(height).expect("height"),
        &[0; 32],
        0x18,
        &[RawBox {
            kind: FourCc(*b"vpcC"),
            payload: vpcc,
        }],
    )
    .expect("sample entry");
    let limits = coreav_isobmff::Limits::default();
    let mut muxer = Muxer::new(
        FileType {
            major: FourCc(*b"isom"),
            minor: 0,
            compatible: vec![FourCc(*b"isom"), FourCc(*b"iso6")],
        },
        MovieHeader {
            timescale: 1000,
            creation_time: 0,
            boxes: Vec::new(),
        },
        &limits,
    )
    .expect("muxer");
    let track = muxer
        .add_track(TrackHeader {
            media: TrackMedia::Video {
                matrix: [0x1_0000, 0, 0, 0, 0x1_0000, 0, 0, 0, 0x4000_0000],
                width: width << 16,
                height: height << 16,
            },
            timescale: 1000,
            language: *b"und",
            creation_time: 0,
            media_creation_time: 0,
            flags: 3,
            layer: 0,
            alternate_group: 0,
            edits: Vec::new(),
            duration: None,
            media_duration: None,
            sample_entry: entry,
            sample_table_boxes: Vec::new(),
            boxes: Vec::new(),
        })
        .expect("track");
    for (i, p) in packets.iter().enumerate() {
        let next = packets.get(i + 1).map_or(p.pts + 40, |n| n.pts);
        muxer
            .push_sample(
                track,
                MuxSample {
                    size: u32::try_from(p.data.len()).expect("size"),
                    duration: u32::try_from(next - p.pts).expect("duration"),
                    sync: p.keyframe,
                    composition_offset: 0,
                },
            )
            .expect("sample");
    }
    muxer.finish_track(track);
    let mut file = Vec::new();
    muxer.write_head(&mut file).expect("head");
    let mut next = packets.iter();
    muxer
        .for_each_chunk::<coreav_isobmff::Error>(|_, samples| {
            for p in next.by_ref().take(samples as usize) {
                file.extend_from_slice(&p.data);
            }
            Ok(())
        })
        .expect("chunks");
    std::fs::write(out, &file).expect("write");
    out.to_path_buf()
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

/// FFmpeg's frames of the file's video, through its own decoder of
/// `codec` (`vp8`): each frame's pts in milliseconds, its size and the MD5
/// of its planes.
fn framemd5(file: &Path, codec: &str) -> Vec<(i64, usize, String)> {
    let out = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-c:v", codec, "-i"])
        .arg(file)
        .args(["-map", "0:v:0", "-fps_mode", "passthrough", "-noautoscale"])
        .args(["-enc_time_base:v", "1/1000", "-f", "framemd5", "-"])
        .output()
        .expect("ffmpeg runs");
    assert!(
        out.status.success(),
        "{}: {}",
        file.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("text");
    assert!(
        text.lines().any(|l| l.starts_with("#tb 0: 1/1000")),
        "{}: milliseconds",
        file.display()
    );
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let fields: Vec<&str> = l.split(',').map(str::trim).collect();
            (
                fields[2].parse().expect("pts"),
                fields[4].parse().expect("size"),
                fields[5].to_owned(),
            )
        })
        .collect()
}

struct Decoded {
    bytes: Vec<u8>,
    pts: Vec<i64>,
    sizes: Vec<(u32, u32)>,
    /// Each frame's size in bytes and the MD5 of its planes, as FFmpeg's
    /// `framemd5` gives them.
    md5: Vec<(usize, String)>,
}

/// A packet of the file, read.
struct Sample {
    data: Vec<u8>,
    pts: i64,
    dts: Option<i64>,
    keyframe: bool,
}

/// The file's video stream and its packets, as `coreav thumbnail` and the
/// decode oracle read them.
fn packets(file: &Path) -> (StreamConfig, Vec<Sample>) {
    let source = FileSource::open(file).expect("open");
    let options = ThumbnailOptions::at(Rational::from_int(0));
    let (_, stream, list) = first_packets(source, &options, usize::MAX).expect("packets");
    let packets = list
        .into_iter()
        .map(|p| Sample {
            data: p.data,
            pts: p.pts,
            dts: p.dts,
            keyframe: p.keyframe,
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
        md5: Vec::new(),
    };
    let take = |decoder: &mut LibvpxDecoder, out: &mut Decoded| -> Result<(), ErrorKind> {
        while let Some(frame) = decoder.receive_frame().map_err(|e| e.kind)? {
            out.pts.push(frame.pts.expect("pts"));
            let picture = frame.to_cpu().expect("cpu");
            out.sizes.push((picture.width(), picture.height()));
            let mut planes = Vec::new();
            picture.write_packed(&mut planes).expect("packed");
            out.md5
                .push((planes.len(), format!("{:x}", Md5::digest(&planes))));
            out.bytes.extend_from_slice(&planes);
        }
        Ok(())
    };
    for (index, sample) in packets.iter().enumerate() {
        decoder
            .send_packet(&Packet {
                data: &sample.data,
                pts: Some(sample.pts),
                dts: sample.dts,
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
        let file = generate(&dir, name, "libvpx-vp9", size, pix_fmt, extra);
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

/// Asserts that CoreAV's frames of `file` are FFmpeg's: as many, at the
/// same times, of the same size and MD5.
fn agrees_with_ffmpeg_vp8(file: &Path) -> Decoded {
    let name = file.display();
    let ours = decode(file, &Limits::default(), |_| false).expect("decodes");
    let theirs = framemd5(file, "vp8");
    assert_eq!(ours.pts.len(), theirs.len(), "{name}: frames");
    for (i, (pts, (size, md5))) in ours.pts.iter().zip(&ours.md5).enumerate() {
        let (their_pts, their_size, their_md5) = &theirs[i];
        assert_eq!(pts, their_pts, "{name}: frame {i} pts");
        assert_eq!(size, their_size, "{name}: frame {i} bytes");
        assert_eq!(md5, their_md5, "{name}: frame {i} MD5");
    }
    ours
}

#[test]
#[ignore = "needs ffmpeg with libvpx and its own vp8 decoder"]
fn frames_match_ffmpeg_vp8() {
    let dir = scratch("vp8");
    for (name, size, extra) in [
        ("420.webm", "176x144", &[][..]),
        ("420.mkv", "176x144", &[]),
        ("odd.webm", "75x51", &[]),
        // Version 3: the simple loop filter and full-sample chroma motion.
        ("version3.webm", "176x144", &["-profile:v", "3"]),
        (
            "partitions.webm",
            "1280x720",
            &["-cpu-used", "8", "-slices", "4"],
        ),
    ] {
        let file = generate(&dir, name, "libvpx", size, "yuv420p", extra);
        let ours = agrees_with_ffmpeg_vp8(&file);
        assert_eq!(ours.pts.len(), 25, "{name}: frames");
        let (width, height) = size.split_once('x').expect("WxH");
        let size = (
            width.parse().expect("width"),
            height.parse().expect("height"),
        );
        assert!(
            ours.sizes.iter().all(|s| *s == size),
            "{name}: {:?}",
            ours.sizes.first()
        );
    }
    // Hidden alternate reference frames: packets of their own that give no
    // frame.
    let altref = vp8_altref(&dir);
    let (_, sent) = packets(&altref);
    let ours = agrees_with_ffmpeg_vp8(&altref);
    assert_eq!(ours.pts.len(), 25, "altref.webm: frames");
    assert!(sent.len() > 25, "altref.webm: hidden frames");
    // A key frame of another size changes the frames' size.
    let resized = agrees_with_ffmpeg_vp8(&vp8_resized(&dir));
    assert_eq!(resized.sizes.first(), Some(&(176, 144)));
    assert_eq!(resized.sizes.last(), Some(&(320, 240)));
    assert_eq!(resized.sizes.len(), 50, "resized.webm: frames");
    // The same frames from MP4 (`vp08`) as from WebM.
    let webm = dir.join("420.webm");
    let mp4 = vp8_mp4(&webm, &dir.join("420.mp4"));
    let (stream, _) = packets(&mp4);
    assert_eq!(stream.codec, Codec::Vp8);
    let from_mp4 = agrees_with_ffmpeg_vp8(&mp4);
    let from_webm = decode(&webm, &Limits::default(), |_| false).expect("decodes");
    assert_eq!(from_mp4.md5, from_webm.md5, "420.mp4: the WebM's frames");
    assert_eq!(from_mp4.pts, from_webm.pts, "420.mp4: the WebM's times");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg with libvpx"]
fn vp8_key_frames_state_the_decoded_size() {
    let dir = scratch("vp8-sizes");
    let file = vp8_resized(&dir);
    let (_, packets) = packets(&file);
    let decoded = decode(&file, &Limits::default(), |_| false).expect("decodes");
    let mut stated = Vec::new();
    for sample in &packets {
        let size = vp8_frame_size(&sample.data).expect("frame tag");
        assert_eq!(size.is_some(), sample.keyframe, "key frames state a size");
        stated.extend(size.map(|s| (s.width, s.height)));
    }
    assert_eq!(
        stated,
        [(176, 144); 3]
            .into_iter()
            .chain([(320, 240); 3])
            .collect::<Vec<_>>()
    );
    let mut sizes = decoded.sizes.clone();
    sizes.dedup();
    assert_eq!(sizes, [(176, 144), (320, 240)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs ffmpeg with libvpx-vp9"]
fn stated_sizes_are_the_decoded_sizes() {
    let dir = scratch("sizes");
    let file = generate(
        &dir,
        "s.mp4",
        "libvpx-vp9",
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
#[ignore = "needs ffmpeg with libvpx-vp9 and libvpx"]
fn discarded_packets_give_no_frame_and_limits_hold() {
    let dir = scratch("discard");
    for (name, encoder) in [("d.mp4", "libvpx-vp9"), ("d.webm", "libvpx")] {
        discarding(&generate(&dir, name, encoder, "176x144", "yuv420p", &[]));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn discarding(file: &Path) {
    let all = decode(file, &Limits::default(), |_| false).expect("decodes");
    let some = decode(file, &Limits::default(), |i| i < 10).expect("decodes");
    let name = file.display();
    assert_eq!(some.pts.len(), 15, "{name}");
    assert_eq!(some.pts[..], all.pts[10..]);
    let frame = all.bytes.len() / 25;
    assert!(
        some.bytes[..] == all.bytes[10 * frame..],
        "{name}: the frames kept are the same"
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
            decode(file, &small, |_| false).err(),
            Some(ErrorKind::Limit),
            "{name}: frame size limit"
        );
    }
    let decoded = Limits {
        max_decoded_bytes: (frame * 3) as u64,
        ..Limits::default()
    };
    assert_eq!(
        decode(file, &decoded, |_| false).err(),
        Some(ErrorKind::Limit),
        "{name}: decoded-bytes limit"
    );
    let memory = Limits {
        max_memory_bytes: 64 << 20,
        ..Limits::default()
    };
    assert_eq!(
        decode(file, &memory, |_| false).err(),
        Some(ErrorKind::Limit),
        "{name}: memory estimate"
    );
}

/// The first packet of a one-frame VP9 file of the given size, with no
/// hidden frame, so no superframe index.
fn key_frame(dir: &Path, name: &str, size: &str) -> Vec<u8> {
    let file = generate(
        dir,
        name,
        "libvpx-vp9",
        size,
        "yuv420p",
        &[
            "-frames:v",
            "1",
            "-auto-alt-ref",
            "0",
            "-lag-in-frames",
            "0",
        ],
    );
    let (_, packets) = packets(&file);
    packets.into_iter().next().expect("a packet").data
}

#[test]
#[ignore = "needs ffmpeg with libvpx-vp9"]
fn frames_after_the_first_in_an_unindexed_packet_are_checked() {
    let dir = scratch("concatenated");
    let small = key_frame(&dir, "small.mp4", "64x64");
    let large = key_frame(&dir, "large.mp4", "128x128");
    let mut packet = small.clone();
    packet.extend_from_slice(&large);
    let limits = Limits {
        max_width: 64,
        max_height: 64,
        ..Limits::default()
    };
    for discard in [false, true] {
        let mut decoder = LibvpxDecoder::open(
            &LibvpxOptions::default(),
            &StreamConfig::new(Codec::Vp9),
            &limits,
        )
        .expect("open");
        let sent = decoder.send_packet(&Packet {
            data: &packet,
            pts: Some(0),
            dts: Some(0),
            keyframe: true,
            discard,
        });
        assert!(sent.is_ok(), "discard {discard}: {sent:?}");
        let mut sizes = Vec::new();
        while let Some(frame) = decoder.receive_frame().expect("frame") {
            let picture = frame.to_cpu().expect("cpu");
            sizes.push((picture.width(), picture.height()));
        }
        // One frame, the first: the second's bytes are part of it, and
        // libvpx never decodes a frame no header check has seen.
        let expected = if discard { vec![] } else { vec![(64, 64)] };
        assert_eq!(sizes, expected, "discard {discard}");
    }
    let mut decoder = LibvpxDecoder::open(
        &LibvpxOptions::default(),
        &StreamConfig::new(Codec::Vp9),
        &Limits::default(),
    )
    .expect("open");
    for data in [&small, &large] {
        decoder
            .send_packet(&Packet {
                data,
                pts: Some(0),
                dts: Some(0),
                keyframe: true,
                discard: false,
            })
            .expect("each frame alone decodes");
    }
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
    // A VP8 key frame tag and header stating 65 x 48 and nothing else,
    // over a width limit of 64: libvpx would call the frame corrupt.
    let header = [0x10, 0x00, 0x00, 0x9d, 0x01, 0x2a, 65, 0x00, 48, 0x00];
    let limits = Limits {
        max_width: 64,
        ..Limits::default()
    };
    let mut decoder = LibvpxDecoder::open(
        &LibvpxOptions::default(),
        &StreamConfig::new(Codec::Vp8),
        &limits,
    )
    .expect("open");
    let packet = Packet {
        data: &header,
        ..packet
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
    // A key frame's start for each codec: VP9's frame marker, profile 0
    // and sync code; VP8's frame tag, start code and a size of 64 x 48.
    let starts: [(Codec, &[u8]); 2] = [
        (Codec::Vp9, &[0x82, 0x49, 0x83, 0x42]),
        (
            Codec::Vp8,
            &[0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a, 64, 0, 48, 0],
        ),
    ];
    for round in 0..128 {
        let (codec, start) = starts[round % 2];
        let stream = StreamConfig::new(codec);
        let mut decoder =
            LibvpxDecoder::open(&LibvpxOptions::default(), &stream, &Limits::default())
                .expect("open");
        for i in 0..32 {
            let len = (next() % 256) as usize + round / 2;
            let mut data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Half the packets start like a key frame, so that libvpx reads
            // past the header.
            if i % 2 == 0 && data.len() > start.len() {
                data[..start.len()].copy_from_slice(start);
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
