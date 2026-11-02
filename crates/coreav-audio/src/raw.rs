//! Sized signed-16 little-endian PCM input, with a byte-preserving bypass
//! or the independently derived compatibility rate conversion.

mod rate;

use crate::{Error, LimitKind, Limits, OutputFormat, SampleFormat, Sink};
use coreav_isobmff::Source;
use rate::Rate;

/// An explicitly declared raw PCM input format, without container inference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawS16Format {
    rate: u32,
    channels: u16,
}

impl RawS16Format {
    /// Declares mono or stereo at a supported integral sample rate.
    pub fn new(rate: u32, channels: u16) -> Result<Self, Error> {
        if !crate::conversion::RATES.contains(&rate) {
            return Err(Error::Unsupported(
                "raw PCM rate outside 1,000 to 768,000 Hz",
            ));
        }
        if !matches!(channels, 1 | 2) {
            return Err(Error::Unsupported(
                "raw PCM channels other than mono or stereo",
            ));
        }
        Ok(Self { rate, channels })
    }
}

/// A fully validated sized input and its output conversion.
pub struct RawS16Plan {
    input: RawS16Format,
    bytes: u64,
    input_frames: u64,
    output: OutputFormat,
    output_frames: u64,
    rate: Option<Rate>,
    capacity: usize,
}

/// The input and output frames processed by a raw PCM request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawSummary {
    /// Complete input sample frames.
    pub input_frames: u64,
    /// Written output sample frames, including the drain.
    pub output_frames: u64,
    /// Output sample format, rate and channels.
    pub output: OutputFormat,
}

impl RawS16Plan {
    /// Validates framing, output limits and coefficient storage before output starts.
    pub fn new(
        input: RawS16Format,
        output_rate: Option<u32>,
        bytes: u64,
        limits: &Limits,
    ) -> Result<Self, Error> {
        if u32::from(input.channels) > limits.max_channels {
            return Err(Error::LimitExceeded(LimitKind::Channels));
        }
        let frame = u64::from(input.channels) * 2;
        if bytes % frame != 0 {
            return Err(Error::Invalid("raw PCM input ends inside a sample frame"));
        }
        let output_rate = output_rate.unwrap_or(input.rate);
        RawS16Format::new(output_rate, input.channels)?;
        let rate = (output_rate != input.rate)
            .then(|| Rate::new(input.rate, output_rate))
            .transpose()?;
        let input_frames = bytes / frame;
        let output_frames = match &rate {
            Some(rate) => rate.output_frames(input_frames)?,
            None => input_frames,
        };
        if output_frames
            .checked_mul(u64::from(input.channels))
            .is_none_or(|n| n > limits.max_output_samples)
        {
            return Err(Error::LimitExceeded(LimitKind::OutputSamples));
        }
        let needed = rate.as_ref().map_or(frame, |r| r.length as u64 * frame);
        if needed > limits.max_packet_bytes {
            return Err(Error::LimitExceeded(LimitKind::PacketBytes));
        }
        let capacity = usize::try_from(limits.max_packet_bytes.min(65_536))
            .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
        let output = OutputFormat {
            sample_rate: output_rate,
            channels: input.channels,
            channel_mask: crate::convert::default_mask(input.channels),
            format: SampleFormat::S16,
        };
        Ok(Self {
            input,
            bytes,
            input_frames,
            output,
            output_frames,
            rate,
            capacity,
        })
    }

    /// The output known before the first input read or output write.
    pub fn output(&self) -> OutputFormat {
        self.output
    }

    /// Complete output frames after draining.
    pub fn output_frames(&self) -> u64 {
        self.output_frames
    }

