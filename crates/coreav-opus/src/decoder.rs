//! The decoder of one Opus stream (RFC 6716 section 4): packets in, 48 kHz
//! samples out.
//!
//! One Opus frame at a time, as RFC 6716 4.5 and the reference's decoding of
//! a frame order the layers: the SILK layer (SILK-only and Hybrid frames),
//! the redundancy flags and size that follow it (4.5.1), the CELT layer
//! (CELT-only frames, and bands 17 and up of Hybrid ones) on the same range
//! decoder, a redundant 5 ms CELT frame on its own range decoder, and the
//! cross-fades of a mode switch: with the redundant frame (4.5.1.4), or with
//! 5 ms of concealment of the previous mode where there is none (4.5.2).

use crate::Error;
use crate::celt::{self, Bands, Celt};
use crate::packet::{Bandwidth, Mode, Packet, Toc};
use crate::range::RangeDecoder;
use crate::resample::Upsampler;
use crate::silk::{self, Layout, Rate, Silk};

/// A decoder of one Opus stream (one elementary stream of a multistream
/// packet, RFC 7845 5.1.1) to 48 kHz, mono or stereo.
///
/// Every mode decodes, with the redundant CELT frames and the cross-fades
/// of mode switches. A frame of 0 or 1 bytes (a lost frame, or the
/// encoder's discontinuous transmission) and a packet the caller reports
/// lost ([`Self::conceal`]) decode to silence: the concealment of 4.4 is
/// not implemented.
///
/// ```
/// use coreav_opus::Decoder;
///
/// let mut decoder = Decoder::new(2).unwrap();
/// let mut pcm = Vec::new();
/// // A 20 ms narrowband SILK frame, mono, decoded to stereo.
/// let frames = decoder.decode(&[1 << 3, 0x07, 0xc6, 0x40], &mut pcm).unwrap();
/// assert_eq!((frames, pcm.len()), (960, 1920));
/// ```
#[derive(Clone, Debug)]
pub struct Decoder {
    channels: usize,
    silk: Silk,
    upsamplers: [Option<Upsampler>; 2],
    /// Whether the last SILK frame was stereo; `None` before the first.
    stereo: Option<bool>,
    /// The range decoder's state after the last frame (RFC 6716 section 6).
    range: u32,
    /// One Opus frame at the SILK rate, per channel.
    buffers: Box<[[i16; silk::MAX_SAMPLES]; 2]>,
    /// One SILK frame at 48 kHz, per channel.
    upsampled: Box<[[i16; UPSAMPLED]; 2]>,
    celt: Celt,
    /// One CELT frame, per channel.
    celt_out: Box<[[f32; celt::MAX_FRAME]; 2]>,
    /// A redundant CELT frame, or 5 ms of concealment, per channel.
    side_out: Box<[[f32; celt::MAX_FRAME]; 2]>,
    /// The mode of the last frame decoded or concealed; `None` before the
    /// first.
    previous: Option<Mode>,
    /// The last frame ended in a redundant CELT frame (a switch from SILK to
    /// CELT), which the next CELT frame continues from.
    previous_redundancy: bool,
    /// The TOC byte of the last packet, which concealment follows.
    toc: Option<Toc>,
    /// The first coded CELT band as last set: 0, or 17 after a Hybrid
    /// frame; concealment depends on it.
    celt_start: usize,
}

/// The samples of a 20 ms SILK frame at 48 kHz.
const UPSAMPLED: usize = 960;

/// Samples per channel of 2.5, 5 and 20 ms at 48 kHz.
const F2_5: usize = 120;
const F5: usize = 240;
const F20: usize = 960;

/// A CELT frame that codes nothing but its silence flag (4.3.1), which a
/// switch from Hybrid to SILK decodes for the CELT layer's fade-out.
const SILENCE: [u8; 2] = [0xff, 0xff];

