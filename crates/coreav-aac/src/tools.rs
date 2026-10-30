//! The spectral tools after a channel is read: perceptual noise
//! substitution (ISO/IEC 14496-3 4.6.13), M/S and intensity stereo
//! (4.6.8.1, 4.6.8.2) and temporal noise shaping (4.6.9).

use crate::ics::{
    Bands, Channel, INTENSITY_HCB, INTENSITY_HCB2, Info, MAX_BANDS, NOISE_HCB, pow2_eighth,
};

/// `ms_mask_present` and `ms_used` of a channel pair.
#[derive(Clone, Copy)]
pub(crate) struct MsMask {
    pub(crate) present: u8,
    pub(crate) used: [[bool; MAX_BANDS]; 8],
}

impl Default for MsMask {
    fn default() -> Self {
        Self {
            present: 0,
            used: [[false; MAX_BANDS]; 8],
        }
    }
}

impl MsMask {
    /// Whether band `sfb` of group `g` is M/S coded: `ms_used`, or every
    /// band with `ms_mask_present` 2.
    fn on(&self, g: usize, sfb: usize) -> bool {
        self.present == 2
            || (self.present == 1
                && self
                    .used
                    .get(g)
                    .and_then(|row| row.get(sfb))
                    .copied()
                    .unwrap_or(false))
    }
}

/// The random numbers of noise substitution: a 32-bit linear
/// congruential generator, seeded per frame from the packet's bytes and
/// its position in the stream, so that a frame's noise depends on that
/// packet alone and a packet repeated frame after frame (stationary noise)
/// still gets new noise each time. The standard leaves the generator to
/// the decoder (4.6.13.3, `gen_rand_vector()`).
pub(crate) struct Noise<'a> {
    state: u32,
    /// The packet and position not yet hashed: most frames have no noise
    /// band, so the seed is computed on first use.
    seed: Option<(&'a [u8], u64)>,
}

impl<'a> Noise<'a> {
    /// The generator of the frame coded in `packet`, packet `position` of
    /// the stream: FNV-1a of its bytes, then of the position's.
    pub(crate) fn for_packet(packet: &'a [u8], position: u64) -> Self {
        Self {
            state: 0,
            seed: Some((packet, position)),
        }
    }

    /// The state, seeded if it was not yet.
    fn state(&mut self) -> u32 {
        if let Some((packet, position)) = self.seed.take() {
            let mut hash = 0x811c_9dc5u32;
            for &b in packet.iter().chain(&position.to_le_bytes()) {
                hash = (hash ^ u32::from(b)).wrapping_mul(0x0100_0193);
            }
            self.state = hash;
        }
        self.state
    }

    #[inline(always)]
    fn next(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        (self.state as i32) as f32
    }

    /// Fills `out` with random values whose energy is `amplitude`^2; a
    /// band whose random values all came out zero stays zero.
    fn fill(&mut self, out: &mut [f32], amplitude: f32) {
        self.state();
        let mut energy = 0f32;
        for v in out.iter_mut() {
            *v = self.next();
            energy += *v * *v;
        }
        let gain = if energy > 0.0 {
            amplitude / energy.sqrt()
        } else {
            0.0
        };
        for v in out.iter_mut() {
            *v *= gain;
        }
    }
}

/// Every band below `max_sfb` of every window of `info`: (group, band,
/// start, end), `start..end` the band's coefficients in the de-interleaved
/// spectrum.
pub(crate) fn bands_of(
    info: Info,
    bands: &Bands,
) -> impl Iterator<Item = (usize, usize, usize, usize)> {
    let short = info.short();
    let window_len = if short {
        bands.short_len()
    } else {
        bands.frame
    };
    let offsets = info.offsets(bands);
    let mut first = 0;
    (0..info.groups).flat_map(move |g| {
        let len = info.group_len.get(g).copied().unwrap_or(0) as usize;
        let windows = first..first + len;
        first += len;
        windows.flat_map(move |w| {
            (0..info.max_sfb).map(move |sfb| {
                let start = offsets.get(sfb).copied().unwrap_or(0) as usize;
                let end = offsets.get(sfb + 1).copied().unwrap_or(0) as usize;
                (g, sfb, w * window_len + start, w * window_len + end)
            })
        })
    })
}

fn codebook(channel: &Channel, g: usize, sfb: usize) -> u8 {
    channel
        .cb
        .get(g)
        .and_then(|row| row.get(sfb))
        .copied()
        .unwrap_or(0)
}

