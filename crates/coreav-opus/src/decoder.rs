//! The decoder of one Opus stream (RFC 6716 section 4): packets in, 48 kHz
//! samples out.

use crate::Error;
use crate::celt::{self, Bands, Celt};
use crate::packet::{Bandwidth, Mode, Packet};
use crate::range::RangeDecoder;
use crate::resample::Upsampler;
use crate::silk::{self, Layout, Rate, Silk};

/// A decoder of one Opus stream (one elementary stream of a multistream
/// packet, RFC 7845 5.1.1) to 48 kHz, mono or stereo.
///
/// So far it decodes SILK-only and CELT-only packets; Hybrid packets, and
/// SILK-only frames that carry a redundant CELT frame (RFC 6716 4.5.1, which
/// encoders write before a switch to CELT), are [`Error::Unsupported`], and
/// a switch between SILK and CELT starts the new mode from a clean state
/// without the reference decoder's cross-fade, until the hybrid layer
/// lands. Lost frames and the empty frames of discontinuous transmission
/// decode to silence and leave the state as it was, until concealment
/// lands.
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
    /// The mode of the last packet; `None` before the first.
    previous: Option<Mode>,
}

/// The samples of a 20 ms SILK frame at 48 kHz.
const UPSAMPLED: usize = 960;

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
            previous: None,
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
        let packet = Packet::parse(packet)?;
        let result = match packet.toc().mode() {
            Mode::Silk => self.decode_silk(&packet, out),
            Mode::Celt => Ok(self.decode_celt(&packet, out)),
            Mode::Hybrid => return Err(Error::Unsupported("Opus Hybrid mode")),
        };
        if result.is_ok() {
            self.previous = Some(packet.toc().mode());
        }
        result
    }

    /// Decodes a CELT-only packet (RFC 6716 4.3).
    fn decode_celt(&mut self, packet: &Packet<'_>, out: &mut Vec<f32>) -> usize {
        let toc = packet.toc();
        if self.previous.is_some_and(|m| m != Mode::Celt) {
            // A new mode starts from a clean state (RFC 6716 4.5.2).
            self.celt.reset();
        }
        // SILK starts over after CELT-only frames.
        self.reset_silk();
        let end = match toc.bandwidth() {
            Bandwidth::Narrow => 13,
            Bandwidth::Medium | Bandwidth::Wide => 17,
            Bandwidth::SuperWide => 19,
            Bandwidth::Full => 21,
        };
        let samples = toc.frame_samples() as usize;
        let lm = match samples {
            120 => 0,
            240 => 1,
            480 => 2,
            _ => 3,
        };
        let frame = celt::Frame {
            lm,
            stereo: toc.stereo(),
            bands: Bands { start: 0, end },
        };
        let start = out.len();
        let len = samples * self.channels;
        out.resize(start + packet.frames().len() * len, 0.0);
        for (data, target) in packet.frames().iter().zip(
            out.get_mut(start..)
                .unwrap_or(&mut [])
                .chunks_exact_mut(len),
        ) {
            if data.len() <= 1 {
                // Discontinuous transmission or a lost frame: silence for
                // now, concealment later.
                self.range = 0;
                continue;
            }
            let mut rd = RangeDecoder::new(data);
            self.celt
                .decode(&mut rd, frame, self.channels, &mut self.celt_out);
            self.range = rd.range();
            for (i, o) in target.iter_mut().enumerate() {
                let (channel, t) = (i % self.channels, i / self.channels);
                *o = self
                    .celt_out
                    .get(channel)
                    .and_then(|c| c.get(t))
                    .copied()
                    .unwrap_or(0.0);
            }
        }
        packet.frames().len() * samples
    }

    /// Decodes a SILK-only packet (RFC 6716 4.2).
    fn decode_silk(&mut self, packet: &Packet<'_>, out: &mut Vec<f32>) -> Result<usize, Error> {
        let toc = packet.toc();
        let rate = match toc.bandwidth() {
            Bandwidth::Narrow => Rate::Narrow,
            Bandwidth::Medium => Rate::Medium,
            _ => Rate::Wide,
        };
        let samples = toc.frame_samples() as usize;
        let layout = Layout::new(rate, toc.frame_samples() / 48, toc.stereo())
            .ok_or(Error::Invalid("SILK frame duration"))?;

        let start = out.len();
        out.resize(start + packet.frames().len() * samples * self.channels, 0.0);
        let mut at = start;
        for frame in packet.frames() {
            if frame.len() <= 1 {
                // A frame of 0 or 1 bytes carries nothing: the encoder's
                // discontinuous transmission or a lost frame (RFC 6716
                // 3.2.1, 4.4). Silence for now, concealment later.
                self.range = 0;
                at += samples * self.channels;
                continue;
            }
            let mut rd = RangeDecoder::new(frame);
            let [left, right] = &mut *self.buffers;
            let right = (self.channels == 2).then_some(right.as_mut_slice());
            self.silk
                .decode(&mut rd, layout, left.as_mut_slice(), right);
            // Enough bits left for a redundant CELT frame after the SILK
            // data (4.5.1.1), which is not decoded yet: the SILK state has
            // advanced, but the packet's output and range would be wrong.
            if rd.tell() + 17 <= 8 * frame.len() as u32 {
                out.truncate(start);
                return Err(Error::Unsupported("Opus redundant CELT frames"));
            }
            self.range = rd.range();

            for f in 0..layout.frames {
                let range = f * layout.frame_len()..(f + 1) * layout.frame_len();
                let n = range.len() * (48 / rate.khz());
                self.upsample(layout, range);
                let target = out.get_mut(at..at + n * self.channels).unwrap_or(&mut []);
                for (i, o) in target.iter_mut().enumerate() {
                    let channel = i % self.channels;
                    let sample = self
                        .upsampled
                        .get(channel)
                        .and_then(|c| c.get(i / self.channels))
                        .copied()
                        .unwrap_or(0);
                    *o = f32::from(sample) * (1.0 / 32768.0);
                }
                at += n * self.channels;
            }
        }
        Ok(packet.frames().len() * samples)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redundant_celt_frames_are_refused_whole() {
        let mut decoder = Decoder::new(2).unwrap();
        let mut pcm = vec![0.25];
        // A 20 ms NB SILK frame with 40 more bytes: room for redundancy.
        let mut packet = vec![1 << 3, 0x07, 0xc6, 0x40];
        packet.extend([0x55; 40]);
        assert!(matches!(
            decoder.decode(&packet, &mut pcm),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(pcm, [0.25]);
    }

    #[test]
    fn celt_only_packets_clear_the_silk_state() {
        let mut decoder = Decoder::new(2).unwrap();
        let mut pcm = Vec::new();
        decoder
            .decode(&[1 << 3, 0x07, 0xc6, 0x40], &mut pcm)
            .unwrap();
        assert!(decoder.stereo.is_some());
        // Configuration 16: CELT-only, NB, 2.5 ms.
        let frames = decoder.decode(&[16 << 3, 0xff, 0x00], &mut pcm).unwrap();
        assert_eq!((frames, pcm.len()), (120, 1920 + 240));
        assert!(decoder.stereo.is_none() && decoder.upsamplers.iter().all(Option::is_none));
    }

    #[test]
    fn celt_frames_of_every_size_and_bandwidth() {
        // Configurations 16 to 31, mono and stereo, on arbitrary bytes:
        // every frame decodes to its duration, within full scale.
        let mut decoder = Decoder::new(2).unwrap();
        let mut seed = 1u32;
        for config in 16u8..32 {
            for stereo in [0, 4] {
                let mut packet = vec![config << 3 | stereo];
                for _ in 0..60 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    packet.push((seed >> 16) as u8);
                }
                let mut pcm = Vec::new();
                let frames = decoder.decode(&packet, &mut pcm).unwrap();
                assert_eq!(pcm.len(), 2 * frames);
                assert_eq!(frames, 120 << (config & 3));
                assert!(pcm.iter().all(|v| v.is_finite()));
            }
        }
    }
}
