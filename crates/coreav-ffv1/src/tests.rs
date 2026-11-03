#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use super::*;
use coreav_frame::Plane;
use std::time::Duration;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    code: u32,
    span: u32,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 2,
            code: u32::from(bytes.first().copied().unwrap_or(0)) * 256
                + u32::from(bytes.get(1).copied().unwrap_or(0)),
            span: 0xff00,
        }
    }

    fn bit(&mut self, probability: &mut u8) -> bool {
        let split = (self.span * u32::from(*probability)) >> 8;
        let boundary = self.span - split;
        let answer = self.code >= boundary;
        if answer {
            self.code -= boundary;
            self.span = split;
            *probability = SPEC_STATES[usize::from(*probability)];
        } else {
            self.span = boundary;
            *probability = (256 - u16::from(SPEC_STATES[256 - usize::from(*probability)])) as u8;
        }
        if self.span < 256 {
            self.span *= 256;
            self.code = self.code * 256 + u32::from(self.bytes.get(self.at).copied().unwrap_or(0));
            self.at += 1;
        }
        assert!(self.code < self.span);
        answer
    }

    fn symbol(&mut self, states: &mut [u8; 32], signed: bool) -> i64 {
        if self.bit(&mut states[0]) {
            return 0;
        }
        let mut exponent = 0;
        while self.bit(&mut states[1 + exponent.min(9)]) {
            exponent += 1;
            assert!(exponent <= 31);
        }
        let mut magnitude = 1i64;
        for i in (0..exponent).rev() {
            magnitude = magnitude * 2 + i64::from(self.bit(&mut states[22 + i.min(9)]));
        }
        if signed && self.bit(&mut states[11 + exponent.min(10)]) {
            -magnitude
        } else {
            magnitude
        }
    }
}

const SPEC_STATES: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37,
    37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 56, 57, 58, 59,
    60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 75, 76, 77, 78, 79, 80, 81, 82,
    83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103,
    104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 114, 115, 116, 117, 118, 119, 120, 121,
    122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 133, 134, 135, 136, 137, 138, 139,
    140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 152, 153, 154, 155, 156, 157,
    158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 171, 172, 173, 174, 175,
    176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 190, 191, 192, 194,
    194, 195, 196, 197, 198, 199, 200, 201, 202, 202, 204, 205, 206, 207, 208, 209, 209, 210, 211,
    212, 213, 215, 215, 216, 217, 218, 219, 220, 220, 222, 223, 224, 225, 226, 227, 227, 229, 229,
    230, 231, 232, 234, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248,
    248, 0, 0, 0, 0, 0, 0, 0,
];

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

fn settings(width: u32, height: u32, chroma: Chroma) -> Settings {
    Settings {
        width,
        height,
        format: PixelFormat::Yuv { chroma, depth: 8 },
        sample_aspect: Some((4, 3)),
    }
}

fn picture(settings: Settings, padding: usize, seed: u32) -> Picture {
    let mut planes = Vec::new();
    let mut random = seed;
    for p in 0..3 {
        let (w, h) = settings
            .format
            .plane_size(p, settings.width, settings.height)
            .unwrap();
        let stride = w as usize + padding;
        let mut bytes = vec![97; stride * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                bytes[y * stride + x] = match seed {
                    0 => 0,
                    1 => 255,
                    2 => (x + y * w as usize) as u8,
                    3 => {
                        if (x + y) % 2 == 0 {
                            0
                        } else {
                            255
                        }
                    }
                    _ => (random >> 24) as u8,
                };
            }
        }
        planes.push(Plane::new(bytes, stride));
    }
    Picture::new(settings.width, settings.height, settings.format, planes).unwrap()
}

fn slow_crc(bytes: &[u8]) -> u32 {
    let mut value = 0u64;
    for byte in bytes {
        for bit in (0..8).rev() {
            let top = value & (1 << 31) != 0;
            value = ((value << 1) & 0xffff_ffff) ^ u64::from((byte >> bit) & 1);
            if top {
                value ^= 0x04c1_1db7;
            }
        }
    }
    for _ in 0..32 {
        value <<= 1;
        if value & (1 << 32) != 0 {
            value ^= 0x1_04c1_1db7;
        }
    }
    value as u32
}