    /// Reads one sized source and consumes the conversion, draining exactly once.
    pub fn run<S: Source>(self, mut source: S, sink: &mut dyn Sink) -> Result<RawSummary, Error> {
        if source.len()? != self.bytes {
            return Err(Error::Invalid("raw PCM source size changed"));
        }
        sink.start(&self.output)?;
        let frame = usize::from(self.input.channels) * 2;
        let mut output = Vec::new();
        output
            .try_reserve_exact(self.capacity)
            .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
        let mut reader = Reader::new(source, self.bytes, self.capacity)?;
        match &self.rate {
            None => {
                let chunk = self.capacity / frame * frame;
                let mut offset = 0;
                while offset < self.bytes {
                    let count = usize::try_from((self.bytes - offset).min(chunk as u64))
                        .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
                    sink.write(reader.window(offset, count)?)?;
                    offset += count as u64;
                }
            }
            Some(rate) => {
                let center = (rate.length / 2 - 1) as u64;
                let half = rate.length as u64 / 2;
                let mut position = 0u64;
                let mut phase = 0u64;
                for _ in 0..self.output_frames {
                    let first = position.saturating_sub(center);
                    let end = (position + half + 1).min(self.input_frames);
                    let count = usize::try_from(end - first)
                        .ok()
                        .and_then(|n| n.checked_mul(frame))
                        .ok_or(Error::LimitExceeded(LimitKind::PacketBytes))?;
                    let bytes = reader.window(first * frame as u64, count)?;
                    for channel in 0..usize::from(self.input.channels) {
                        let mut lanes = [0.0f32; 8];
                        for (i, &weight) in rate.row(phase)?.iter().enumerate() {
                            let at = i as i128 + i128::from(position) - i128::from(center);
                            let at = if at < 0 {
                                -at
                            } else if at >= i128::from(self.input_frames) {
                                2 * i128::from(self.input_frames) - 1 - at
                            } else {
                                at
                            };
                            let at = usize::try_from(at - i128::from(first))
                                .ok()
                                .and_then(|n| n.checked_mul(frame))
                                .and_then(|n| n.checked_add(channel * 2))
                                .ok_or(Error::Invalid("raw PCM window"))?;
                            let sample = match bytes.get(at..at + 2) {
                                Some([a, b]) => f32::from(i16::from_le_bytes([*a, *b])) / 32_768.0,
                                _ => return Err(Error::Invalid("raw PCM window")),
                            };
                            let lane = lanes
                                .get_mut(i % 8)
                                .ok_or(Error::Invalid("raw PCM accumulator"))?;
                            *lane = sample.mul_add(weight, *lane);
                        }
                        for width in [4, 2, 1] {
                            let (left, right) = lanes.split_at_mut(width);
                            for (a, b) in left.iter_mut().zip(right) {
                                *a += *b;
                            }
                        }
                        let value = lanes
                            .first()
                            .copied()
                            .ok_or(Error::Invalid("raw PCM accumulator"))?;
                        output.extend_from_slice(&crate::convert::to_i16(value).to_le_bytes());
                    }
                    if output.len() + frame > self.capacity {
                        sink.write(&output)?;
                        output.clear();
                    }
                    phase += rate.step;
                    position += phase / rate.phases;
                    phase %= rate.phases;
                }
                if !output.is_empty() {
                    sink.write(&output)?;
                }
            }
        }
        Ok(RawSummary {
            input_frames: self.input_frames,
            output_frames: self.output_frames,
            output: self.output,
        })
    }
}

struct Reader<S> {
    source: S,
    bytes: Vec<u8>,
    total: u64,
    start: u64,
    capacity: usize,
}

impl<S: Source> Reader<S> {
    fn new(source: S, total: u64, capacity: usize) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
        Ok(Self {
            source,
            bytes,
            total,
            start: 0,
            capacity,
        })
    }

    fn window(&mut self, offset: u64, count: usize) -> Result<&[u8], Error> {
        let end = offset
            .checked_add(count as u64)
            .ok_or(Error::Invalid("raw PCM read range"))?;
        if count > self.capacity || end > self.total {
            return Err(Error::Invalid("raw PCM read range"));
        }
        if offset < self.start || end > self.start + self.bytes.len() as u64 {
            let count = usize::try_from((self.total - offset).min(self.capacity as u64))
                .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
            self.bytes.resize(count, 0);
            self.source.read_exact_at(offset, &mut self.bytes)?;
            self.start = offset;
        }
        let start = usize::try_from(offset - self.start)
            .map_err(|_| Error::Invalid("raw PCM read range"))?;
        self.bytes
            .get(start..start + count)
            .ok_or(Error::Invalid("raw PCM read range"))
    }
}
