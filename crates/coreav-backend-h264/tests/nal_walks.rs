//! The two framings of H.264 NAL units against each other, on generated
//! frames: one frame written as a byte stream (ITU-T H.264 Annex B) and
//! as length-prefixed units (ISO/IEC 14496-15) must give the same NAL
//! units through `coreav_h264::annex_b`, the chunked `AnnexB` splitter
//! and `coreav_h264::length_prefixed`, and the same
//! `coreav_isobmff::access::FrameAccess` through `annex_b_frame_access`
//! and `read_frame_access`.
//!
//! The default run checks a few hundred frames; the ignored test checks
//! 20 000 (`cargo test --release -p coreav-backend-h264 --test nal_walks
//! -- --ignored`).
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use coreav_h264::{AnnexB, Limits, annex_b, length_prefixed};
use coreav_isobmff::SliceSource;
use coreav_isobmff::access::{AccessFormat, annex_b_frame_access, read_frame_access};

/// xorshift64: a fixed generator, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn range(&mut self, from: usize, to: usize) -> usize {
        from + self.below(to - from + 1)
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    /// A byte, zero or below 4 often enough that emulation prevention
    /// (7.4.1.1) has work.
    fn byte(&mut self) -> u8 {
        match self.below(8) {
            0..=2 => 0,
            3 => self.below(4) as u8,
            _ => self.next() as u8,
        }
    }
}

/// An RBSP written bit by bit.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u8,
}

impl Bits {
    fn bit(&mut self, bit: bool) {
        if self.used == 0 {
            self.bytes.push(0);
        }
        if bit {
            *self.bytes.last_mut().unwrap() |= 0x80 >> self.used;
        }
        self.used = (self.used + 1) % 8;
    }

    fn byte(&mut self, byte: u8) {
        for i in (0..8).rev() {
            self.bit(byte >> i & 1 == 1);
        }
    }

    fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let width = 64 - code.leading_zeros();
        for _ in 1..width {
            self.bit(false);
        }
        for i in (0..width).rev() {
            self.bit(code >> i & 1 == 1);
        }
    }

    /// `rbsp_trailing_bits` (7.3.2.11), which also end an SEI payload
    /// that is not byte aligned (7.3.2.3.1).
    fn trailing(mut self) -> Vec<u8> {
        self.bit(true);
        while self.used != 0 {
            self.bit(false);
        }
        self.bytes
    }
}