#[test]
fn scalar_range_roundtrips_all_residuals_and_carries() {
    for pattern in 0..5 {
        let mut bytes = Buffer::new(1_000_000, 2_000_000, 0);
        let mut coder = Range::new(&mut bytes);
        let mut states = [128; 32];
        let mut symbols = Vec::new();
        for i in 0..8192 {
            let value = match pattern {
                0 => i % 256 - 128,
                1 => 0,
                2 => {
                    if i % 2 == 0 {
                        -128
                    } else {
                        127
                    }
                }
                3 => i % 127 + 1,
                _ => -1 - i % 128,
            };
            coder.signed(&mut states, value).unwrap();
            symbols.push(value);
        }
        coder.finish().unwrap();
        let output = bytes.into_vec();
        let mut reader = Reader::new(&output);
        let mut states = [128; 32];
        for value in symbols {
            assert_eq!(reader.symbol(&mut states, true), i64::from(value));
        }
        assert!(reader.at <= output.len() + 1);
    }
    for value in [0, 1, 2, 127, 128, 255, 256, 65_535, u32::MAX] {
        let mut bytes = Buffer::new(1024, 2048, 0);
        let mut coder = Range::new(&mut bytes);
        coder.unsigned(&mut [128; 32], value).unwrap();
        coder.finish().unwrap();
        assert_eq!(
            Reader::new(&bytes.into_vec()).symbol(&mut [128; 32], false),
            i64::from(value)
        );
    }
}

fn configuration(encoder: &Encoder) {
    let record = encoder.configuration();
    assert_eq!(range::crc(record), 0);
    assert_eq!(slow_crc(record), 0);
    let mut r = Reader::new(&record[..record.len() - 4]);
    let mut s = [128; 32];
    for value in [3, 4, 1, 0, 8] {
        assert_eq!(r.symbol(&mut s, false), value);
    }
    assert!(r.bit(&mut s[0]));
    assert_eq!(r.symbol(&mut s, false), i64::from(encoder.shifts.0));
    assert_eq!(r.symbol(&mut s, false), i64::from(encoder.shifts.1));
    assert!(!r.bit(&mut s[0]));
    assert_eq!(r.symbol(&mut s, false), i64::from(encoder.grid - 1));
    assert_eq!(r.symbol(&mut s, false), i64::from(encoder.grid - 1));
    assert_eq!(r.symbol(&mut s, false), 1);
    let mut q = [128; 32];
    let mut scale = 1;
    for table in 0..5 {
        let mut entries = Vec::new();
        let mut count = 0;
        while entries.len() < 128 {
            let length = r.symbol(&mut q, false) as usize + 1;
            entries.extend(std::iter::repeat_n(count * scale, length));
            count += 1;
        }
        assert_eq!(entries.len(), 128);
        assert_eq!(entries[0], 0);
        if table < 3 {
            assert_eq!(entries[1], scale);
            assert_eq!(entries[3], scale);
            assert_eq!(entries[4], 2 * scale);
            assert_eq!(entries[127], 2 * scale);
        } else {
            assert!(entries.iter().all(|&value| value == 0));
        }
        scale *= 2 * count - 1;
    }
    assert_eq!((scale + 1) / 2, 63);
    assert!(!r.bit(&mut s[0]));
    assert_eq!(r.symbol(&mut s, false), 1);
    assert_eq!(r.symbol(&mut s, false), 1);
}