/// The last coded CELT band for a bandwidth (4.3, Table 55).
fn end_band(bandwidth: Bandwidth) -> usize {
    match bandwidth {
        Bandwidth::Narrow => 13,
        Bandwidth::Medium | Bandwidth::Wide => 17,
        Bandwidth::SuperWide => 19,
        Bandwidth::Full => 21,
    }
}

/// log2 of a CELT frame's size in 2.5 ms units.
fn lm(samples: usize) -> usize {
    match samples {
        0..=120 => 0,
        121..=240 => 1,
        241..=480 => 2,
        _ => 3,
    }
}

impl Decoder {
    /// A decoder with `channels` output channels, 1 or 2, whatever the
    /// packets code: a mono stream is copied to both channels, a stereo
    /// one decoded to mono gets its mid channel (RFC 6716 4.2.8).
    pub fn new(channels: u8) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error::Invalid("an Opus stream decodes to 1 or 2 channels"));
        }
        Ok(Self {
            channels: usize::from(channels),
            silk: Silk::default(),
            upsamplers: [None, None],
            stereo: None,
            range: 0,
            buffers: Box::new([[0; silk::MAX_SAMPLES]; 2]),
            upsampled: Box::new([[0; UPSAMPLED]; 2]),
            celt: Celt::default(),
            celt_out: Box::new([[0.0; celt::MAX_FRAME]; 2]),
            side_out: Box::new([[0.0; celt::MAX_FRAME]; 2]),
            previous: None,
            previous_redundancy: false,
            toc: None,
            celt_start: 0,
        })
    }

    /// The number of output channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The range decoder's final state for the last frame decoded, which
    /// RFC 6716 section 6 compares with the encoder's.
    pub fn final_range(&self) -> u32 {
        self.range
    }

    /// Forgets all state, as at the start of a stream.
    pub fn reset(&mut self) {
        self.reset_silk();
        self.celt.reset();
        self.range = 0;
        self.previous = None;
        self.previous_redundancy = false;
        self.toc = None;
        self.celt_start = 0;
    }

    fn reset_silk(&mut self) {
        self.silk.reset();
        self.upsamplers = [None, None];
        self.stereo = None;
    }

    /// Decodes a packet, appending its samples to `out` interleaved, at
    /// full scale ±1.0, and returns the number of samples per channel.
    /// Nothing is appended on error. CELT's output is not clipped: like the
    /// reference decoder's floating-point output, it may exceed full scale.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<usize, Error> {
        self.decode_packet(&Packet::parse(packet)?, out)
    }

    /// Decodes a packet already parsed, such as one stream's packet of a
    /// multistream sample ([`crate::for_each_stream`]), as [`Self::decode`]
    /// does.
    pub fn decode_packet(
        &mut self,
        packet: &Packet<'_>,
        out: &mut Vec<f32>,
    ) -> Result<usize, Error> {
        let start = out.len();
        out.resize(start + self.packet_len(packet), 0.0);
        Ok(self.fill(packet, out.get_mut(start..).unwrap_or(&mut [])))
    }

    /// Decodes a packet as [`Self::decode_packet`] does, into `out` in place
    /// of what it held, which a buffer of the same length keeps from being
    /// cleared first.
    pub(crate) fn decode_packet_into(&mut self, packet: &Packet<'_>, out: &mut Vec<f32>) -> usize {
        out.resize(self.packet_len(packet), 0.0);
        self.fill(packet, out)
    }

    /// The samples of all channels a packet decodes to.
    fn packet_len(&self, packet: &Packet<'_>) -> usize {
        packet.frames().len() * packet.toc().frame_samples() as usize * self.channels
    }

    /// Decodes `packet` into `out`, interleaved, and returns its samples
    /// per channel.
    fn fill(&mut self, packet: &Packet<'_>, out: &mut [f32]) -> usize {
        let toc = packet.toc();
        self.toc = Some(toc);
        let samples = toc.frame_samples() as usize;
        for (data, target) in packet
            .frames()
            .iter()
            .zip(out.chunks_exact_mut(samples * self.channels))
        {
            self.frame(toc, Some(data), target);
        }
        packet.frames().len() * samples
    }

    /// Conceals a lost packet of `samples` samples per channel (at most
    /// 120 ms), appending them to `out` as [`Self::decode`] does. They are
    /// silence (the concealment of 4.4 is not implemented), decoded as
    /// frames of the last packet's mode, so that the next packet's resets
    /// and cross-fades follow them as they follow a lost frame.
    pub fn conceal(&mut self, samples: usize, out: &mut Vec<f32>) -> usize {
        let samples = samples.min(crate::MAX_PACKET_SAMPLES as usize);
        let start = out.len();
        out.resize(start + samples * self.channels, 0.0);
        // The reference conceals at most a frame of the last packet's
        // duration at a time.
        let step = self
            .toc
            .map_or(F20, |t| t.frame_samples() as usize)
            .max(F2_5);
        let toc = self.toc.unwrap_or(Toc(0));
        for target in out
            .get_mut(start..)
            .unwrap_or(&mut [])
            .chunks_mut(step * self.channels)
        {
            self.frame(toc, None, target);
        }
        samples
    }

    /// Decodes one Opus frame, or conceals one when `data` is `None` or
    /// holds at most a byte (4.4), into `pcm`, interleaved. `toc` is the
    /// TOC of the packet being decoded (the last one when concealing).
    fn frame(&mut self, toc: Toc, data: Option<&[u8]>, pcm: &mut [f32]) {
        let channels = self.channels;
        let data = data.filter(|d| d.len() > 1);
        let audiosize = pcm.len() / channels;
        let mode = match (data, self.previous) {
            (Some(_), _) => toc.mode(),
            (None, Some(previous)) => previous,
            (None, None) => {
                // Nothing to conceal from yet.
                pcm.fill(0.0);
                return;
            }
        };
        // CELT and Hybrid concealment goes 20 ms at a time.
        if data.is_none() && audiosize > F20 && mode != Mode::Silk {
            for chunk in pcm.chunks_mut(F20 * channels) {
                self.frame(toc, None, chunk);
            }
            return;
        }

        let mut transition = data.is_some()
            && self.previous.is_some_and(|previous| {
                (mode == Mode::Celt && previous != Mode::Celt && !self.previous_redundancy)
                    || (mode != Mode::Celt && previous == Mode::Celt)
            });
        let mut faded = [0f32; F5 * 2];
        let fade_len = audiosize.min(F5) * channels;
        if transition && mode == Mode::Celt {
            self.frame(toc, None, faded.get_mut(..fade_len).unwrap_or(&mut []));
        }

        let mut rd = RangeDecoder::new(data.unwrap_or(&[]));
        if mode != Mode::Celt {
            if self.previous == Some(Mode::Celt) {
                self.reset_silk();
            }
            match data {
                Some(_) => self.decode_silk(&mut rd, toc, mode, pcm),
                None => self.conceal_silk(pcm),
            }
        }

        // The redundancy flags and size after the SILK layer (4.5.1).
        let mut len = data.map_or(0, <[u8]>::len);
        let mut redundancy = false;
        let mut celt_to_silk = false;
        let mut redundant: &[u8] = &[];
        let hybrid = mode == Mode::Hybrid;
        if data.is_some()
            && mode != Mode::Celt
            && rd.tell() + 17 + 20 * u32::from(hybrid) <= 8 * len as u32
        {
            redundancy = !hybrid || rd.bit_logp(12);
            if redundancy {
                celt_to_silk = rd.bit_logp(1);
                // At least two bytes: the check above leaves room for them.
                let bytes = if hybrid {
                    rd.uint(256) as usize + 2
                } else {
                    len.saturating_sub((rd.tell() as usize).div_ceil(8))
                };
                match len.checked_sub(bytes) {
                    Some(rest) if rest * 8 >= rd.tell() as usize => {
                        len = rest;
                        redundant = data.and_then(|d| d.get(rest..)).unwrap_or(&[]);
                        rd.shrink(bytes);
                    }
                    // Not a valid packet; the reference's handling, not
                    // normative, decodes no CELT layer from it.
                    _ => {
                        len = 0;
                        redundancy = false;
                    }
                }
            }
        }
        let start = if mode == Mode::Celt { 0 } else { 17 };
        let celt_frame = |start, lm| celt::Frame {
            lm,
            stereo: toc.stereo(),
            bands: Bands {
                start,
                end: end_band(toc.bandwidth()),
            },
        };

        if redundancy {
            transition = false;
        }
        if transition && mode != Mode::Celt {
            self.frame(toc, None, faded.get_mut(..fade_len).unwrap_or(&mut []));
        }

        // The redundant frame of a switch from CELT to SILK comes first,
        // continuing the CELT state (4.5.1.4).
        let mut redundant_range = 0;
        if redundancy && celt_to_silk {
            self.celt_start = 0;
            let mut rd = RangeDecoder::new(redundant);
            self.celt
                .decode(&mut rd, celt_frame(0, 1), channels, &mut self.side_out);
            redundant_range = rd.range();
        }
        self.celt_start = start;

        if mode != Mode::Silk {
            let n = audiosize.min(F20);
            if self.previous.is_some_and(|p| p != mode) && !self.previous_redundancy {
                self.celt.reset();
            }
            match data {
                Some(_) if len > 1 => {
                    self.celt.decode(
                        &mut rd,
                        celt_frame(start, lm(n)),
                        channels,
                        &mut self.celt_out,
                    );
                }
                _ => self.conceal_celt(toc, n),
            }
            add_planar(pcm, &self.celt_out, channels, n, mode == Mode::Celt);
        } else if self.previous == Some(Mode::Hybrid)
            && !(redundancy && celt_to_silk && self.previous_redundancy)
        {
            // The CELT layer fades out through its overlap by decoding a
            // silent frame.
            self.celt_start = 0;
            let mut rd = RangeDecoder::new(&SILENCE);
            self.celt
                .decode(&mut rd, celt_frame(0, 0), channels, &mut self.celt_out);
            add_planar(pcm, &self.celt_out, channels, F2_5.min(audiosize), false);
        }

        let window = celt::window();
        if redundancy && !celt_to_silk {
            // The redundant frame of a switch from SILK to CELT ends this
            // frame, from a clean CELT state (4.5.1.4).
            self.celt.reset();
            self.celt_start = 0;
            let mut rd = RangeDecoder::new(redundant);
            self.celt
                .decode(&mut rd, celt_frame(0, 1), channels, &mut self.side_out);
            redundant_range = rd.range();
            let at = audiosize.saturating_sub(F2_5);
            let tail = pcm.get_mut(at * channels..).unwrap_or(&mut []);
            fade(
                tail,
                |i, c| at_planar(&self.side_out, c, F2_5 + i),
                window,
                channels,
                false,
            );
        }
        if redundancy && celt_to_silk {
            let head = pcm.get_mut(..F2_5 * channels).unwrap_or(&mut []);
            for (i, sample) in head.iter_mut().enumerate() {
                *sample = at_planar(&self.side_out, i % channels, i / channels);
            }
            let next = pcm.get_mut(F2_5 * channels..).unwrap_or(&mut []);
            fade(
                next,
                |i, c| at_planar(&self.side_out, c, F2_5 + i),
                window,
                channels,
                true,
            );
        }
        if transition {
            let faded_at = |i: usize, c: usize| faded.get(i * channels + c).copied().unwrap_or(0.0);
            if audiosize >= F5 {
                if let (Some(head), Some(from)) =
                    (pcm.get_mut(..F2_5 * channels), faded.get(..F2_5 * channels))
                {
                    head.copy_from_slice(from);
                }
                let next = pcm.get_mut(F2_5 * channels..).unwrap_or(&mut []);
                fade(next, |i, c| faded_at(F2_5 + i, c), window, channels, true);
            } else {
                fade(pcm, faded_at, window, channels, true);
            }
        }

        self.range = if len <= 1 {
            0
        } else {
            rd.range() ^ redundant_range
        };
        self.previous = Some(mode);
        self.previous_redundancy = redundancy && !celt_to_silk;
    }

    /// Decodes the SILK layer of an Opus frame (4.2) into `pcm` at 48 kHz.
    fn decode_silk(&mut self, rd: &mut RangeDecoder<'_>, toc: Toc, mode: Mode, pcm: &mut [f32]) {
        let rate = match (mode, toc.bandwidth()) {
            (Mode::Hybrid, _) => Rate::Wide,
            (_, Bandwidth::Narrow) => Rate::Narrow,
            (_, Bandwidth::Medium) => Rate::Medium,
            _ => Rate::Wide,
        };
        let Some(layout) = Layout::new(rate, toc.frame_samples() / 48, toc.stereo()) else {
            pcm.fill(0.0);
            return;
        };
        let [left, right] = &mut *self.buffers;
        let right = (self.channels == 2).then_some(right.as_mut_slice());
        self.silk.decode(rd, layout, left.as_mut_slice(), right);

        let channels = self.channels;
        let mut at = 0;
        for f in 0..layout.frames {
            let range = f * layout.frame_len()..(f + 1) * layout.frame_len();
            let n = range.len() * (48 / rate.khz());
            self.upsample(layout, range);
            let target = pcm.get_mut(at..at + n * channels).unwrap_or(&mut []);
            let [left, right] = &*self.upsampled;
            let scale = |s: i16| f32::from(s) * (1.0 / 32768.0);
            if channels == 1 {
                for (o, &l) in target.iter_mut().zip(left.iter()) {
                    *o = scale(l);
                }
            } else {
                for ((frame, &l), &r) in target
                    .chunks_exact_mut(2)
                    .zip(left.iter())
                    .zip(right.iter())
                {
                    if let [a, b] = frame {
                        (*a, *b) = (scale(l), scale(r));
                    }
                }
            }
            at += n * channels;
        }
    }

    /// Conceals the SILK layer of a lost frame into `pcm`.
    fn conceal_silk(&mut self, pcm: &mut [f32]) {
        pcm.fill(0.0);
    }

    /// Conceals `n` samples of the CELT layer into `celt_out`.
    fn conceal_celt(&mut self, _toc: Toc, n: usize) {
        for channel in self.celt_out.iter_mut() {
            channel.iter_mut().take(n).for_each(|s| *s = 0.0);
        }
    }

    /// Upsamples one SILK frame, `range` of the SILK-rate buffers, into
    /// `upsampled`. Each channel's upsampler starts over at a new rate; a
    /// switch from mono to stereo gives the right channel the left one's
    /// state, and the first mono frame after stereo upsamples the right
    /// channel with its own state, both as the reference decoder does.
    fn upsample(&mut self, layout: Layout, range: core::ops::Range<usize>) {
        let rate = layout.rate;
        let previous = self.stereo.replace(layout.stereo);
        let same_rate = self.upsamplers[0].as_ref().map(Upsampler::rate) == Some(rate);
        let decoded = if layout.stereo { 2 } else { 1 };
        for upsampler in self.upsamplers.iter_mut().take(decoded) {
            if upsampler.as_ref().map(Upsampler::rate) != Some(rate) {
                *upsampler = Some(Upsampler::new(rate));
            }
        }
        if self.channels == 2 && layout.stereo && previous == Some(false) {
            self.upsamplers[1] = self.upsamplers[0].clone();
        }
        let right_own =
            self.channels == 2 && (layout.stereo || (previous == Some(true) && same_rate));
        let channels = if right_own { 2 } else { 1 };
        for ((upsampler, buffer), out) in self
            .upsamplers
            .iter_mut()
            .zip(self.buffers.iter())
            .zip(self.upsampled.iter_mut())
            .take(channels)
        {
            if let (Some(upsampler), Some(input)) = (upsampler, buffer.get(range.clone())) {
                upsampler.process(input, out);
            }
        }
        if self.channels == 2 && !right_own {
            let [left, right] = &mut *self.upsampled;
            right.copy_from_slice(left);
        }
    }
}