/// The NAL unit of header byte `header` and RBSP `rbsp`, with an
/// emulation_prevention_three_byte wherever two zeros come before a byte
/// of 3 or less, and after an RBSP that ends in a zero (7.4.1).
fn escape(header: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = vec![header];
    let mut zeros = 0;
    for &b in rbsp {
        if zeros == 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    if zeros > 0 {
        out.push(3);
    }
    out
}

/// What the generated frames covered, so that a generator change that
/// stops reaching an edge case fails the test.
#[derive(Debug, Default)]
struct Seen {
    escaped: usize,
    cabac_zero_words: usize,
    four_byte_start_codes: usize,
    three_byte_start_codes: usize,
    trailing_zeros: usize,
    length_sizes: [usize; 5],
    recovery_points: usize,
    recovery_points_at_read_limit: usize,
    unparsed_slices: usize,
}

/// A slice header (7.3.3) up to `slice_type`, then slice data.
fn slice(rng: &mut Rng, seen: &mut Seen) -> Vec<u8> {
    let kind = [1u8, 1, 1, 5, 5, 2][rng.below(6)];
    let header = (rng.below(4) as u8) << 5 | kind;
    let mut bits = Bits::default();
    bits.ue(if rng.one_in(4) {
        (rng.next() as u32) >> rng.below(32)
    } else {
        0
    });
    let slice_type = if rng.one_in(10) {
        seen.unparsed_slices += 1;
        rng.range(10, 300) as u32
    } else {
        rng.below(10) as u32
    };
    bits.ue(slice_type);
    let data = match rng.below(100) {
        0 => rng.range(66_000, 70_000),
        1..=8 => rng.range(256, 2_000),
        _ => rng.below(200),
    };
    for _ in 0..data {
        bits.byte(rng.byte());
    }
    let mut rbsp = bits.trailing();
    if rng.one_in(8) {
        // cabac_zero_words (7.3.2.10): the unit then ends in 0x000003.
        seen.cabac_zero_words += 1;
        for _ in 0..rng.range(1, 3) {
            rbsp.extend([0, 0]);
        }
    }
    escape(header, &rbsp)
}

/// An SEI payloadType or payloadSize (7.3.2.3.1).
fn sei_value(out: &mut Vec<u8>, mut value: usize) {
    while value >= 255 {
        out.push(0xFF);
        value -= 255;
    }
    out.push(value as u8);
}

/// Bytes the access walks read of an SEI unit, its header among them
/// (`MAX_SEI_BYTES` in `coreav_isobmff::access`).
const SEI_READ_BYTES: usize = 4096;

/// SEI messages (7.3.2.3), a recovery point (D.1.8) among them or not.
/// A long `filler_payload` (Annex D) first sometimes puts the recovery
/// point at the end of the bytes the access walks read of the unit.
fn sei(rng: &mut Rng, seen: &mut Seen) -> Vec<u8> {
    let mut rbsp = Vec::new();
    let filler = rng.one_in(4);
    if filler {
        let size = rng.range(SEI_READ_BYTES - 32, SEI_READ_BYTES - 12);
        sei_value(&mut rbsp, 3);
        sei_value(&mut rbsp, size);
        rbsp.resize(rbsp.len() + size, 0xFF);
    } else {
        for _ in 0..rng.below(3) {
            let size = rng.below(40);
            sei_value(&mut rbsp, [0, 1, 4, 5, 45][rng.below(5)]);
            sei_value(&mut rbsp, size);
            rbsp.extend((0..size).map(|_| rng.byte()));
        }
    }
    let recovery = rng.below(3) != 0;
    if recovery {
        seen.recovery_points += 1;
        let mut bits = Bits::default();
        bits.ue(rng.below(300) as u32);
        for _ in 0..4 {
            bits.bit(rng.one_in(2));
        }
        let body = bits.trailing();
        sei_value(&mut rbsp, 6);
        sei_value(&mut rbsp, body.len());
        rbsp.extend(body);
    }
    rbsp.push(0x80);
    let unit = escape((rng.below(4) as u8) << 5 | 6, &rbsp);
    if filler && recovery && unit.len().abs_diff(SEI_READ_BYTES) <= 2 {
        seen.recovery_points_at_read_limit += 1;
    }
    unit
}

/// Any other unit: parameter sets, delimiters, filler, reserved types.
fn other(rng: &mut Rng) -> Vec<u8> {
    let kind = [3u8, 4, 7, 8, 9, 10, 11, 12, 13, 19, 20, 23, 31][rng.below(13)];
    let mut bits = Bits::default();
    for _ in 0..rng.below(60) {
        bits.byte(rng.byte());
    }
    escape((rng.below(4) as u8) << 5 | kind, &bits.trailing())
}

fn frame(rng: &mut Rng, seen: &mut Seen) -> Vec<Vec<u8>> {
    let units: Vec<Vec<u8>> = (0..rng.range(1, 8))
        .map(|_| match rng.below(6) {
            0..=2 => slice(rng, seen),
            3 => sei(rng, seen),
            _ => other(rng),
        })
        .collect();
    for unit in &units {
        seen.escaped += unit.windows(3).filter(|w| w == &[0, 0, 3]).count();
    }
    units
}

/// The frame as a byte stream (B.1): leading zeros, a three- or
/// four-byte start code before each unit, trailing zeros after it.
fn byte_stream(rng: &mut Rng, units: &[Vec<u8>], seen: &mut Seen) -> Vec<u8> {
    let mut out = vec![0; rng.below(3)];
    for unit in units {
        if rng.one_in(2) {
            seen.four_byte_start_codes += 1;
            out.push(0);
        } else {
            seen.three_byte_start_codes += 1;
        }
        out.extend([0, 0, 1]);
        out.extend(unit);
        if rng.one_in(4) {
            seen.trailing_zeros += 1;
            out.resize(out.len() + rng.range(1, 4), 0);
        }
    }
    out
}

/// The frame as length-prefixed units, of a length size that holds the
/// longest unit.
fn prefixed(rng: &mut Rng, units: &[Vec<u8>], seen: &mut Seen) -> (Vec<u8>, usize) {
    let longest = units.iter().map(Vec::len).max().unwrap_or(0);
    let sizes: Vec<usize> = [1, 2, 4]
        .into_iter()
        .filter(|&s| longest < 1 << (8 * s))
        .collect();
    let size = sizes[rng.below(sizes.len())];
    seen.length_sizes[size] += 1;
    let mut out = Vec::new();
    for unit in units {
        out.extend(&(unit.len() as u32).to_be_bytes()[4 - size..]);
        out.extend(unit);
    }
    (out, size)
}

fn chunked(stream: &[u8], rng: &mut Rng) -> Vec<Vec<u8>> {
    let mut splitter = AnnexB::new(&Limits::default());
    let mut out = Vec::new();
    let mut rest = stream;
    while !rest.is_empty() {
        let (chunk, tail) = rest.split_at(rng.range(1, 64).min(rest.len()));
        splitter
            .push(chunk, |nal| {
                out.push(nal.to_vec());
                Ok(())
            })
            .unwrap();
        rest = tail;
    }
    splitter
        .finish(|nal| {
            out.push(nal.to_vec());
            Ok(())
        })
        .unwrap();
    out
}

fn check(frames: usize, seed: u64) -> Seen {
    let mut rng = Rng(seed);
    let mut seen = Seen::default();
    for index in 0..frames {
        let units = frame(&mut rng, &mut seen);
        let stream = byte_stream(&mut rng, &units, &mut seen);
        let (prefixed, size) = prefixed(&mut rng, &units, &mut seen);
        let context = format!("seed {seed:#x}, frame {index}, length size {size}");

        let whole: Vec<&[u8]> = annex_b(&stream).collect();
        assert_eq!(whole, units, "annex_b, {context}");
        assert_eq!(chunked(&stream, &mut rng), units, "AnnexB, {context}");
        let counted: Vec<&[u8]> = length_prefixed(&prefixed, size)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(counted, units, "length_prefixed, {context}");

        let walked = read_frame_access(
            AccessFormat::H264 {
                length_size: size as u8,
            },
            &mut SliceSource::new(&prefixed),
            0,
            prefixed.len() as u64,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(
            annex_b_frame_access(&stream, true),
            walked,
            "frame access, {context}"
        );
    }
    seen
}

#[test]
fn byte_stream_and_length_prefixed_frames_give_the_same_units() {
    let seen = check(400, 0x9e37_79b9_7f4a_7c15);
    assert!(seen.escaped > 1_000, "{seen:?}");
    assert!(seen.cabac_zero_words > 50, "{seen:?}");
    assert!(seen.four_byte_start_codes > 100, "{seen:?}");
    assert!(seen.three_byte_start_codes > 100, "{seen:?}");
    assert!(seen.trailing_zeros > 100, "{seen:?}");
    assert!(
        [1, 2, 4].iter().all(|&s| seen.length_sizes[s] > 0),
        "{seen:?}"
    );
    assert!(seen.recovery_points > 50, "{seen:?}");
    assert!(seen.recovery_points_at_read_limit > 0, "{seen:?}");
    assert!(seen.unparsed_slices > 20, "{seen:?}");
}

#[test]
#[ignore = "20 000 frames: run with --release"]
fn twenty_thousand_frames_give_the_same_units() {
    check(20_000, 0x2545_f491_4f6c_dd1d);
}
