//! Channel downmixing to mono or stereo (`docs/design/audio-decode.md`,
//! "Resampling and downmixing"): ITU-R BS.775-4 Annex 4 Table 2 for the
//! five main channels, its Annex 5 for mono to stereo, and the same roles
//! for the other speaker positions of WAV's channel mask.

use crate::Error;

/// √½, the gain BS.775 gives a channel shared between two.
const HALF_POWER: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// The gains of one speaker position into the left and right channels of
/// a stereo downmix, by its bit in WAV's `dwChannelMask` (bit 0 front left
/// to bit 17 top back right). The mono downmix of a position is √½ times
/// the sum of its two gains, which gives BS.775's mono row (front left and
/// right √½, centre 1, surrounds ½).
fn stereo_gains(bit: u32) -> (f32, f32) {
    match bit {
        // Front left and right; the front left and right of centre; the
        // top front left and right.
        0 | 6 | 12 => (1.0, 0.0),
        1 | 7 | 14 => (0.0, 1.0),
        // Front centre, top centre and top front centre: the centre.
        2 | 11 | 13 => (HALF_POWER, HALF_POWER),
        // Back and side left and right, top back left and right: the
        // surrounds.
        4 | 9 | 15 => (HALF_POWER, 0.0),
        5 | 10 | 17 => (0.0, HALF_POWER),
        // Back centre and top back centre: a mono surround, split equally
        // between the two surrounds.
        8 | 16 => (0.5, 0.5),
        // Low-frequency effects (bit 3): BS.775 leaves it out.
        _ => (0.0, 0.0),
    }
}

/// A downmix: `outputs × inputs` gains, row by row.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Matrix {
    pub(crate) inputs: usize,
    pub(crate) outputs: usize,
    pub(crate) gains: Vec<f32>,
    /// The output's speaker positions.
    pub(crate) mask: u32,
}

impl Matrix {
    /// The downmix of `channels` channels at the speaker positions `mask`
    /// (WAV's channel order: the set bits, lowest first) to `target`
    /// channels, 1 or 2. A mask that does not name every channel is
    /// unknown, except for one channel (the centre) and two (left and
    /// right); an unknown layout is mixed to mono with equal gains and not
    /// to stereo. The gains are scaled so that no output exceeds full
    /// scale: when a row's absolute gains sum to more than 1, every row is
    /// divided by the largest sum.
    pub(crate) fn new(channels: u16, mask: u32, target: u16) -> Result<Self, Error> {
        let inputs = usize::from(channels);
        let outputs = usize::from(target);
        let out_mask = match target {
            1 => 0x4,
            2 => 0x3,
            _ => {
                return Err(Error::Unsupported(
                    "downmixing to other than 1 or 2 channels",
                ));
            }
        };
        let positions: Option<Vec<u32>> = if mask.count_ones() == u32::from(channels) {
            Some((0..32).filter(|bit| mask & (1 << bit) != 0).collect())
        } else {
            match channels {
                1 => Some(vec![2]),
                2 => Some(vec![0, 1]),
                _ => None,
            }
        };
        let mut gains = vec![0.0f32; inputs.saturating_mul(outputs)];
        match positions {
            Some(positions) => {
                for (input, &bit) in positions.iter().enumerate() {
                    let (left, right) = stereo_gains(bit);
                    if outputs == 1 {
                        if let Some(g) = gains.get_mut(input) {
                            *g = HALF_POWER * (left + right);
                        }
                    } else {
                        if let Some(g) = gains.get_mut(input) {
                            *g = left;
                        }
                        if let Some(g) = gains.get_mut(inputs + input) {
                            *g = right;
                        }
                    }
                }
            }
            None if outputs == 1 => {
                let share = 1.0 / inputs.max(1) as f32;
                gains.iter_mut().for_each(|g| *g = share);
            }
            None => {
                return Err(Error::Unsupported(
                    "downmixing channels without speaker positions to stereo",
                ));
            }
        }
        let largest = gains
            .chunks(inputs.max(1))
            .map(|row| row.iter().map(|g| g.abs()).sum::<f32>())
            .fold(0.0f32, f32::max);
        if largest > 1.0 {
            gains.iter_mut().for_each(|g| *g /= largest);
        }
        Ok(Self {
            inputs,
            outputs,
            gains,
            mask: out_mask,
        })
    }

