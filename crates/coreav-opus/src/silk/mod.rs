//! The SILK layer (RFC 6716 4.2): the linear prediction layer of SILK-only
//! and Hybrid frames.
//!
//! [`Silk::decode`] reads the SILK part of one Opus frame: the header bits
//! (4.2.3), the per-frame LBRR flags (4.2.4), the LBRR frames, which it
//! reads past (4.2.5), and the regular frames (4.2.6), each with its side
//! information ([`frame`]) and excitation ([`excitation`]).
//!
//! Choices, beyond what the RFC fixes:
//! - A whole Opus frame at a time (up to three 20 ms frames per channel),
//!   rather than one SILK frame per call: the frames of a channel that
//!   depend on each other (a relative gain or lag, 4.2.7.4 and 4.2.7.6.1)
//!   are then always in the same call, and that state lives in locals.
//! - The LBRR frames are read and dropped: they only serve concealment
//!   of a lost packet, which a decoder of files does not do.

pub(crate) mod excitation;
pub(crate) mod fixed;
pub(crate) mod frame;
pub(crate) mod lsf;
mod stereo;
pub(crate) mod synth;

use crate::range::RangeDecoder;
use crate::tables::silk as t;
use frame::{Coding, Params};

/// The rate SILK runs at (4.2): NB, MB or WB; Hybrid frames use WB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rate {
    Narrow,
    Medium,
    Wide,
}

impl Rate {
    /// The rate in kHz.
    pub(crate) fn khz(self) -> usize {
        match self {
            Self::Narrow => 8,
            Self::Medium => 12,
            Self::Wide => 16,
        }
    }
}

/// The shape of an Opus frame's SILK layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) rate: Rate,
    /// 20 ms frames per channel, 1 to 3; or one 10 ms frame.
    pub(crate) frames: usize,
    /// Subframes per frame: 2 (a 10 ms frame) or 4.
    pub(crate) subframes: usize,
    pub(crate) stereo: bool,
}

impl Layout {
    /// The layout of an Opus frame of `ms` milliseconds (10, 20, 40 or
    /// 60), or `None` for a duration SILK cannot code.
    pub(crate) fn new(rate: Rate, ms: u32, stereo: bool) -> Option<Self> {
        let (frames, subframes) = match ms {
            10 => (1, 2),
            20 => (1, 4),
            40 => (2, 4),
            60 => (3, 4),
            _ => return None,
        };
        Some(Self {
            rate,
            frames,
            subframes,
            stereo,
        })
    }

    fn channels(self) -> usize {
        if self.stereo { 2 } else { 1 }
    }

    /// Samples per frame and channel at the SILK rate.
    pub(crate) fn frame_len(self) -> usize {
        self.subframes * 5 * self.rate.khz()
    }
}

/// The most SILK frames per channel in an Opus frame.
const MAX_FRAMES: usize = 3;

/// The most samples per channel of an Opus frame at the SILK rate: 60 ms
/// at 16 kHz.
pub(crate) const MAX_SAMPLES: usize = MAX_FRAMES * synth::MAX_FRAME;

/// The SILK decoder: both channels' prediction state and the stereo
/// unmixing's.
#[derive(Clone, Debug, Default)]
pub(crate) struct Silk {
    /// The excitation of the frame being decoded.
    pulses: Pulses,
    /// Mid (or mono) and side.
    channels: [synth::Channel; 2],
    /// The rate of the last frame, `None` after a reset.
    rate: Option<Rate>,
    /// The last frame was stereo.
    stereo: bool,
    /// The last 20 ms coded no side channel (4.2.7.2).
    mid_only: bool,
    stereo_state: stereo::Unmixer,
    /// The decoded mid and side signals of the current Opus frame.
    mid: Signal,
    side: Signal,
}

#[derive(Clone, Debug)]
struct Pulses([i32; excitation::BLOCK * excitation::MAX_BLOCKS]);

impl Default for Pulses {
    fn default() -> Self {
        Self([0; excitation::BLOCK * excitation::MAX_BLOCKS])
    }
}

#[derive(Clone, Debug)]
struct Signal([i16; synth::MAX_FRAME]);

impl Default for Signal {
    fn default() -> Self {
        Self([0; synth::MAX_FRAME])
    }
}

