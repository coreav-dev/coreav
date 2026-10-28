//! Samples to the output's format, interleaved and little-endian, and the
//! default speaker positions.

use crate::decode::SampleFormat;
use crate::{Pcm, Plane};

/// The speaker positions of `channels` channels in the default order of
/// FLAC (RFC 9639 Table 16), which is also WAV's channel order.
pub(crate) fn default_mask(channels: u16) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        3 => 0x7,
        4 => 0x33,
        5 => 0x37,
        6 => 0x3F,
        7 => 0x70F,
        8 => 0x63F,
        _ => 0,
    }
}

/// Writes samples `offset..offset + count` of every channel of `pcm`,
/// interleaved, in `format`, into `out` from byte `at` on.
///
/// Integers are moved to the output's width by shifting (a 24-bit sample
/// becomes 16-bit by dropping its 8 low bits, a 12-bit one by adding 4 zero
/// bits); floating point is scaled by the output's full scale, rounded to
/// the nearest integer (ties to even) and clipped.
pub(crate) fn interleave(
    pcm: &Pcm,
    offset: usize,
    count: usize,
    format: SampleFormat,
    out: &mut Vec<u8>,
    at: usize,
) {
    let channels = pcm.channels();
    let width = format.bytes();
    let frame = channels.saturating_mul(width);
    let end = at.saturating_add(count.saturating_mul(frame));
    // A buffer that is long enough already is overwritten, not cleared
    // and refilled.
    if out.len() < end {
        out.resize(end, 0);
    }
    let Some(out) = out.get_mut(at..end) else {
        return;
    };
    let end = offset.saturating_add(count);
    // The common case of the lossy codecs, floating point to 16 bits on one
    // or two channels, in loops the compiler vectorizes.
    if format == SampleFormat::S16 {
        let plane = |c| match pcm.plane(c) {
            Some(Plane::Float(samples)) => samples.get(offset..end),
            _ => None,
        };
        match (channels, plane(0), plane(1)) {
            (1, Some(mono), _) => {
                for (o, &v) in out.chunks_exact_mut(2).zip(mono) {
                    if let [a, b] = o {
                        [*a, *b] = to_i16(v).to_le_bytes();
                    }
                }
                return;
            }
            (2, Some(left), Some(right)) => {
                for (o, (&l, &r)) in out.chunks_exact_mut(4).zip(left.iter().zip(right)) {
                    if let [a, b, c, d] = o {
                        [*a, *b] = to_i16(l).to_le_bytes();
                        [*c, *d] = to_i16(r).to_le_bytes();
                    }
                }
                return;
            }
            _ => {}
        }
    }
    // Integers to floating point on one or two channels, the stream's
    // output when it is resampled or downmixed: in single precision, which
    // gives the same value (a power-of-two scale commutes with rounding), in
    // loops the compiler vectorizes.
    if format == SampleFormat::F32 {
        let plane = |c| match pcm.plane(c) {
            Some(Plane::Int { bits, samples }) => {
                samples.get(offset..end).map(|s| (float_scale(bits), s))
            }
            _ => None,
        };
        match (channels, plane(0), plane(1)) {
            (1, Some((scale, mono)), _) => {
                for (o, &v) in out.chunks_exact_mut(4).zip(mono) {
                    if let [a, b, c, d] = o {
                        [*a, *b, *c, *d] = (v as f32 * scale).to_le_bytes();
                    }
                }
                return;
            }
            (2, Some((left_scale, left)), Some((right_scale, right))) => {
                for (o, (&l, &r)) in out.chunks_exact_mut(8).zip(left.iter().zip(right)) {
                    if let [a, b, c, d, e, f, g, h] = o {
                        [*a, *b, *c, *d] = (l as f32 * left_scale).to_le_bytes();
                        [*e, *f, *g, *h] = (r as f32 * right_scale).to_le_bytes();
                    }
                }
                return;
            }
            _ => {}
        }
    }
    for channel in 0..channels {
        let at = channel * width;
        match pcm.plane(channel) {
            Some(Plane::Int { bits, samples }) => {
                let samples = samples.get(offset..end).unwrap_or_default();
                match format {
                    SampleFormat::S16 => {
                        column(out, frame, at, samples, |v| {
                            (shift_to(v, bits, 16) as i16).to_le_bytes()
                        });
                    }
                    SampleFormat::S24 => column(out, frame, at, samples, |v| {
                        let [a, b, c, _] = shift_to(v, bits, 24).to_le_bytes();
                        [a, b, c]
                    }),
                    SampleFormat::S32 => {
                        column(out, frame, at, samples, |v| {
                            shift_to(v, bits, 32).to_le_bytes()
                        });
                    }
                    SampleFormat::F32 => {
                        let scale = float_scale(bits);
                        column(out, frame, at, samples, |v| {
                            (v as f32 * scale).to_le_bytes()
                        });
                    }
                }
            }
            Some(Plane::Float(samples)) => {
                let samples = samples.get(offset..end).unwrap_or_default();
                match format {
                    SampleFormat::S16 => {
                        column(out, frame, at, samples, |v| to_i16(v).to_le_bytes())
                    }
                    SampleFormat::S24 => column(out, frame, at, samples, |v| {
                        let [a, b, c, _] = (to_int(v, 8_388_608.0) as i32).to_le_bytes();
                        [a, b, c]
                    }),
                    SampleFormat::S32 => column(out, frame, at, samples, |v| {
                        (to_int(v, 2_147_483_648.0) as i32).to_le_bytes()
                    }),
                    SampleFormat::F32 => column(out, frame, at, samples, f32::to_le_bytes),
                }
            }
            None => {}
        }
    }
}

