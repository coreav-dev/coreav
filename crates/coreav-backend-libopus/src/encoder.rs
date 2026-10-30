//! The [`OpusEncoder`] over libopus.

use opus::{Application, Bitrate, Channels, Encoder};

/// The sample rate libopus is given: 48 kHz, the rate Opus codes at.
pub const SAMPLE_RATE: u32 = 48_000;

/// Samples per channel of one frame: 20 ms at 48 kHz, libopus's default
/// frame duration.
pub const FRAME_SAMPLES: usize = 960;

/// One libopus encoder for mono or stereo at 48 kHz: the `audio`
/// application, unconstrained variable bit rate (libopus's defaults) at the
/// given target, one 20 ms frame per packet.
pub struct OpusEncoder {
    encoder: Encoder,
    channels: usize,
}

impl OpusEncoder {
    /// An encoder of `channels` (1 or 2) channels at `bitrate` bits a
    /// second. Err is libopus's message.
    pub fn new(channels: u8, bitrate: u32) -> Result<Self, String> {
        let layout = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => return Err(format!("{channels} channels: the encoder takes 1 or 2")),
        };
        let bits = i32::try_from(bitrate).map_err(|_| format!("bit rate {bitrate}"))?;
        let mut encoder =
            Encoder::new(SAMPLE_RATE, layout, Application::Audio).map_err(|e| e.to_string())?;
        encoder
            .set_bitrate(Bitrate::Bits(bits))
            .map_err(|e| e.to_string())?;
        encoder.set_vbr(true).map_err(|e| e.to_string())?;
        encoder
            .set_vbr_constraint(false)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            encoder,
            channels: usize::from(channels),
        })
    }

    /// The samples of delay at the start of the decoded stream, which a
    /// player skips (the Opus pre-skip).
    pub fn lookahead(&mut self) -> Result<u32, String> {
        let samples = self.encoder.get_lookahead().map_err(|e| e.to_string())?;
        u32::try_from(samples).map_err(|_| format!("a lookahead of {samples} samples"))
    }

    /// Encodes one frame, [`FRAME_SAMPLES`] samples of every channel
    /// interleaved, into `out`; returns the packet's length.
    pub fn encode(&mut self, frame: &[f32], out: &mut [u8]) -> Result<usize, String> {
        if frame.len() != FRAME_SAMPLES * self.channels {
            return Err(format!(
                "a frame of {} samples, not {}",
                frame.len(),
                FRAME_SAMPLES * self.channels
            ));
        }
        self.encoder
            .encode_float(frame, out)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(channels: usize, frame: usize) -> Vec<f32> {
        (0..FRAME_SAMPLES * channels)
            .map(|i| {
                let t = (frame * FRAME_SAMPLES + i / channels) as f32 / SAMPLE_RATE as f32;
                0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn a_second_of_sine_makes_twenty_millisecond_packets() {
        for channels in [1, 2] {
            let mut encoder = OpusEncoder::new(channels, 64_000).unwrap();
            assert!(encoder.lookahead().unwrap() <= 3_840);
            let mut out = [0; 4_000];
            let mut bytes = 0;
            for frame in 0..50 {
                let len = encoder
                    .encode(&sine(usize::from(channels), frame), &mut out)
                    .unwrap();
                assert!((1..=out.len()).contains(&len));
                // RFC 6716 section 3.1: configs 3, 7, 11, 13, 15, 19, 23,
                // 27 and 31 are 20 ms; one frame (code 0) or more.
                let config = out[0] >> 3;
                assert!(
                    matches!(config, 3 | 7 | 11 | 13 | 15 | 19 | 23 | 27 | 31),
                    "{config}"
                );
                assert_eq!(out[0] & 0x04 != 0, channels == 2);
                bytes += len;
            }
            // About the target over one second, far from silence or excess.
            assert!((2_000..=16_000).contains(&bytes), "{bytes}");
        }
    }

    #[test]
    fn other_layouts_and_frame_lengths_are_refused() {
        assert!(OpusEncoder::new(3, 64_000).is_err());
        let mut encoder = OpusEncoder::new(2, 96_000).unwrap();
        let mut out = [0; 4_000];
        assert!(encoder.encode(&[0.0; FRAME_SAMPLES], &mut out).is_err());
    }
}