fn value(channel: &Channel, g: usize, sfb: usize) -> i32 {
    channel
        .sf
        .get(g)
        .and_then(|row| row.get(sfb))
        .copied()
        .unwrap_or(0)
}

/// The amplitude of a noise band of energy `nrg` (4.6.13.3): 2^(nrg / 4),
/// normalized as the channel's spectrum is.
fn noise_amplitude(channel: &Channel, g: usize, sfb: usize, scale: [f32; 2]) -> f32 {
    let s = if channel.info.short() {
        scale[1]
    } else {
        scale[0]
    };
    pow2_eighth(value(channel, g, sfb).saturating_mul(2)) * s
}

/// Noise substitution of a single channel.
pub(crate) fn noise(channel: &mut Channel, bands: &Bands, rng: &mut Noise<'_>, scale: [f32; 2]) {
    if channel.noise_bands == 0 {
        return;
    }
    for (g, sfb, start, end) in bands_of(channel.info, bands) {
        if codebook(channel, g, sfb) == NOISE_HCB {
            let amplitude = noise_amplitude(channel, g, sfb, scale);
            if let Some(out) = channel.spec.get_mut(start..end) {
                rng.fill(out, amplitude);
            }
        }
    }
}

/// Noise substitution of a channel pair with a common window: a band
/// that is noise in both channels and M/S coded uses one random vector
/// for both (4.6.13.3).
pub(crate) fn noise_pair(
    left: &mut Channel,
    right: &mut Channel,
    ms: &MsMask,
    bands: &Bands,
    rng: &mut Noise<'_>,
    scale: [f32; 2],
) {
    if left.noise_bands == 0 && right.noise_bands == 0 {
        return;
    }
    for (g, sfb, start, end) in bands_of(left.info, bands) {
        let l = codebook(left, g, sfb) == NOISE_HCB;
        let r = codebook(right, g, sfb) == NOISE_HCB;
        if !l && !r {
            continue;
        }
        let saved = rng.state();
        if l {
            let amplitude = noise_amplitude(left, g, sfb, scale);
            if let Some(out) = left.spec.get_mut(start..end) {
                rng.fill(out, amplitude);
            }
        }
        if r {
            let amplitude = noise_amplitude(right, g, sfb, scale);
            let mut shared = Noise {
                state: saved,
                seed: None,
            };
            let generator = if l && ms.on(g, sfb) {
                &mut shared
            } else {
                &mut *rng
            };
            if let Some(out) = right.spec.get_mut(start..end) {
                generator.fill(out, amplitude);
            }
        }
    }
}

/// M/S stereo, then intensity stereo, of a channel pair with a common
/// window (4.6.8.1.3, 4.6.8.2.3). M/S is not applied to a band that is
/// intensity coded, or noise in either channel. Returns the intensity
/// bands under an `ms_mask_present` of 2 ([`crate::Frame`]).
pub(crate) fn stereo(left: &mut Channel, right: &mut Channel, ms: &MsMask, bands: &Bands) -> u32 {
    let mut all_ms = 0;
    for (g, sfb, start, end) in bands_of(left.info, bands) {
        let rcb = codebook(right, g, sfb);
        if rcb == INTENSITY_HCB || rcb == INTENSITY_HCB2 {
            all_ms += u32::from(ms.present == 2);
            let sign = if rcb == INTENSITY_HCB { 1.0 } else { -1.0 };
            let invert = if ms.present == 1 && ms.on(g, sfb) {
                -1.0
            } else {
                1.0
            };
            let scale = sign * invert * pow2_eighth(value(right, g, sfb).saturating_mul(-2));
            if let (Some(l), Some(r)) = (left.spec.get(start..end), right.spec.get_mut(start..end))
            {
                for (b, &a) in r.iter_mut().zip(l) {
                    *b = scale * a;
                }
            }
        } else if ms.on(g, sfb) && codebook(left, g, sfb) != NOISE_HCB && rcb != NOISE_HCB {
            if let (Some(l), Some(r)) = (
                left.spec.get_mut(start..end),
                right.spec.get_mut(start..end),
            ) {
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    let (m, s) = (*a, *b);
                    *a = m + s;
                    *b = m - s;
                }
            }
        }
    }
    all_ms
}

