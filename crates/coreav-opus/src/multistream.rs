//! The decoder of an Opus track: the packet of every elementary stream of a
//! sample (RFC 7845 5.1.1, RFC 6716 Appendix B), mapped to the output
//! channels, with the header's output gain.

use crate::{Decoder, Error, Head, Source, for_each_stream};

/// A decoder of a whole Opus track, as its identification header describes
/// it: one [`Decoder`] per stream (stereo for the coupled ones), and the
/// channel mapping table that assigns their channels to the output.
///
/// ```
/// use coreav_opus::{Head, MultistreamDecoder};
///
/// // A stereo header of mapping family 0 (RFC 7845 5.1).
/// let mut config = b"OpusHead".to_vec();
/// config.extend([1, 2, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0]);
/// let mut decoder = MultistreamDecoder::new(&Head::from_config(&config).unwrap()).unwrap();
/// let mut planes = Vec::new();
/// // A 20 ms narrowband SILK frame, mono, decoded to both channels.
/// let samples = decoder.decode(&[1 << 3, 0x07, 0xc6, 0x40], &mut planes).unwrap();
/// assert_eq!((samples, planes.len()), (960, 2 * 960));
/// ```
#[derive(Clone, Debug)]
pub struct MultistreamDecoder {
    head: Head,
    streams: Vec<Decoder>,
    /// Each stream's samples of the sample being decoded, interleaved.
    decoded: Vec<Vec<f32>>,
    gain: f32,
}

impl MultistreamDecoder {
    /// A decoder for the track `head` describes.
    pub fn new(head: &Head) -> Result<Self, Error> {
        let coupled = usize::from(head.coupled());
        let streams = (0..usize::from(head.streams()))
            .map(|stream| Decoder::new(if stream < coupled { 2 } else { 1 }))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            head: *head,
            decoded: vec![Vec::new(); streams.len()],
            streams,
            gain: gain(head.output_gain()),
        })
    }

    /// Output channels, in the header's order (RFC 7845 5.1.1).
    pub fn channels(&self) -> usize {
        usize::from(self.head.channels())
    }

    /// Forgets all state, as at the start of the track.
    pub fn reset(&mut self) {
        self.streams.iter_mut().for_each(Decoder::reset);
    }

    /// Decodes one sample of the track into `planes`, which it replaces
    /// with [`Self::channels`] runs of the returned number of samples, in
    /// the header's channel order, the output gain applied; a channel the
    /// table maps to no stream is silence. On error `planes` is left
    /// empty; the streams decoded before the failing one keep their new
    /// state, as for any damaged packet.
    pub fn decode(&mut self, data: &[u8], planes: &mut Vec<f32>) -> Result<usize, Error> {
        let Self {
            streams, decoded, ..
        } = self;
        // Each stream's buffer is overwritten, not cleared and refilled.
        let decoded_all = for_each_stream(data, streams.len(), |stream, packet| {
            match (streams.get_mut(stream), decoded.get_mut(stream)) {
                (Some(decoder), Some(buffer)) => {
                    decoder.decode_packet_into(packet, buffer);
                    Ok(())
                }
                _ => Err(Error::Invalid("Opus stream past the header's count")),
            }
        });
        let samples = match decoded_all {
            Ok(samples) => samples as usize,
            Err(e) => {
                planes.clear();
                return Err(e);
            }
        };
        // A buffer of the same length is overwritten in place.
        planes.resize(self.channels() * samples, 0.0);
        if samples == 0 {
            return Ok(0);
        }
        let gain = self.gain;
        // Stereo from one coupled stream, its channels in order: both planes
        // in one pass over the stream's samples.
        if let (
            2,
            Some(Source::Coupled {
                stream,
                right: false,
            }),
            Some(Source::Coupled {
                stream: s,
                right: true,
            }),
        ) = (self.channels(), self.head.source(0), self.head.source(1))
        {
            if let (true, Some(buffer)) = (stream == s, self.decoded.get(usize::from(stream))) {
                let (left, right) = planes.split_at_mut(samples);
                for ((l, r), pair) in left
                    .iter_mut()
                    .zip(right.iter_mut())
                    .zip(buffer.chunks_exact(2))
                {
                    if let [a, b] = pair {
                        (*l, *r) = (a * gain, b * gain);
                    }
                }
                return Ok(samples);
            }
        }
        for (channel, plane) in planes.chunks_exact_mut(samples).enumerate() {
            let source = match self.head.source(channel) {
                Some(Source::Coupled { stream, right }) => Some((stream, usize::from(right), 2)),
                Some(Source::Mono { stream }) => Some((stream, 0, 1)),
                Some(Source::Silence) | None => None,
            };
            let Some((buffer, first, step)) = source.and_then(|(stream, first, step)| {
                Some((self.decoded.get(usize::from(stream))?, first, step))
            }) else {
                plane.fill(0.0);
                continue;
            };
            if step == 1 {
                for (out, &sample) in plane.iter_mut().zip(buffer.iter()) {
                    *out = sample * gain;
                }
            } else {
                for (out, pair) in plane.iter_mut().zip(buffer.chunks_exact(2)) {
                    *out = pair.get(first).copied().unwrap_or(0.0) * gain;
                }
            }
        }
        Ok(samples)
    }
}