/// Sample `i` of channel `c` of planar `planes`, 0 past their end.
fn at_planar(planes: &[[f32; celt::MAX_FRAME]; 2], c: usize, i: usize) -> f32 {
    planes.get(c).and_then(|p| p.get(i)).copied().unwrap_or(0.0)
}

/// Writes (`assign`) or adds the first `n` samples of planar `planes`
/// into interleaved `pcm`.
fn add_planar(
    pcm: &mut [f32],
    planes: &[[f32; celt::MAX_FRAME]; 2],
    channels: usize,
    n: usize,
    assign: bool,
) {
    let [left, right] = planes;
    let combine = |o: &mut f32, v: f32| *o = if assign { v } else { *o + v };
    if channels == 1 {
        for (o, &l) in pcm.iter_mut().zip(left.iter()).take(n) {
            combine(o, l);
        }
        return;
    }
    for ((frame, &l), &r) in pcm
        .chunks_exact_mut(2)
        .zip(left.iter())
        .zip(right.iter())
        .take(n)
    {
        if let [a, b] = frame {
            combine(a, l);
            combine(b, r);
        }
    }
}

/// The cross-fade of a mode switch over the first 2.5 ms of interleaved
/// `pcm`, with the square of the CELT window as the weight: towards `pcm`
/// from `other` (`into_pcm`), or from `pcm` towards `other`.
fn fade(
    pcm: &mut [f32],
    other: impl Fn(usize, usize) -> f32,
    window: &[f32; F2_5],
    channels: usize,
    into_pcm: bool,
) {
    for (i, frame) in pcm.chunks_exact_mut(channels).take(F2_5).enumerate() {
        let w = window.get(i).copied().unwrap_or(1.0);
        let w = w * w;
        for (c, sample) in frame.iter_mut().enumerate() {
            let (from, to) = if into_pcm {
                (other(i, c), *sample)
            } else {
                (*sample, other(i, c))
            };
            *sample = w * to + (1.0 - w) * from;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silk_starts_over_after_celt() {
        let silk = [1 << 3, 0x07, 0xc6, 0x40];
        let mut decoder = Decoder::new(2).unwrap();
        let mut pcm = Vec::new();
        decoder.decode(&silk, &mut pcm).unwrap();
        // Configuration 16: CELT-only, NB, 2.5 ms.
        let frames = decoder.decode(&[16 << 3, 0xff, 0x00], &mut pcm).unwrap();
        assert_eq!((frames, pcm.len()), (120, 1920 + 240));
        pcm.clear();
        decoder.decode(&silk, &mut pcm).unwrap();
        // Past the 5 ms of the switch's cross-fade, the SILK frame decodes
        // as from a new decoder.
        let mut fresh = Vec::new();
        Decoder::new(2).unwrap().decode(&silk, &mut fresh).unwrap();
        assert_eq!(pcm[2 * F5..], fresh[2 * F5..]);
    }

    #[test]
    fn frames_of_every_configuration() {
        // Every configuration, mono and stereo, on arbitrary bytes: every
        // frame decodes to its duration, finite.
        let mut decoder = Decoder::new(2).unwrap();
        let mut seed = 1u32;
        for config in 0u8..32 {
            for stereo in [0, 4] {
                let mut packet = vec![config << 3 | stereo];
                for _ in 0..80 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    packet.push((seed >> 16) as u8);
                }
                let mut pcm = Vec::new();
                let frames = decoder.decode(&packet, &mut pcm).unwrap();
                assert_eq!(pcm.len(), 2 * frames);
                assert_eq!(frames, Toc(config << 3).frame_samples() as usize);
                assert!(pcm.iter().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn concealment_before_any_packet_is_silence() {
        let mut decoder = Decoder::new(1).unwrap();
        let mut pcm = vec![0.5];
        assert_eq!(decoder.conceal(960, &mut pcm), 960);
        assert_eq!(pcm.len(), 961);
        assert!(pcm[1..].iter().all(|&v| v == 0.0));
    }
}