/// The flags of an Opus frame's header (4.2.3, 4.2.4), by channel.
#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    vad: [[bool; MAX_FRAMES]; 2],
    lbrr: [[bool; MAX_FRAMES]; 2],
}

impl Silk {
    /// Forgets all state (4.5.2).
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Decodes the SILK layer of one Opus frame into `left` and `right`
    /// at the SILK rate, `layout.frames * layout.frame_len()` samples
    /// each, one sample late (4.2.8). `right` gets the side-unmixed
    /// right channel of a stereo frame, or a copy of `left`; with
    /// `right` absent, `left` gets the mid channel.
    pub(crate) fn decode(
        &mut self,
        rd: &mut RangeDecoder<'_>,
        layout: Layout,
        left: &mut [i16],
        mut right: Option<&mut [i16]>,
    ) {
        // A new rate starts every channel over (4.2.9: the buffers are
        // not available at the new rate); a new side channel starts fresh,
        // with zero stereo weights (4.2.7.1).
        if self.rate != Some(layout.rate) {
            for channel in &mut self.channels {
                channel.reset();
            }
            self.rate = Some(layout.rate);
        }
        if layout.stereo && !self.stereo {
            if let Some(side) = self.channels.get_mut(1) {
                *side = synth::Channel::default();
            }
            self.stereo_state.start_stereo();
        }
        self.stereo = layout.stereo;

        let flags = read_flags(rd, layout);
        self.skip_lbrr(rd, layout, &flags);

        // The regular frames (4.2.6), mid then side for each 20 ms.
        let len = layout.frame_len();
        let mut previous_lag: [Option<i32>; 2] = [None, None];
        for i in 0..layout.frames {
            let mut weights = [0, 0];
            let mut mid_only = false;
            if layout.stereo {
                weights = read_stereo_weights(rd);
                // The mid-only flag, unless the side is active (4.2.7.2).
                if !flags.vad[1].get(i).copied().unwrap_or(false) {
                    mid_only = rd.icdf(&t::STEREO_MID_ONLY_ICDF, 8) == 1;
                }
                // A side channel coded again starts from silence.
                if !mid_only && self.mid_only {
                    if let Some(side) = self.channels.get_mut(1) {
                        side.reset();
                    }
                }
            }
            for channel in 0..layout.channels() {
                if channel == 1 && mid_only {
                    previous_lag[1] = None;
                    self.side.0.fill(0);
                    continue;
                }
                let coding = if i == 0 {
                    Coding::Independent
                } else if channel == 1 && self.mid_only {
                    Coding::NoLtpScaling
                } else {
                    Coding::Conditional
                };
                let active = flags
                    .vad
                    .get(channel)
                    .and_then(|v| v.get(i))
                    .copied()
                    .unwrap_or(false);
                let lag = previous_lag.get(channel).copied().flatten();
                let params = self.read_frame(rd, layout, active, coding, lag);
                if let Some(slot) = previous_lag.get_mut(channel) {
                    *slot = params.pitch.map(|p| p.lag);
                }
                let (state, signal) = match channel {
                    0 => (self.channels.get_mut(0), &mut self.mid),
                    _ => (self.channels.get_mut(1), &mut self.side),
                };
                if let Some(state) = state {
                    state.decode(
                        layout.rate,
                        layout.subframes,
                        &params,
                        &self.pulses.0,
                        &mut signal.0,
                    );
                }
            }
            self.mid_only = mid_only;

            // Stereo unmixing (4.2.8), or the mono delay that matches it.
            let at = i * len;
            let mid = self.mid.0.get(..len).unwrap_or(&[]);
            let side = self.side.0.get(..len).unwrap_or(&[]);
            let left = left.get_mut(at..at + len).unwrap_or(&mut []);
            match right
                .as_deref_mut()
                .map(|r| r.get_mut(at..at + len).unwrap_or(&mut []))
            {
                Some(right) if layout.stereo => {
                    self.stereo_state
                        .unmix(mid, side, weights, layout.rate.khz(), left, right);
                }
                Some(right) => {
                    self.stereo_state.delay(mid, left);
                    right.copy_from_slice(left);
                }
                None => self.stereo_state.delay(mid, left),
            }
        }
    }