/// The factor of an output gain of `q8` / 256 dB (RFC 7845 5.1):
/// 10^(q8 / 5120), computed as a power of two without the C math library
/// (`docs/design/audio-decode.md`, MP3 "Tables": linking it costs every
/// process its loading).
fn gain(q8: i16) -> f32 {
    if q8 == 0 {
        return 1.0;
    }
    let exponent = f64::from(q8) / 5120.0 * core::f64::consts::LOG2_10;
    // |exponent| < 22: an integer part, and 2^f for f in [0, 1) by its
    // Taylor series in f ln 2, whose terms past the 16th are below 2^-60.
    let mut whole = exponent as i32;
    if f64::from(whole) > exponent {
        whole -= 1;
    }
    let x = (exponent - f64::from(whole)) * core::f64::consts::LN_2;
    let (mut sum, mut term) = (1.0f64, 1.0f64);
    for n in 1..=16 {
        term *= x / f64::from(n);
        sum += term;
    }
    let scale = if whole >= 0 { 2.0f64 } else { 0.5f64 };
    for _ in 0..whole.unsigned_abs() {
        sum *= scale;
    }
    sum as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(channels: u8, family: u8, table: &[u8]) -> Head {
        let mut config = b"OpusHead".to_vec();
        config.extend([1, channels, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, family]);
        config.extend(table);
        Head::from_config(&config).unwrap()
    }

    #[test]
    fn output_gain_is_a_power_of_ten() {
        for (q8, expected) in [
            (0i16, 1.0f64),
            (5120, 10.0),
            (-5120, 0.1),
            (1541, 10f64.powf(1541.0 / 5120.0)),
            (-32768, 10f64.powf(-32768.0 / 5120.0)),
            (32767, 10f64.powf(32767.0 / 5120.0)),
        ] {
            let got = f64::from(gain(q8));
            assert!(
                (got / expected - 1.0).abs() < 1e-6,
                "{q8}: {got} {expected}"
            );
        }
    }

    #[test]
    fn channels_follow_the_mapping_table() {
        // Family 1, three channels: one coupled stream (left, right) and one
        // mono stream, mapped as right, mono, left; a fourth table entry of
        // 255 would be silence.
        let head = head(3, 1, &[2, 1, 1, 2, 0]);
        let mut decoder = MultistreamDecoder::new(&head).unwrap();
        // Stream 0 self-delimited (RFC 6716 Appendix B): TOC, its frame's
        // length, the frame; then stream 1 as an ordinary packet.
        let frame = [0x07, 0xc6, 0x40];
        let mut data = vec![1 << 3 | 4, frame.len() as u8];
        data.extend(frame);
        data.push(1 << 3);
        data.extend(frame);
        let mut planes = Vec::new();
        let samples = decoder.decode(&data, &mut planes).unwrap();
        assert_eq!((samples, planes.len()), (960, 3 * 960));

        let mut stereo = Decoder::new(2).unwrap();
        let mut both = Vec::new();
        stereo
            .decode(&[1 << 3 | 4, 0x07, 0xc6, 0x40], &mut both)
            .unwrap();
        let mut mono = Decoder::new(1).unwrap();
        let mut one = Vec::new();
        mono.decode(&[1 << 3, 0x07, 0xc6, 0x40], &mut one).unwrap();
        let right: Vec<f32> = both.iter().skip(1).step_by(2).copied().collect();
        let left: Vec<f32> = both.iter().step_by(2).copied().collect();
        assert_eq!(&planes[..960], &right[..]);
        assert_eq!(&planes[960..1920], &one[..]);
        assert_eq!(&planes[1920..], &left[..]);
    }

    #[test]
    fn a_damaged_sample_leaves_nothing() {
        let head = head(2, 0, &[]);
        let mut decoder = MultistreamDecoder::new(&head).unwrap();
        let mut planes = vec![1.0];
        assert!(decoder.decode(&[], &mut planes).is_err());
        assert!(planes.is_empty());
    }
}