/// Temporal noise shaping of a channel (4.6.9.3, `tns_decode_frame()`).
pub(crate) fn tns(channel: &mut Channel, bands: &Bands) {
    if !channel.tns.present {
        return;
    }
    let short = channel.info.short();
    let window_len = if short {
        bands.short_len()
    } else {
        bands.frame
    };
    let offsets = channel.info.offsets(bands);
    let windows = if short { 8 } else { 1 };
    let num_swb = offsets.len().saturating_sub(1);
    let max_bands = if short {
        bands.tns_short
    } else {
        bands.tns_long
    };
    let limit = max_bands.min(channel.info.max_sfb);
    for w in 0..windows {
        let count = channel.tns.count.get(w).copied().unwrap_or(0) as usize;
        let mut bottom = num_swb;
        for f in 0..count {
            let Some(filter) = channel.tns.filters.get(w).and_then(|ws| ws.get(f)).copied() else {
                break;
            };
            let top = bottom;
            bottom = top.saturating_sub(filter.length);
            if filter.order == 0 {
                continue;
            }
            let start = offsets.get(bottom.min(limit)).copied().unwrap_or(0) as usize;
            let end = offsets.get(top.min(limit)).copied().unwrap_or(0) as usize;
            if end <= start {
                continue;
            }
            let base = w * window_len;
            let Some(data) = channel.spec.get_mut(base + start..base + end) else {
                continue;
            };
            let lpc = filter.lpc.get(..filter.order).unwrap_or_default();
            ar_filter(data, lpc, filter.downward);
        }
    }
}

/// y(n) = x(n) - lpc\[1\] y(n - 1) - ... - lpc\[order\] y(n - order), in
/// place, upward or downward, the state starting at zero.
fn ar_filter(data: &mut [f32], lpc: &[f32], downward: bool) {
    // Filtered in place, so y(n - i) is the output already written; a
    // downward filter runs on the band reversed.
    if downward {
        data.reverse();
    }
    for n in 0..data.len() {
        let (past, rest) = data.split_at_mut(n);
        if let Some(x) = rest.first_mut() {
            let mut y = *x;
            for (a, &p) in lpc.iter().zip(past.iter().rev()) {
                y -= a * p;
            }
            *x = y;
        }
    }
    if downward {
        data.reverse();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ar_filter_is_all_pole() {
        // y(n) = x(n) - 0.5 y(n - 1) on an impulse: 1, -0.5, 0.25.
        let mut data = [1.0, 0.0, 0.0];
        ar_filter(&mut data, &[0.5], false);
        assert_eq!(data, [1.0, -0.5, 0.25]);
        let mut data = [0.0, 0.0, 1.0];
        ar_filter(&mut data, &[0.5], true);
        assert_eq!(data, [0.25, -0.5, 1.0]);
    }

    #[test]
    fn a_repeated_packet_gets_new_noise() {
        let packet = [7u8; 40];
        let (mut a, mut b, mut c) = ([0f32; 16], [0f32; 16], [0f32; 16]);
        Noise::for_packet(&packet, 5).fill(&mut a, 1.0);
        Noise::for_packet(&packet, 6).fill(&mut b, 1.0);
        Noise::for_packet(&packet, 5).fill(&mut c, 1.0);
        assert_ne!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn the_tns_filter_is_its_recursion() {
        let lpc = [0.5f32, -0.25, 0.125];
        let x: Vec<f32> = (0..20).map(|i| ((i * 37) % 11) as f32 - 5.0).collect();
        for downward in [false, true] {
            let mut got = x.clone();
            ar_filter(&mut got, &lpc, downward);
            let order: Vec<usize> = if downward {
                (0..x.len()).rev().collect()
            } else {
                (0..x.len()).collect()
            };
            let mut want = vec![0f32; x.len()];
            for (k, &n) in order.iter().enumerate() {
                let mut y = x[n];
                for (i, a) in lpc.iter().enumerate() {
                    if k > i {
                        y -= a * want[order[k - 1 - i]];
                    }
                }
                want[n] = y;
            }
            assert_eq!(got, want, "downward {downward}");
        }
    }

    #[test]
    fn noise_has_the_coded_energy() {
        let mut rng = Noise::for_packet(&[1, 2, 3], 0);
        let mut band = [0f32; 16];
        rng.fill(&mut band, 3.0);
        let energy: f32 = band.iter().map(|v| v * v).sum();
        assert!((energy - 9.0).abs() < 1e-3, "{energy}");
    }
}
