//! The encoder programs as black boxes (`docs/design/transcode.md`, "The
//! encoder program"): the pinned x264 and FFmpeg with libx264, run
//! directly (the sandbox launcher's own runs are in `sandboxed.rs`). Each
//! check skips a program that is not installed.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use coreav_frame::{Chroma, ColorInfo, PixelFormat};
use coreav_transcode::encoder::EncoderSettings;

use common::*;

#[test]
fn thirty_frames_come_out() {
    let dir = scratch("thirty");
    for program in programs() {
        let format = PixelFormat::YUV420P;
        let frames: Vec<_> = (0..30).map(|n| moving(176, 144, format, n)).collect();
        let stream = encode(&program, &settings(176, 144, format), None, &frames).unwrap();
        let lines = probe(&stream, &dir, program.name());
        assert_eq!(field(&lines, "nb_read_frames"), "30", "{program:?}");
        assert_eq!(field(&lines, "width"), "176");
        assert_eq!(field(&lines, "height"), "144");
        assert_eq!(field(&lines, "profile"), "High", "{program:?}");
        assert_eq!(field(&lines, "pix_fmt"), "yuv420p");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Black-box check (i): full-range frames keep their levels. A flat frame
/// at 0 and one at the maximum, signalled as full range, decode to 0 and the
/// maximum again, not to 16 and 235 (a conversion to limited range).
#[test]
fn full_range_is_not_converted() {
    let dir = scratch("range");
    for program in programs() {
        for depth in [8u8, 10] {
            let format = yuv(Chroma::Cs420, depth);
            let max = (1u16 << depth) - 1;
            let mid = 1u16 << (depth - 1);
            let frames: Vec<_> = [0u16, max, 0, max]
                .iter()
                .map(|&v| picture(64, 64, format, move |p, _, _| if p == 0 { v } else { mid }))
                .collect();
            let s = EncoderSettings {
                color: ColorInfo {
                    full_range: Some(true),
                    ..ColorInfo::default()
                },
                ..settings(64, 64, format)
            };
            let stream = encode(&program, &s, None, &frames).unwrap();
            let lines = probe(&stream, &dir, program.name());
            assert_eq!(field(&lines, "color_range"), "pc", "{program:?}");
            let raw = decode(&stream, &dir, program.name());
            let bytes = usize::from(depth > 8) + 1;
            let frame = 64 * 64 * 3 / 2 * bytes;
            assert_eq!(raw.len(), frame * 4, "{program:?}");
            for (n, want) in [0u16, max, 0, max].iter().enumerate() {
                let luma = &raw[n * frame..n * frame + 64 * 64 * bytes];
                let worst = luma
                    .chunks(bytes)
                    .map(|s| {
                        let v = if bytes == 1 {
                            u16::from(s[0])
                        } else {
                            u16::from_le_bytes([s[0], s[1]])
                        };
                        v.abs_diff(*want)
                    })
                    .max()
                    .unwrap();
                assert!(
                    worst <= 1,
                    "{program:?} depth {depth} frame {n}: off by {worst}"
                );
            }
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The names `ffprobe` gives the H.273 code points, for the check below.
fn probe_name(table: &str, code: u8) -> &'static str {
    match (table, code) {
        ("primaries", 1) | ("transfer", 1) | ("matrix", 1) => "bt709",
        ("primaries", 4) | ("transfer", 4) => "bt470m",
        ("primaries", 5) | ("transfer", 5) | ("matrix", 5) => "bt470bg",
        (_, 6) => "smpte170m",
        (_, 7) => "smpte240m",
        ("primaries", 8) => "film",
        ("primaries", 9) => "bt2020",
        ("primaries", 10) => "smpte428",
        ("primaries", 11) => "smpte431",
        ("primaries", 12) => "smpte432",
        ("primaries", 22) => "ebu3213",
        ("transfer", 8) => "linear",
        ("transfer", 9) => "log100",
        ("transfer", 10) => "log316",
        ("transfer", 11) => "iec61966-2-4",
        ("transfer", 12) => "bt1361e",
        ("transfer", 13) => "iec61966-2-1",
        ("transfer", 14) => "bt2020-10",
        ("transfer", 15) => "bt2020-12",
        ("transfer", 16) => "smpte2084",
        ("transfer", 17) => "smpte428",
        ("transfer", 18) => "arib-std-b67",
        ("matrix", 0) => "gbr",
        ("matrix", 4) => "fcc",
        ("matrix", 8) => "ycgco",
        ("matrix", 9) => "bt2020nc",
        ("matrix", 10) => "bt2020c",
        ("matrix", 11) => "smpte2085",
        ("matrix", 12) => "chroma-derived-nc",
        ("matrix", 13) => "chroma-derived-c",
        ("matrix", 14) => "ictcp",
        _ => panic!("no name for {table} {code}"),
    }
}

/// Black-box check (ii): every H.273 code point CoreAV passes comes out in
/// the stream's VUI as asked, with both programs, and the range with it.
#[test]
fn colour_description_comes_out_as_asked() {
    let primaries = [1u8, 4, 5, 6, 7, 8, 9, 10, 11, 12, 22];
    let transfer = [1u8, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18];
    let matrix = [1u8, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 0];
    let dir = scratch("vui");
    let format = PixelFormat::YUV420P;
    let frames: Vec<_> = (0..2).map(|n| moving(32, 32, format, n)).collect();
    for program in programs() {
        for i in 0..transfer.len() {
            let color = ColorInfo {
                primaries: Some(primaries[i % primaries.len()]),
                transfer: Some(transfer[i]),
                matrix: Some(matrix[i % matrix.len()]),
                full_range: Some(i % 2 == 0),
                chroma_location: None,
            };
            let s = EncoderSettings {
                color,
                ..settings(32, 32, format)
            };
            let stream = encode(&program, &s, None, &frames).unwrap();
            let lines = probe(&stream, &dir, program.name());
            let prim = color.primaries.unwrap();
            // Primaries 22 (EBU 3213) are not passed: x264 has no name for
            // them, and libx264 drops them (FFmpeg's `-color_primaries
            // ebu3213` came out unspecified).
            let want_prim = if prim == 22 {
                "unknown"
            } else {
                probe_name("primaries", prim)
            };
            assert_eq!(
                field(&lines, "color_primaries"),
                want_prim,
                "{program:?} {color:?}"
            );
            assert_eq!(
                field(&lines, "color_transfer"),
                probe_name("transfer", color.transfer.unwrap()),
                "{program:?} {color:?}"
            );
            assert_eq!(
                field(&lines, "color_space"),
                probe_name("matrix", color.matrix.unwrap()),
                "{program:?} {color:?}"
            );
            assert_eq!(
                field(&lines, "color_range"),
                if i % 2 == 0 { "pc" } else { "tv" },
                "{program:?} {color:?}"
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Black-box check (iii): 10-bit input is encoded at 10 bits (High 10), and
/// 4:2:2 and 4:4:4 keep their chroma, with both programs.
#[test]
fn formats_keep_their_depth_and_chroma() {
    let dir = scratch("formats");
    let cases = [
        (Chroma::Cs420, 10, "High 10", "yuv420p10le"),
        (Chroma::Cs422, 8, "High 4:2:2", "yuv422p"),
        (Chroma::Cs422, 10, "High 4:2:2", "yuv422p10le"),
        (Chroma::Cs444, 8, "High 4:4:4 Predictive", "yuv444p"),
        (Chroma::Cs444, 10, "High 4:4:4 Predictive", "yuv444p10le"),
    ];
    for program in programs() {
        for (chroma, depth, profile, pix_fmt) in cases {
            let format = yuv(chroma, depth);
            let frames: Vec<_> = (0..5).map(|n| moving(96, 64, format, n)).collect();
            let stream = encode(&program, &settings(96, 64, format), None, &frames).unwrap();
            let lines = probe(&stream, &dir, program.name());
            assert_eq!(field(&lines, "profile"), profile, "{program:?} {format:?}");
            assert_eq!(field(&lines, "pix_fmt"), pix_fmt, "{program:?} {format:?}");
            assert_eq!(field(&lines, "nb_read_frames"), "5");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Black-box check (iv): the sample aspect ratio comes out exactly, also
/// one whose terms are large (FFmpeg's `-aspect` rounds 4095:4096 at
/// 720x480 to 8:9).
#[test]
fn sample_aspect_ratio_comes_out_as_asked() {
    let dir = scratch("sar");
    let format = PixelFormat::YUV420P;
    let frames: Vec<_> = (0..3).map(|n| moving(720, 480, format, n)).collect();
    for program in programs() {
        for (sar, want) in [
            ((10, 11), "10:11"),
            ((4095, 4096), "4095:4096"),
            ((64, 45), "64:45"),
        ] {
            let s = EncoderSettings {
                sample_aspect: Some(sar),
                preset: "ultrafast".into(),
                ..settings(720, 480, format)
            };
            let stream = encode(&program, &s, None, &frames).unwrap();
            let lines = probe(&stream, &dir, program.name());
            assert_eq!(field(&lines, "sample_aspect_ratio"), want, "{program:?}");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_frame_that_differs_from_the_settings_is_refused() {
    let Some(program) = programs().into_iter().next() else {
        return;
    };
    let format = PixelFormat::YUV420P;
    let frames = [moving(64, 64, format, 0), moving(64, 48, format, 1)];
    let error = encode(&program, &settings(64, 64, format), None, &frames).unwrap_err();
    assert_eq!(
        error.kind,
        coreav_decode::ErrorKind::InvalidData,
        "{error:?}"
    );
    let frames = [moving(64, 64, yuv(Chroma::Cs444, 8), 0)];
    let error = encode(&program, &settings(64, 64, format), None, &frames).unwrap_err();
    assert_eq!(
        error.kind,
        coreav_decode::ErrorKind::InvalidData,
        "{error:?}"
    );
}