/// Writes one channel's converted samples at byte `at` of each frame of
/// `frame` bytes.
#[inline(always)]
fn column<T: Copy, const B: usize>(
    out: &mut [u8],
    frame: usize,
    at: usize,
    samples: &[T],
    convert: impl Fn(T) -> [u8; B],
) {
    for (frame, &v) in out.chunks_exact_mut(frame).zip(samples) {
        if let Some(slot) = frame.get_mut(at..at + B) {
            for (d, s) in slot.iter_mut().zip(convert(v)) {
                *d = s;
            }
        }
    }
}

/// The scale from a `bits`-bit integer sample to floating point,
/// 2^(1 - bits): exact in single precision, so `v as f32 * scale` rounds
/// once, as the exact product would.
fn float_scale(bits: u32) -> f32 {
    (1.0 / (1u64 << bits.saturating_sub(1).min(63)) as f64) as f32
}

#[inline(always)]
fn shift_to(v: i32, bits: u32, target: u32) -> i32 {
    if bits >= target {
        v >> (bits - target)
    } else {
        v << (target - bits)
    }
}

/// `v` scaled by `full`, clipped to the integer range and rounded to the
/// nearest integer, ties to even: adding and subtracting 1.5 * 2^52 leaves
/// the nearest integer under the default rounding mode, for every value
/// below 2^51 (no library call, and the loop vectorizes).
#[inline(always)]
fn to_int(v: f32, full: f64) -> i64 {
    const ROUND: f64 = 6_755_399_441_055_744.0;
    let scaled = (f64::from(v) * full).clamp(-full, full - 1.0);
    ((scaled + ROUND) - ROUND) as i64
}

/// [`to_int`] for 16 bits in single precision: scaling by 2^15 is exact,
/// and adding 1.5 * 2^23 rounds every value below 2^22 to an integer that
/// is then the float's low mantissa bits (1.5 * 2^23 is `0x4B40_0000`), so
/// no float-to-integer conversion is needed and the loop vectorizes. NaN
/// gives 0.
#[inline(always)]
fn to_i16(v: f32) -> i16 {
    const ROUND: f32 = 12_582_912.0;
    let scaled = (v * 32_768.0).clamp(-32_768.0, 32_767.0);
    (scaled + ROUND).to_bits().wrapping_sub(0x4B40_0000) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_to_nearest_ties_to_even_and_clipped() {
        for (v, want) in [
            (0.5f32 / 32_768.0, 0i16),
            (1.5 / 32_768.0, 2),
            (2.5 / 32_768.0, 2),
            (-0.5 / 32_768.0, 0),
            (-1.5 / 32_768.0, -2),
            (0.4 / 32_768.0, 0),
            (1.0, 32_767),
            (-1.0, -32_768),
            (3.0, 32_767),
            (-7.0, -32_768),
            (f32::NAN, 0),
        ] {
            assert_eq!(to_i16(v), want, "{v}");
            assert_eq!(to_int(v, 32_768.0), i64::from(want), "{v}");
        }
        for i in -70_000..70_000 {
            let v = i as f32 / 65_536.0 / 1.37;
            let exact = (f64::from(v) * 32_768.0)
                .round_ties_even()
                .clamp(-32_768.0, 32_767.0) as i16;
            assert_eq!(to_i16(v), exact);
        }
        assert_eq!(to_int(0.75, 2_147_483_648.0), 1_610_612_736);
        assert_eq!(to_int(1.0, 2_147_483_648.0), 2_147_483_647);
    }

    /// Integers to floating point in single precision give the exact
    /// product rounded once, on every path.
    #[test]
    fn integers_to_float_round_once() {
        for bits in 1..=32u32 {
            let low = -(1i64 << (bits - 1));
            let high = (1i64 << (bits - 1)) - 1;
            let mut values: Vec<i32> = (0..4_000i64)
                .map(|i| (low + (i * 2_654_435_761) % (high - low + 1)) as i32)
                .collect();
            values.extend([low as i32, high as i32, 0, -1, 1]);
            let want: Vec<u8> = values
                .iter()
                .flat_map(|&v| {
                    let exact = f64::from(v) / (1u64 << (bits - 1)) as f64;
                    (exact as f32).to_le_bytes()
                })
                .collect();
            for channels in [1usize, 2, 3] {
                let mut pcm = Pcm::new();
                let frames = values.len();
                let samples = pcm.int_buffer(channels, frames, bits);
                for (c, plane) in samples.chunks_exact_mut(frames).enumerate() {
                    for (i, s) in plane.iter_mut().enumerate() {
                        *s = values[(i + c * 7) % frames];
                    }
                }
                let mut out = Vec::new();
                interleave(&pcm, 0, frames, SampleFormat::F32, &mut out, 0);
                for (i, frame) in out.chunks_exact(4 * channels).enumerate() {
                    for c in 0..channels {
                        let k = (i + c * 7) % frames;
                        assert_eq!(
                            &frame[4 * c..4 * c + 4],
                            &want[4 * k..4 * k + 4],
                            "{bits} {channels}"
                        );
                    }
                }
            }
        }
    }
}