fn reconstruct(encoder: &Encoder, packet: &[u8], source: &Picture) {
    let mut slices = Vec::new();
    let mut end = packet.len();
    while end != 0 {
        assert!(end >= 8);
        let size = (usize::from(packet[end - 8]) << 16)
            | (usize::from(packet[end - 7]) << 8)
            | usize::from(packet[end - 6]);
        let start = end - 8 - size;
        assert_eq!(packet[end - 5], 0);
        assert_eq!(slow_crc(&packet[start..end]), 0);
        slices.push(&packet[start..end - 8]);
        end = start;
    }
    slices.reverse();
    assert_eq!(slices.len(), (encoder.grid * encoder.grid) as usize);
    for (index, bytes) in slices.into_iter().enumerate() {
        let mut reader = Reader::new(bytes);
        if index == 0 {
            assert!(reader.bit(&mut 128));
        }
        let mut header = [128; 32];
        let sx = reader.symbol(&mut header, false) as u32;
        let sy = reader.symbol(&mut header, false) as u32;
        assert_eq!(
            (sx, sy),
            (index as u32 % encoder.grid, index as u32 / encoder.grid)
        );
        for _ in 0..4 {
            assert_eq!(reader.symbol(&mut header, false), 0);
        }
        assert_eq!(reader.symbol(&mut header, false), 3);
        let (n, d) = encoder.settings.sample_aspect.unwrap_or((0, 0));
        assert_eq!(reader.symbol(&mut header, false), i64::from(n));
        assert_eq!(reader.symbol(&mut header, false), i64::from(d));
        let mut banks = [[[128; 32]; 63]; 2];
        for p in 0..3 {
            let hs = if p == 0 { 0 } else { encoder.shifts.0 };
            let vs = if p == 0 { 0 } else { encoder.shifts.1 };
            let x0 = encoder.settings.width * sx / encoder.grid;
            let y0 = encoder.settings.height * sy / encoder.grid;
            let x1 = encoder.settings.width * (sx + 1) / encoder.grid;
            let y1 = encoder.settings.height * (sy + 1) / encoder.grid;
            let w = (x1 - x0).div_ceil(1 << hs) as usize;
            let h = (y1 - y0).div_ceil(1 << vs) as usize;
            let mut raster = vec![vec![0i32; w + 2]; h + 2];
            for y in 0..h {
                for x in 0..w {
                    let left = raster[y + 2][x];
                    let diag = raster[y + 1][x];
                    let top = raster[y + 1][x + 1];
                    let right = raster[y + 1][x + 2];
                    let classify = |d: i32| {
                        let byte = (d & 255) as u8;
                        let v = i32::from(byte as i8);
                        v.signum()
                            * if v == 0 {
                                0
                            } else if v.abs() < 4 {
                                1
                            } else {
                                2
                            }
                    };
                    let context = classify(left - diag)
                        + classify(diag - top) * 5
                        + classify(top - right) * 25;
                    let mut candidates = [left, top, left + top - diag];
                    candidates.sort();
                    let residual = reader.symbol(
                        &mut banks[usize::from(p != 0)][context.unsigned_abs() as usize],
                        true,
                    ) as i32;
                    let value =
                        (candidates[1] + if context < 0 { -residual } else { residual }) & 255;
                    raster[y + 2][x + 1] = value;
                    assert_eq!(
                        value as u8,
                        source.row(p, (y0 >> vs) as usize + y).unwrap()[(x0 >> hs) as usize + x]
                    );
                }
                raster[y + 2][w + 1] = raster[y + 2][w];
                if y + 3 < h + 2 {
                    raster[y + 3][0] = raster[y + 2][1];
                }
            }
        }
        assert!(reader.at <= bytes.len() + 1);
    }
}

#[test]
fn native_planes_borders_padding_and_independent_frames() {
    for chroma in [Chroma::Cs420, Chroma::Cs422, Chroma::Cs444] {
        for (width, height) in [(1, 1), (1, 33), (35, 1), (17, 19), (32, 24)] {
            let settings = settings(width, height, chroma);
            let mut encoder = Encoder::new(settings, Limits::default()).unwrap();
            configuration(&encoder);
            for seed in 0..6 {
                let packed = picture(settings, 0, seed);
                let padded = picture(settings, 13, seed);
                let packet = encoder.encode(&packed, deadline()).unwrap();
                assert_eq!(packet, encoder.encode(&padded, deadline()).unwrap());
                reconstruct(&encoder, &packet, &packed);
                encoder
                    .encode(&picture(settings, 1, 99), deadline())
                    .unwrap();
                assert_eq!(packet, encoder.encode(&packed, deadline()).unwrap());
            }
        }
    }
}

#[test]
fn threshold_and_four_slice_coverage() {
    for chroma in [Chroma::Cs420, Chroma::Cs422, Chroma::Cs444] {
        for (width, height, grid) in [(352, 288, 1), (356, 288, 2), (321, 317, 2)] {
            let settings = settings(width, height, chroma);
            if chroma != Chroma::Cs444 && width == 321 {
                assert!(matches!(
                    Encoder::new(settings, Limits::default()),
                    Err(Error::Unsupported(_))
                ));
                continue;
            }
            let mut encoder = Encoder::new(settings, Limits::default()).unwrap();
            assert_eq!(encoder.grid, grid);
            configuration(&encoder);
            let source = picture(settings, 2, 7);
            let packet = encoder.encode(&source, deadline()).unwrap();
            reconstruct(&encoder, &packet, &source);
        }
    }
}

#[test]
fn crc_and_quantized_context_boundaries() {
    assert_eq!(range::crc(b"123456789"), 0x89a1_897f);
    assert_eq!(slow_crc(b"123456789"), 0x89a1_897f);
    for d in -255..=255 {
        let signed = (d & 255) as u8 as i8;
        let expected = match signed {
            0 => 0,
            1..=3 => 1,
            -3..=-1 => -1,
            4..=127 => 2,
            _ => -2,
        };
        assert_eq!(quant(d), expected);
    }
}