    /// Mixes interleaved frames of `inputs` channels (`f32`, little-endian)
    /// and appends each output channel's samples to `out[channel]`.
    pub(crate) fn mix(&self, bytes: &[u8], out: &mut [Vec<f32>]) {
        let frame = self.inputs.saturating_mul(4);
        if frame == 0 {
            return;
        }
        let frames = bytes.len() / frame;
        for (o, plane) in out.iter_mut().enumerate().take(self.outputs) {
            let row = self
                .gains
                .get(o * self.inputs..(o + 1) * self.inputs)
                .unwrap_or_default();
            plane.reserve(frames);
            // The common rows, stereo to one channel, in a loop the
            // compiler vectorizes.
            if let [a, b] = row {
                plane.extend(bytes.chunks_exact(8).map(|f| {
                    let (l, r) = (sample(f, 0), sample(f, 1));
                    a * l + b * r
                }));
                continue;
            }
            plane.extend(bytes.chunks_exact(frame).map(|f| {
                row.iter()
                    .enumerate()
                    .map(|(c, g)| g * sample(f, c))
                    .sum::<f32>()
            }));
        }
    }
}

/// Appends channel `channel` of interleaved `f32` frames of `channels`
/// channels to `out`.
pub(crate) fn deinterleave(bytes: &[u8], channels: usize, channel: usize, out: &mut Vec<f32>) {
    let frame = channels.saturating_mul(4);
    if frame == 0 {
        return;
    }
    out.reserve(bytes.len() / frame);
    // One or two channels, the common case, with the frame's size known to
    // the compiler.
    match channels {
        1 => out.extend(bytes.chunks_exact(4).map(|f| sample(f, 0))),
        2 => out.extend(bytes.chunks_exact(8).map(|f| sample(f, channel))),
        _ => out.extend(bytes.chunks_exact(frame).map(|f| sample(f, channel))),
    }
}

#[inline(always)]
fn sample(frame: &[u8], channel: usize) -> f32 {
    match frame.get(channel * 4..channel * 4 + 4) {
        Some(&[a, b, c, d]) => f32::from_le_bytes([a, b, c, d]),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gains(channels: u16, mask: u32, target: u16) -> Vec<f32> {
        Matrix::new(channels, mask, target)
            .map(|m| m.gains)
            .unwrap_or_default()
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    #[test]
    fn bs775_rows_scaled_to_full_scale() {
        // Stereo to mono: √½ each, scaled by √2 to ½.
        assert!(close(&gains(2, 0x3, 1), &[0.5, 0.5]));
        // Mono to stereo: √½ each (Annex 5 1.1), below full scale.
        assert!(close(&gains(1, 0x4, 2), &[HALF_POWER, HALF_POWER]));
        // 5.1 to stereo: L + √½ C + √½ Ls over 1 + √2.
        let s = 1.0 + 2.0 * HALF_POWER;
        let h = HALF_POWER / s;
        assert!(close(
            &gains(6, 0x3F, 2),
            &[1.0 / s, 0.0, h, 0.0, h, 0.0, 0.0, 1.0 / s, h, 0.0, 0.0, h]
        ));
        // 5.1 to mono: √½ L + √½ R + C + ½ Ls + ½ Rs, over their sum.
        let m = 2.0 * HALF_POWER + 1.0 + 1.0;
        assert!(close(
            &gains(6, 0x3F, 1),
            &[
                HALF_POWER / m,
                HALF_POWER / m,
                1.0 / m,
                0.0,
                0.5 / m,
                0.5 / m
            ]
        ));
    }

    #[test]
    fn unknown_layouts() {
        assert!(close(&gains(3, 0, 1), &[1.0 / 3.0; 3]));
        assert!(Matrix::new(3, 0, 2).is_err());
        assert!(Matrix::new(6, 0x3F, 3).is_err());
        assert!(close(&gains(2, 0, 1), &[0.5, 0.5]));
    }
}