    /// Reads past the LBRR frames (4.2.5): mid and side interleaved for
    /// each 20 ms, each coded relative to the channel's previous LBRR frame
    /// when there is one.
    fn skip_lbrr(&mut self, rd: &mut RangeDecoder<'_>, layout: Layout, flags: &Flags) {
        let mut previous_lag: [Option<i32>; 2] = [None, None];
        for i in 0..layout.frames {
            for channel in 0..layout.channels() {
                let coded = |c: usize, i: usize| {
                    flags
                        .lbrr
                        .get(c)
                        .and_then(|f| f.get(i))
                        .copied()
                        .unwrap_or(false)
                };
                if !coded(channel, i) {
                    continue;
                }
                if layout.stereo && channel == 0 {
                    let _ = read_stereo_weights(rd);
                    if !coded(1, i) {
                        // The flag is read but there is nothing to act on.
                        rd.icdf(&t::STEREO_MID_ONLY_ICDF, 8);
                    }
                }
                let coding = if i > 0 && coded(channel, i - 1) {
                    Coding::Conditional
                } else {
                    Coding::Independent
                };
                let lag = previous_lag.get(channel).copied().flatten();
                let params = self.read_frame(rd, layout, true, coding, lag);
                if let Some(slot) = previous_lag.get_mut(channel) {
                    *slot = params.pitch.map(|p| p.lag);
                }
            }
        }
    }

    /// Reads one frame's side information and excitation.
    fn read_frame(
        &mut self,
        rd: &mut RangeDecoder<'_>,
        layout: Layout,
        active: bool,
        coding: Coding,
        previous_lag: Option<i32>,
    ) -> Params {
        let params = Params::read(
            rd,
            layout.rate,
            layout.subframes,
            active,
            coding,
            previous_lag,
        );
        excitation::read(
            rd,
            params.signal,
            params.high_offset,
            layout.frame_len(),
            &mut self.pulses.0,
        );
        params
    }
}

/// The VAD and LBRR flags of each channel (4.2.3, 4.2.4).
fn read_flags(rd: &mut RangeDecoder<'_>, layout: Layout) -> Flags {
    let mut flags = Flags::default();
    let mut any_lbrr = [false; 2];
    for (vad, lbrr) in flags
        .vad
        .iter_mut()
        .zip(any_lbrr.iter_mut())
        .take(layout.channels())
    {
        for flag in vad.iter_mut().take(layout.frames) {
            *flag = rd.bit_logp(1);
        }
        *lbrr = rd.bit_logp(1);
    }
    for (lbrr, &any) in flags.lbrr.iter_mut().zip(&any_lbrr).take(layout.channels()) {
        if !any {
            continue;
        }
        let bits = match layout.frames {
            2 => rd.icdf(&t::LBRR_FLAGS_40MS_ICDF, 8) + 1,
            3 => rd.icdf(&t::LBRR_FLAGS_60MS_ICDF, 8) + 1,
            _ => 1,
        };
        for (i, flag) in lbrr.iter_mut().take(layout.frames).enumerate() {
            *flag = bits >> i & 1 == 1;
        }
    }
    flags
}

/// Reads the stereo prediction weights (4.2.7.1), `[w0_Q13, w1_Q13]`.
fn read_stereo_weights(rd: &mut RangeDecoder<'_>) -> [i32; 2] {
    let joint = rd.icdf(&t::STEREO_STAGE1_ICDF, 8);
    let mut weights = [0i32; 2];
    for (n, weight) in weights.iter_mut().enumerate() {
        let fine = rd.icdf(&t::UNIFORM3_ICDF, 8);
        let step = rd.icdf(&t::UNIFORM5_ICDF, 8);
        let coarse = if n == 0 { joint / 5 } else { joint % 5 };
        let index = fine + 3 * coarse;
        let low = t::STEREO_WEIGHTS_Q13
            .get(index)
            .map_or(0, |&w| i32::from(w));
        let high = t::STEREO_WEIGHTS_Q13
            .get(index + 1)
            .map_or(low, |&w| i32::from(w));
        // 6554 is 0.1 in Q16.
        *weight = low + (((high - low) * 6554) >> 16) * (2 * step as i32 + 1);
    }
    weights[0] -= weights[1];
    weights
}

#[cfg(test)]
mod tests;