#[test]
fn limits_deadline_and_invalid_frames_publish_nothing() {
    let settings = settings(17, 19, Chroma::Cs420);
    let source = picture(settings, 5, 7);
    let mut encoder = Encoder::new(settings, Limits::default()).unwrap();
    let packet = encoder.encode(&source, deadline()).unwrap();
    assert_eq!(encoder.encode(&source, Instant::now()), Err(Error::Timeout));
    assert_eq!(
        encoder.encode(
            &picture(
                super::Settings {
                    width: 18,
                    ..settings
                },
                0,
                0
            ),
            deadline()
        ),
        Err(Error::InvalidFrame)
    );
    for (bound, good) in [(packet.len(), true), (packet.len() - 1, false)] {
        let limits = Limits {
            max_packet_bytes: bound,
            ..Limits::default()
        };
        let mut encoder = Encoder::new(settings, limits).unwrap();
        assert_eq!(encoder.encode(&source, deadline()).is_ok(), good);
    }
    for limits in [
        Limits {
            max_width: 16,
            ..Limits::default()
        },
        Limits {
            max_height: 18,
            ..Limits::default()
        },
        Limits {
            max_pixels: 322,
            ..Limits::default()
        },
        Limits {
            max_alloc_bytes: WORKSPACE,
            ..Limits::default()
        },
    ] {
        assert!(matches!(
            Encoder::new(settings, limits),
            Err(Error::Limit(_))
        ));
    }
    let exact = Limits {
        max_width: 17,
        max_height: 19,
        max_pixels: 323,
        ..Limits::default()
    };
    assert!(Encoder::new(settings, exact).is_ok());
    let mut lo = WORKSPACE;
    let mut hi = 100_000;
    while lo < hi {
        let middle = lo + (hi - lo) / 2;
        let limits = Limits {
            max_alloc_bytes: middle,
            ..Limits::default()
        };
        let result = Encoder::new(settings, limits).and_then(|mut e| e.encode(&source, deadline()));
        if result.is_ok() {
            hi = middle;
        } else {
            lo = middle + 1;
        }
    }
    for (budget, good) in [(lo, true), (lo - 1, false)] {
        let limits = Limits {
            max_alloc_bytes: budget,
            ..Limits::default()
        };
        let result = Encoder::new(settings, limits).and_then(|mut e| e.encode(&source, deadline()));
        assert_eq!(result.is_ok(), good);
    }
    assert_eq!(packet, encoder.encode(&source, deadline()).unwrap());
}

#[test]
fn packet_capacity_growth_charges_both_live_allocations() {
    for (live, good) in [(513, true), (512, false)] {
        let mut buffer = Buffer::new(1024, live, 0);
        for _ in 0..256 {
            buffer.push(42).unwrap();
        }
        assert_eq!(buffer.push(43).is_ok(), good);
        let bytes = buffer.into_vec();
        assert_eq!(bytes.capacity(), if good { 257 } else { 256 });
        assert!(bytes.iter().take(256).all(|&byte| byte == 42));
    }
    let mut buffer = Buffer::new(3, 3, 0);
    for value in 0..3 {
        buffer.push(value).unwrap();
    }
    assert_eq!(buffer.push(3), Err(Error::Limit("packet bytes")));
    let bytes = buffer.into_vec();
    assert_eq!(bytes.capacity(), 3);
    assert_eq!(bytes, [0, 1, 2]);
}

#[test]
fn unsupported_settings_are_refused_before_allocation() {
    let base = settings(640, 480, Chroma::Cs420);
    for settings in [
        Settings { width: 0, ..base },
        Settings { height: 0, ..base },
        Settings { width: 642, ..base },
        Settings {
            height: 482,
            ..base
        },
        Settings {
            sample_aspect: Some((0, 1)),
            ..base
        },
        Settings {
            sample_aspect: Some((1, 0)),
            ..base
        },
        Settings {
            format: PixelFormat::Rgb24,
            ..base
        },
        Settings {
            format: PixelFormat::Yuv {
                chroma: Chroma::Mono,
                depth: 8,
            },
            ..base
        },
        Settings {
            format: PixelFormat::Yuv {
                chroma: Chroma::Cs420,
                depth: 10,
            },
            ..base
        },
    ] {
        assert!(matches!(
            Encoder::new(
                settings,
                Limits {
                    max_alloc_bytes: 0,
                    ..Limits::default()
                }
            ),
            Err(Error::Unsupported(_))
        ));
    }
}
