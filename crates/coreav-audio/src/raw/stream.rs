use super::{RawS16Format, RawSummary, rate::Rate, render, sample};
use crate::{Error, LimitKind, Limits, OutputFormat, Pcm, SampleFormat, Sink};

/// Output serialization and the optional signed-16 compatibility rate change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawS16Output {
    /// Signed-16 samples, at the input rate unless another rate is declared.
    S16 {
        /// The requested sample rate in Hz, or the input rate.
        sample_rate: Option<u32>,
    },
    /// Floating-point samples at the input rate, with full scale at ±1.0.
    F32,
}

/// An explicitly declared mono floating-point PCM input format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawF32Format {
    rate: u32,
}

impl RawF32Format {
    /// Declares mono floating-point samples at a supported integral sample rate.
    pub fn new(rate: u32, channels: u16) -> Result<Self, Error> {
        RawS16Format::new(rate, channels)?;
        if channels != 1 {
            return Err(Error::Unsupported("raw F32 channels other than mono"));
        }
        Ok(Self { rate })
    }

    /// The declared sample rate in Hz.
    pub fn rate(self) -> u32 {
        self.rate
    }

    /// The declared mono channel count.
    pub fn channels(self) -> u16 {
        1
    }
}

/// Final serialization of unquantized floating-point input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawF32Output {
    /// Floating-point samples with an optional compatibility rate change.
    F32 {
        /// The requested sample rate in Hz, or the input rate.
        sample_rate: Option<u32>,
    },
    /// Signed-16 samples, quantized once after any compatibility rate change.
    S16 {
        /// The requested sample rate in Hz, or the input rate.
        sample_rate: Option<u32>,
    },
}

#[derive(Clone, Copy)]
enum Input {
    S16(RawS16Format),
    F32(RawF32Format),
    F32Identity { rate: u32, channels: u16 },
}

impl Input {
    fn rate(self) -> u32 {
        match self {
            Self::S16(format) => format.rate(),
            Self::F32(format) => format.rate(),
            Self::F32Identity { rate, .. } => rate,
        }
    }

    fn channels(self) -> u16 {
        match self {
            Self::S16(format) => format.channels(),
            Self::F32(format) => format.channels(),
            Self::F32Identity { channels, .. } => channels,
        }
    }

    fn format(self) -> SampleFormat {
        match self {
            Self::S16(_) => SampleFormat::S16,
            Self::F32(_) | Self::F32Identity { .. } => SampleFormat::F32,
        }
    }

    fn frame(self) -> usize {
        usize::from(self.channels()) * self.format().bytes()
    }

    fn sample(self, bytes: &[u8], at: usize) -> Result<f32, Error> {
        match self {
            Self::S16(_) => sample(bytes, at),
            Self::F32(_) | Self::F32Identity { .. } => match bytes.get(at..at + 4) {
                Some([a, b, c, d]) => {
                    let value = f32::from_le_bytes([*a, *b, *c, *d]);
                    if !value.is_finite() {
                        return Err(Error::Invalid("nonfinite raw F32 sample"));
                    }
                    Ok(value)
                }
                _ => Err(Error::Invalid("raw PCM window")),
            },
        }
    }
}

enum Mode {
    Copy,
    Convert,
    Rate {
        rate: Rate,
        ring: Vec<u8>,
        position: u64,
        phase: u64,
    },
}

/// Bounded sequential signed-16 PCM conversion into a caller-owned sink.
///
/// Every write must contain complete interleaved input frames. Any write
/// failure ends the session: subsequent writes and finish return an error
/// without calling the sink again. Finishing consumes and drains it once.
pub struct RawS16Stream<'a>(Stream<'a>);

impl<'a> RawS16Stream<'a> {
    /// Validates and allocates bounded storage before starting output.
    ///
    /// Accepted input samples and emitted output samples are independently
    /// bounded by `max_output_samples`. Writes must contain whole frames,
    /// but may exceed the retained packet storage.
    pub fn new(
        input: RawS16Format,
        output: RawS16Output,
        limits: &Limits,
        sink: &'a mut dyn Sink,
    ) -> Result<Self, Error> {
        let (rate, format) = match output {
            RawS16Output::S16 { sample_rate } => (sample_rate, SampleFormat::S16),
            RawS16Output::F32 => (None, SampleFormat::F32),
        };
        Stream::new(Input::S16(input), rate, format, limits, sink).map(Self)
    }

    /// Accepts complete interleaved signed-16 little-endian input frames.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.0.write(bytes)
    }

    /// Freezes the input length and drains the conversion exactly once.
    pub fn finish(self) -> Result<RawSummary, Error> {
        self.0.finish()
    }
}

/// Bounded F32 packing and mono rate conversion without input quantization.
///
/// Writes must contain complete finite samples. Nonfinite input or any other
/// write failure ends the session without further sink calls. Floating-point
/// output retains the accumulator; signed-16 output quantizes only at the end.
pub struct RawF32Stream<'a>(Stream<'a>);

impl<'a> RawF32Stream<'a> {
    /// Copies finite mono or stereo F32 at its unchanged sample rate.
    ///
    /// No target rate is accepted; the mono compatibility-rate API stays separate.
    pub fn identity(
        sample_rate: u32,
        channels: u16,
        limits: &Limits,
        sink: &'a mut dyn Sink,
    ) -> Result<Self, Error> {
        Self::packing(sample_rate, channels, SampleFormat::F32, limits, sink)
    }

    /// Packs finite native-rate floating-point PCM as F32 or S16 without mixing.
    pub fn packing(
        sample_rate: u32,
        channels: u16,
        format: SampleFormat,
        limits: &Limits,
        sink: &'a mut dyn Sink,
    ) -> Result<Self, Error> {
        if !matches!(format, SampleFormat::F32 | SampleFormat::S16) {
            return Err(Error::Unsupported(
                "native F32 packing other than F32 or S16",
            ));
        }
        RawS16Format::new(sample_rate, channels)?;
        Stream::new(
            Input::F32Identity {
                rate: sample_rate,
                channels,
            },
            None,
            format,
            limits,
            sink,
        )
        .map(Self)
    }

    /// Validates and allocates bounded storage before starting output.
    ///
    /// Accepted input samples and emitted output samples are independently
    /// bounded by `max_output_samples`. Writes may exceed retained storage.
    pub fn new(
        input: RawF32Format,
        output: RawF32Output,
        limits: &Limits,
        sink: &'a mut dyn Sink,
    ) -> Result<Self, Error> {
        let (rate, format) = match output {
            RawF32Output::F32 { sample_rate } => (sample_rate, SampleFormat::F32),
            RawF32Output::S16 { sample_rate } => (sample_rate, SampleFormat::S16),
        };
        Stream::new(Input::F32(input), rate, format, limits, sink).map(Self)
    }

    /// Accepts complete finite floating-point little-endian samples.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.0.write(bytes)
    }

    /// Freezes the input length and drains the conversion exactly once.
    pub fn finish(self) -> Result<RawSummary, Error> {
        self.0.finish()
    }
}

struct Stream<'a> {
    input: Input,
    output: OutputFormat,
    mode: Mode,
    limits: Limits,
    capacity: usize,
    bytes: Vec<u8>,
    pcm: Pcm,
    input_frames: u64,
    output_frames: u64,
    failed: bool,
    sink: &'a mut dyn Sink,
}

impl<'a> Stream<'a> {
    fn new(
        input: Input,
        output_rate: Option<u32>,
        format: SampleFormat,
        limits: &Limits,
        sink: &'a mut dyn Sink,
    ) -> Result<Self, Error> {
        if u32::from(input.channels()) > limits.max_channels {
            return Err(Error::LimitExceeded(LimitKind::Channels));
        }
        let output_rate = output_rate.unwrap_or(input.rate());
        RawS16Format::new(output_rate, input.channels())?;
        let frame = input.frame();
        let output_frame = usize::from(input.channels()) * format.bytes();
        let capacity = usize::try_from(limits.max_packet_bytes.min(65_536))
            .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
        let capacity = capacity / output_frame * output_frame;
        if capacity < output_frame || frame as u64 > limits.max_packet_bytes {
            return Err(Error::LimitExceeded(LimitKind::PacketBytes));
        }
        let mode = if output_rate != input.rate() {
            let rate = Rate::new(input.rate(), output_rate)?;
            let length = (rate.length + 1) * frame;
            if length as u64 > limits.max_packet_bytes {
                return Err(Error::LimitExceeded(LimitKind::PacketBytes));
            }
            let mut ring = Vec::new();
            ring.try_reserve_exact(length)
                .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
            ring.resize(length, 0);
            Mode::Rate {
                rate,
                ring,
                position: 0,
                phase: 0,
            }
        } else if format == input.format() {
            Mode::Copy
        } else {
            Mode::Convert
        };
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
        let mut pcm = Pcm::new();
        if !matches!(mode, Mode::Copy) {
            let channels = usize::from(input.channels());
            if (channels * size_of::<f32>()) as u64 > limits.max_packet_bytes {
                return Err(Error::LimitExceeded(LimitKind::PacketBytes));
            }
            let mut samples = Vec::new();
            samples
                .try_reserve_exact(channels)
                .map_err(|_| Error::LimitExceeded(LimitKind::PacketBytes))?;
            samples.resize(channels, 0.0);
            pcm.swap_float(channels, 1, &mut samples);
        }
        let output = OutputFormat {
            sample_rate: output_rate,
            channels: input.channels(),
            channel_mask: crate::convert::default_mask(input.channels()),
            format,
        };
        sink.start(&output)?;
        Ok(Self {
            input,
            output,
            mode,
            limits: *limits,
            capacity,
            bytes,
            pcm,
            input_frames: 0,
            output_frames: 0,
            failed: false,
            sink,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Invalid("raw PCM stream has failed"));
        }
        self.failed = true;
        self.write_inner(bytes)?;
        self.failed = false;
        Ok(())
    }

    fn write_inner(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let frame = self.input.frame();
        if bytes.len() % frame != 0 {
            return Err(Error::Invalid("raw PCM write ends inside a sample frame"));
        }
        let total = self
            .input_frames
            .checked_add((bytes.len() / frame) as u64)
            .ok_or(Error::LimitExceeded(LimitKind::OutputSamples))?;
        self.check_samples(total)?;
        if matches!(self.mode, Mode::Copy) {
            for chunk in bytes.chunks(self.capacity) {
                if self.input.format() == SampleFormat::F32 {
                    for at in (0..chunk.len()).step_by(SampleFormat::F32.bytes()) {
                        self.input.sample(chunk, at)?;
                    }
                }
                self.sink.write(chunk)?;
            }
            self.input_frames = total;
            self.output_frames = total;
            return Ok(());
        }
        for bytes in bytes.chunks_exact(frame) {
            if self.input.format() == SampleFormat::F32 {
                self.input.sample(bytes, 0)?;
            }
            match &mut self.mode {
                Mode::Copy => return Err(Error::Invalid("raw PCM stream mode")),
                Mode::Convert => {
                    for (channel, out) in self
                        .pcm
                        .float_buffer(usize::from(self.input.channels()), 1)
                        .iter_mut()
                        .enumerate()
                    {
                        *out = self
                            .input
                            .sample(bytes, channel * self.input.format().bytes())?;
                    }
                    self.input_frames += 1;
                    self.append()?;
                }
                Mode::Rate { rate, ring, .. } => {
                    let at = (self.input_frames % (rate.length + 1) as u64) as usize * frame;
                    ring.get_mut(at..at + frame)
                        .ok_or(Error::Invalid("raw PCM window"))?
                        .copy_from_slice(bytes);
                    self.input_frames += 1;
                    while self.ready() {
                        self.emit()?;
                    }
                }
            }
        }
        Ok(())
    }

    fn ready(&self) -> bool {
        match &self.mode {
            Mode::Rate { rate, position, .. } => {
                self.input_frames > rate.length as u64
                    && position
                        .checked_add(rate.length as u64 / 2)
                        .is_some_and(|end| end < self.input_frames)
            }
            _ => false,
        }
    }

    fn emit(&mut self) -> Result<(), Error> {
        let Mode::Rate {
            rate,
            ring,
            position,
            phase,
        } = &mut self.mode
        else {
            return Err(Error::Invalid("raw PCM stream mode"));
        };
        let frame = self.input.frame();
        let slots = (rate.length + 1) as u64;
        render(
            rate,
            *position,
            *phase,
            self.input_frames,
            self.pcm.float_buffer(usize::from(self.input.channels()), 1),
            |at, channel| {
                if at >= self.input_frames || at < self.input_frames.saturating_sub(slots) {
                    return Err(Error::Invalid("raw PCM window"));
                }
                self.input.sample(
                    ring,
                    (at % slots) as usize * frame + channel * self.input.format().bytes(),
                )
            },
        )?;
        *phase += rate.step;
        *position = position
            .checked_add(*phase / rate.phases)
            .ok_or(Error::LimitExceeded(LimitKind::OutputSamples))?;
        *phase %= rate.phases;
        self.append()
    }

    fn check_samples(&self, frames: u64) -> Result<(), Error> {
        if frames
            .checked_mul(u64::from(self.input.channels()))
            .is_none_or(|samples| samples > self.limits.max_output_samples)
        {
            return Err(Error::LimitExceeded(LimitKind::OutputSamples));
        }
        Ok(())
    }

    fn append(&mut self) -> Result<(), Error> {
        let frames = self
            .output_frames
            .checked_add(1)
            .ok_or(Error::LimitExceeded(LimitKind::OutputSamples))?;
        self.check_samples(frames)?;
        let at = self.bytes.len();
        crate::convert::interleave(&self.pcm, 0, 1, self.output.format, &mut self.bytes, at);
        self.output_frames = frames;
        if self.bytes.len() == self.capacity {
            self.sink.write(&self.bytes)?;
            self.bytes.clear();
        }
        Ok(())
    }

    fn finish(mut self) -> Result<RawSummary, Error> {
        if self.failed {
            return Err(Error::Invalid("raw PCM stream has failed"));
        }
        if let Mode::Rate { rate, .. } = &self.mode {
            let frames = rate.output_frames(self.input_frames)?;
            self.check_samples(frames)?;
            while self.output_frames < frames {
                self.emit()?;
            }
        }
        if !self.bytes.is_empty() {
            self.sink.write(&self.bytes)?;
        }
        Ok(RawSummary {
            input_frames: self.input_frames,
            output_frames: self.output_frames,
            output: self.output,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

    use super::*;
    use crate::raw::RawS16Plan;
    use coreav_isobmff::SliceSource;

    #[derive(Default)]
    struct Keep {
        starts: usize,
        writes: usize,
        bytes: Vec<u8>,
        largest: usize,
        fail_start: bool,
        fail_write: bool,
    }

    impl Sink for Keep {
        fn start(&mut self, _: &OutputFormat) -> Result<(), Error> {
            self.starts += 1;
            if self.fail_start {
                return Err(Error::Output(std::io::ErrorKind::BrokenPipe.into()));
            }
            Ok(())
        }

        fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
            self.writes += 1;
            if self.fail_write {
                return Err(Error::Output(std::io::ErrorKind::BrokenPipe.into()));
            }
            self.largest = self.largest.max(bytes.len());
            self.bytes.extend_from_slice(bytes);
            Ok(())
        }
    }

    fn signal(frames: usize, channels: u16) -> Vec<u8> {
        (0..frames * usize::from(channels))
            .flat_map(|n| {
                let sample = match n % 17 {
                    0 => i16::MIN,
                    1 => i16::MAX,
                    _ => (n as u32).wrapping_mul(43_973).wrapping_add(173) as i16,
                };
                sample.to_le_bytes()
            })
            .collect()
    }

    fn stream(
        input: RawS16Format,
        output: RawS16Output,
        bytes: &[u8],
        partition: &[usize],
    ) -> (Keep, RawSummary) {
        let mut sink = Keep::default();
        let mut stream = RawS16Stream::new(input, output, &Limits::default(), &mut sink).unwrap();
        let frame = usize::from(input.channels) * 2;
        let mut at = 0;
        for count in partition.iter().cycle() {
            if at == bytes.len() {
                break;
            }
            let end = bytes.len().min(at + count * frame);
            stream.write(&bytes[at..end]).unwrap();
            at = end;
        }
        let summary = stream.finish().unwrap();
        (sink, summary)
    }

    #[test]
    fn native_stereo_float_packing_uses_the_registered_s16_quantizer() {
        let values = [
            1.0f32,
            -1.0,
            0.5 / 32768.0,
            1.5 / 32768.0,
            -1.5 / 32768.0,
            2.0,
        ];
        let bytes: Vec<u8> = values.into_iter().flat_map(f32::to_le_bytes).collect();
        let mut sink = Keep::default();
        let mut stream =
            RawF32Stream::packing(48000, 2, SampleFormat::S16, &Limits::default(), &mut sink)
                .unwrap();
        stream.write(&bytes).unwrap();
        let summary = stream.finish().unwrap();
        assert_eq!(summary.output_frames, 3);
        assert_eq!(
            sink.bytes,
            [32767i16, -32768, 0, 2, -2, 32767]
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>()
        );
        let mut stream =
            RawF32Stream::packing(48000, 2, SampleFormat::S16, &Limits::default(), &mut sink)
                .unwrap();
        let bad: Vec<u8> = [0.0f32, f32::NAN]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        assert!(stream.write(&bad).is_err());
        assert!(stream.finish().is_err());
    }

    #[test]
    fn stereo_float_identity_is_byte_exact_without_opening_rate_conversion() {
        let values = [
            0.0f32,
            -0.0,
            0.25,
            -0.25,
            1.5,
            -1.75,
            f32::from_bits(1),
            -f32::from_bits(1),
        ];
        let bytes: Vec<u8> = values
            .into_iter()
            .cycle()
            .take(32_770)
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut sink = Keep::default();
        let limits = Limits {
            max_packet_bytes: 17,
            ..Limits::default()
        };
        let mut stereo = RawF32Stream::identity(48000, 2, &limits, &mut sink).unwrap();
        for chunk in bytes.chunks(24) {
            stereo.write(chunk).unwrap();
        }
        let summary = stereo.finish().unwrap();
        assert_eq!(sink.bytes, bytes);
        assert!(sink.largest <= 17);
        assert_eq!(summary.input_frames, 16_385);
        assert_eq!(summary.output_frames, 16_385);
        assert!(RawF32Format::new(48000, 2).is_err());
    }

    #[test]
    fn stereo_float_identity_checks_every_channel_and_poisoned_writes() {
        for channel in [0, 1] {
            for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut sink = Keep::default();
                let mut stream =
                    RawF32Stream::identity(48000, 2, &Limits::default(), &mut sink).unwrap();
                let good = [0.25f32, -0.25]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>();
                stream.write(&good).unwrap();
                let mut values = [0.0f32; 2];
                values[channel] = bad;
                let bytes = values
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>();
                assert!(stream.write(&bytes).is_err());
                assert!(stream.write(&good).is_err());
                assert!(stream.finish().is_err());
                assert_eq!(sink.writes, 1);
                assert_eq!(sink.bytes, good);
            }
        }
        let mut sink = Keep::default();
        assert!(RawF32Stream::identity(48000, 3, &Limits::default(), &mut sink).is_err());
        assert!(RawF32Stream::identity(0, 2, &Limits::default(), &mut sink).is_err());
        assert_eq!(sink.starts, 0);
    }

    #[test]
    fn s16_identity_is_byte_exact_for_large_and_partitioned_writes() {
        for channels in [1, 2] {
            let format = RawS16Format::new(44100, channels).unwrap();
            assert_eq!(format.rate(), 44100);
            assert_eq!(format.channels(), channels);
            let bytes = signal(100_003, channels);
            for rate in [None, Some(44100)] {
                for partition in [&[usize::MAX / 4][..], &[1, 13, 2, 257][..]] {
                    let (sink, summary) = stream(
                        format,
                        RawS16Output::S16 { sample_rate: rate },
                        &bytes,
                        partition,
                    );
                    assert_eq!(sink.bytes, bytes);
                    assert_eq!(sink.starts, 1);
                    assert!(sink.largest <= 65_536);
                    assert_eq!(summary.input_frames, 100_003);
                    assert_eq!(summary.output_frames, 100_003);
                }
            }
        }
    }

    #[test]
    fn float_identity_serializes_every_integer_at_its_exact_power_scale() {
        let bytes: Vec<u8> = (i16::MIN..=i16::MAX).flat_map(i16::to_le_bytes).collect();
        let expected: Vec<u8> = (i16::MIN..=i16::MAX)
            .flat_map(|n| (f32::from(n) * (1.0 / 32_768.0)).to_le_bytes())
            .collect();
        for channels in [1, 2] {
            for partition in [&[65_536][..], &[1, 17, 3][..]] {
                let (sink, summary) = stream(
                    RawS16Format::new(48000, channels).unwrap(),
                    RawS16Output::F32,
                    &bytes,
                    partition,
                );
                assert_eq!(sink.bytes, expected);
                assert_eq!(summary.output.format, SampleFormat::F32);
                assert_eq!(summary.output.sample_rate, 48000);
                assert_eq!(summary.input_frames, 65_536 / u64::from(channels));
                assert_eq!(summary.output_frames, summary.input_frames);
                assert!(sink.largest <= 65_536);
            }
        }
    }

    #[test]
    fn empty_sessions_and_empty_writes_have_no_output() {
        for output in [
            RawS16Output::S16 { sample_rate: None },
            RawS16Output::F32,
            RawS16Output::S16 {
                sample_rate: Some(16000),
            },
        ] {
            let mut sink = Keep::default();
            let limits = Limits {
                max_output_samples: 0,
                ..Limits::default()
            };
            let mut stream = RawS16Stream::new(
                RawS16Format::new(48000, 2).unwrap(),
                output,
                &limits,
                &mut sink,
            )
            .unwrap();
            stream.write(&[]).unwrap();
            stream.write(&[]).unwrap();
            let summary = stream.finish().unwrap();
            assert_eq!(summary.input_frames, 0);
            assert_eq!(summary.output_frames, 0);
            assert_eq!(sink.starts, 1);
            assert_eq!(sink.writes, 0);
        }
    }

    #[test]
    fn identity_limits_are_cumulative_and_packets_stay_frame_aligned() {
        for output in [RawS16Output::S16 { sample_rate: None }, RawS16Output::F32] {
            let limits = Limits {
                max_packet_bytes: 17,
                max_output_samples: 12,
                ..Limits::default()
            };
            let mut sink = Keep::default();
            let format = RawS16Format::new(48000, 2).unwrap();
            let mut stream = RawS16Stream::new(format, output, &limits, &mut sink).unwrap();
            stream.write(&signal(3, 2)).unwrap();
            stream.write(&signal(3, 2)).unwrap();
            let summary = stream.finish().unwrap();
            assert_eq!(summary.input_frames, 6);
            assert_eq!(summary.output_frames, 6);
            assert!(sink.largest <= 17);
            assert_eq!(sink.bytes.len(), 6 * 2 * summary.output.format.bytes());

            let mut sink = Keep::default();
            let mut stream = RawS16Stream::new(format, output, &limits, &mut sink).unwrap();
            stream.write(&signal(3, 2)).unwrap();
            assert!(matches!(
                stream.write(&signal(4, 2)),
                Err(Error::LimitExceeded(LimitKind::OutputSamples))
            ));
            assert!(stream.finish().is_err());
        }
    }

    #[test]
    fn rates_match_sized_output_across_startup_tail_and_partitions() {
        for (input_rate, output_rate) in [
            (48000, 16000),
            (44100, 16000),
            (8000, 48000),
            (48000, 44100),
        ] {
            let rate = Rate::new(input_rate, output_rate).unwrap();
            let length = rate.length;
            let boundary = length * 2 / 3;
            for channels in [1, 2] {
                let format = RawS16Format::new(input_rate, channels).unwrap();
                for frames in [
                    0,
                    1,
                    2,
                    boundary - 1,
                    boundary,
                    boundary + 1,
                    length - 1,
                    length,
                    length + 1,
                    length * 2 + 3,
                    1009,
                ] {
                    let bytes = signal(frames, channels);
                    let mut expected = Keep::default();
                    let summary = RawS16Plan::new(
                        format,
                        Some(output_rate),
                        bytes.len() as u64,
                        &Limits::default(),
                    )
                    .unwrap()
                    .run(SliceSource::new(&bytes), &mut expected)
                    .unwrap();
                    for partition in [&[1][..], &[7, 2, 83, 1, 5][..], &[100_000][..]] {
                        let (sink, actual) = stream(
                            format,
                            RawS16Output::S16 {
                                sample_rate: Some(output_rate),
                            },
                            &bytes,
                            partition,
                        );
                        assert_eq!(
                            actual, summary,
                            "{input_rate}->{output_rate}, {channels}, {frames}"
                        );
                        assert_eq!(
                            sink.bytes, expected.bytes,
                            "{input_rate}->{output_rate}, {channels}, {frames}, {partition:?}"
                        );
                    }
                }
            }
        }
    }

    fn float_reference(
        input_rate: u32,
        output_rate: u32,
        bytes: &[u8],
        format: SampleFormat,
    ) -> Vec<u8> {
        let input = Input::F32(RawF32Format::new(input_rate, 1).unwrap());
        let rate = Rate::new(input_rate, output_rate).unwrap();
        let frames = (bytes.len() / 4) as u64;
        let mut position = 0;
        let mut phase = 0;
        let mut pcm = Pcm::new();
        let mut output = Vec::new();
        for _ in 0..rate.output_frames(frames).unwrap() {
            render(
                &rate,
                position,
                phase,
                frames,
                pcm.float_buffer(1, 1),
                |at, _| input.sample(bytes, at as usize * 4),
            )
            .unwrap();
            let at = output.len();
            crate::convert::interleave(&pcm, 0, 1, format, &mut output, at);
            phase += rate.step;
            position += phase / rate.phases;
            phase %= rate.phases;
        }
        output
    }

    #[test]
    fn float_rate_preserves_unquantized_samples_across_all_partitions() {
        for input_rate in [44100, 48000] {
            let length = Rate::new(input_rate, 16000).unwrap().length;
            for frames in [
                0,
                1,
                length * 2 / 3,
                length * 2 / 3 + 1,
                length - 1,
                length,
                length + 1,
                length * 3 + 7,
                1009,
            ] {
                let bytes: Vec<u8> = (0..frames)
                    .flat_map(|n| {
                        let value = match n % 7 {
                            0 => 1.5,
                            1 => -1.75,
                            2 => 0.000_001_25,
                            3 => -0.0,
                            _ => ((n as u32).wrapping_mul(73_973) % 65536) as f32 / 197_331.0 - 0.2,
                        };
                        value.to_le_bytes()
                    })
                    .collect();
                for output in [
                    RawF32Output::F32 {
                        sample_rate: Some(16000),
                    },
                    RawF32Output::S16 {
                        sample_rate: Some(16000),
                    },
                ] {
                    let format = match output {
                        RawF32Output::F32 { .. } => SampleFormat::F32,
                        _ => SampleFormat::S16,
                    };
                    let expected = float_reference(input_rate, 16000, &bytes, format);
                    for partition in [&[1][..], &[2, 17, 1, 71][..], &[100_000][..]] {
                        let mut sink = Keep::default();
                        let input = RawF32Format::new(input_rate, 1).unwrap();
                        assert_eq!(input.rate(), input_rate);
                        assert_eq!(input.channels(), 1);
                        let mut stream =
                            RawF32Stream::new(input, output, &Limits::default(), &mut sink)
                                .unwrap();
                        let mut at = 0;
                        for count in partition.iter().cycle() {
                            if at == bytes.len() {
                                break;
                            }
                            let end = bytes.len().min(at + count * 4);
                            stream.write(&bytes[at..end]).unwrap();
                            at = end;
                        }
                        let summary = stream.finish().unwrap();
                        assert_eq!(summary.input_frames, frames as u64);
                        assert_eq!(
                            summary.output_frames as usize * format.bytes(),
                            expected.len()
                        );
                        assert_eq!(
                            sink.bytes, expected,
                            "{input_rate}, {frames}, {format:?}, {partition:?}"
                        );
                    }
                }
            }
        }

        let bytes = [0.000_001_25f32.to_le_bytes(); 100].concat();
        let mut sink = Keep::default();
        let mut stream = RawF32Stream::new(
            RawF32Format::new(48000, 1).unwrap(),
            RawF32Output::F32 {
                sample_rate: Some(16000),
            },
            &Limits::default(),
            &mut sink,
        )
        .unwrap();
        stream.write(&bytes).unwrap();
        stream.finish().unwrap();
        assert!(!sink.bytes.is_empty());
        assert!(
            sink.bytes
                .chunks_exact(4)
                .all(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()) > 0.0)
        );
    }

    #[test]
    fn float_rate_s16_matches_the_sized_integer_path_for_identical_samples() {
        for input_rate in [44100, 48000] {
            let bytes = signal(1009, 1);
            let float: Vec<u8> = bytes
                .chunks_exact(2)
                .flat_map(|bytes| {
                    (f32::from(i16::from_le_bytes(bytes.try_into().unwrap())) / 32_768.0)
                        .to_le_bytes()
                })
                .collect();
            let mut expected = Keep::default();
            RawS16Plan::new(
                RawS16Format::new(input_rate, 1).unwrap(),
                Some(16000),
                bytes.len() as u64,
                &Limits::default(),
            )
            .unwrap()
            .run(SliceSource::new(&bytes), &mut expected)
            .unwrap();
            let mut sink = Keep::default();
            let mut stream = RawF32Stream::new(
                RawF32Format::new(input_rate, 1).unwrap(),
                RawF32Output::S16 {
                    sample_rate: Some(16000),
                },
                &Limits::default(),
                &mut sink,
            )
            .unwrap();
            for chunk in float.chunks(28) {
                stream.write(chunk).unwrap();
            }
            stream.finish().unwrap();
            assert_eq!(sink.bytes, expected.bytes);
        }
    }

    #[test]
    fn float_identity_and_final_quantization_preserve_their_boundaries() {
        let values = [
            0.0f32,
            -0.0,
            f32::from_bits(1),
            -f32::from_bits(1),
            0.000_001_25,
            -1.75,
            1.5,
            0.25,
            -0.25,
        ];
        let bytes: Vec<u8> = values.into_iter().flat_map(f32::to_le_bytes).collect();
        for rate in [None, Some(48000)] {
            let mut sink = Keep::default();
            let limits = Limits {
                max_packet_bytes: 13,
                ..Limits::default()
            };
            let mut stream = RawF32Stream::new(
                RawF32Format::new(48000, 1).unwrap(),
                RawF32Output::F32 { sample_rate: rate },
                &limits,
                &mut sink,
            )
            .unwrap();
            stream.write(&bytes).unwrap();
            let summary = stream.finish().unwrap();
            assert_eq!(sink.bytes, bytes);
            assert_eq!(summary.input_frames, values.len() as u64);
            assert_eq!(summary.output_frames, summary.input_frames);
            assert!(sink.largest <= 13);
        }
        let mut sink = Keep::default();
        let mut stream = RawF32Stream::new(
            RawF32Format::new(48000, 1).unwrap(),
            RawF32Output::S16 { sample_rate: None },
            &Limits::default(),
            &mut sink,
        )
        .unwrap();
        stream.write(&bytes).unwrap();
        stream.finish().unwrap();
        let expected: Vec<u8> = [0i16, 0, 0, 0, 0, i16::MIN, i16::MAX, 8192, -8192]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        assert_eq!(sink.bytes, expected);
    }

    #[test]
    fn float_invalid_frames_and_nonfinite_samples_poison_without_drain() {
        assert!(RawF32Format::new(48000, 2).is_err());
        assert!(RawF32Format::new(0, 1).is_err());
        let input = RawF32Format::new(48000, 1).unwrap();
        for output in [
            RawF32Output::F32 { sample_rate: None },
            RawF32Output::S16 { sample_rate: None },
            RawF32Output::F32 {
                sample_rate: Some(16000),
            },
            RawF32Output::S16 {
                sample_rate: Some(16000),
            },
        ] {
            for bad in [
                vec![0; 3],
                f32::NAN.to_le_bytes().to_vec(),
                f32::INFINITY.to_le_bytes().to_vec(),
                f32::NEG_INFINITY.to_le_bytes().to_vec(),
            ] {
                let mut sink = Keep::default();
                let mut stream =
                    RawF32Stream::new(input, output, &Limits::default(), &mut sink).unwrap();
                assert!(matches!(stream.write(&bad), Err(Error::Invalid(_))));
                assert!(stream.write(&[0; 4]).is_err());
                assert!(stream.finish().is_err());
                assert_eq!(sink.writes, 0);
            }
        }
    }

    #[test]
    fn float_storage_and_writes_remain_bounded_on_large_input() {
        let bytes: Vec<u8> = (0..20_003)
            .flat_map(|n| ((n % 1009) as f32 / 1973.0 - 0.25).to_le_bytes())
            .collect();
        let limits = Limits {
            max_packet_bytes: 515,
            ..Limits::default()
        };
        let expected = float_reference(48000, 16000, &bytes, SampleFormat::F32);
        let mut sink = Keep::default();
        let mut stream = RawF32Stream::new(
            RawF32Format::new(48000, 1).unwrap(),
            RawF32Output::F32 {
                sample_rate: Some(16000),
            },
            &limits,
            &mut sink,
        )
        .unwrap();
        let ring_capacity = match &stream.0.mode {
            Mode::Rate { rate, ring, .. } => {
                assert_eq!(ring.len(), (rate.length + 1) * 4);
                ring.capacity()
            }
            _ => panic!("missing rate conversion"),
        };
        let output_capacity = stream.0.bytes.capacity();
        stream.write(&bytes).unwrap();
        assert_eq!(stream.0.bytes.capacity(), output_capacity);
        if let Mode::Rate { ring, .. } = &stream.0.mode {
            assert_eq!(ring.capacity(), ring_capacity);
        }
        stream.finish().unwrap();
        assert_eq!(sink.bytes, expected);
        assert!(sink.largest <= 515);
    }

    #[test]
    fn float_limits_and_sink_failures_end_the_session() {
        let input = RawF32Format::new(48000, 1).unwrap();
        for (output, cap) in [
            (RawF32Output::S16 { sample_rate: None }, 3),
            (
                RawF32Output::F32 {
                    sample_rate: Some(16000),
                },
                400,
            ),
        ] {
            let mut sink = Keep::default();
            let limits = Limits {
                max_packet_bytes: cap,
                ..Limits::default()
            };
            assert!(matches!(
                RawF32Stream::new(input, output, &limits, &mut sink),
                Err(Error::LimitExceeded(LimitKind::PacketBytes))
            ));
            assert_eq!(sink.starts, 0);
        }
        for output in [
            RawF32Output::F32 { sample_rate: None },
            RawF32Output::S16 { sample_rate: None },
            RawF32Output::F32 {
                sample_rate: Some(16000),
            },
        ] {
            let mut sink = Keep::default();
            let limits = Limits {
                max_output_samples: 2,
                ..Limits::default()
            };
            let mut stream = RawF32Stream::new(input, output, &limits, &mut sink).unwrap();
            stream.write(&0.25f32.to_le_bytes()).unwrap();
            assert!(matches!(
                stream.write(&[0; 8]),
                Err(Error::LimitExceeded(LimitKind::OutputSamples))
            ));
            assert!(stream.finish().is_err());

            let mut sink = Keep {
                fail_write: true,
                ..Keep::default()
            };
            let mut stream =
                RawF32Stream::new(input, output, &Limits::default(), &mut sink).unwrap();
            assert!(matches!(stream.write(&[0; 300_000]), Err(Error::Output(_))));
            assert!(stream.write(&[]).is_err());
            assert!(stream.finish().is_err());
            assert_eq!(sink.writes, 1);
        }
    }

    #[test]
    fn long_writes_keep_only_the_filter_ring_and_bounded_output() {
        let format = RawS16Format::new(44100, 2).unwrap();
        let bytes = signal(50_003, 2);
        let limits = Limits {
            max_packet_bytes: 1025,
            ..Limits::default()
        };
        let mut expected = Keep::default();
        RawS16Plan::new(format, Some(16000), bytes.len() as u64, &limits)
            .unwrap()
            .run(SliceSource::new(&bytes), &mut expected)
            .unwrap();
        let mut sink = Keep::default();
        let mut stream = RawS16Stream::new(
            format,
            RawS16Output::S16 {
                sample_rate: Some(16000),
            },
            &limits,
            &mut sink,
        )
        .unwrap();
        let ring_capacity = match &stream.0.mode {
            Mode::Rate { ring, rate, .. } => {
                assert_eq!(ring.len(), (rate.length + 1) * 4);
                ring.capacity()
            }
            _ => panic!("missing rate conversion"),
        };
        let output_capacity = stream.0.bytes.capacity();
        for chunk in bytes.chunks(100_000) {
            stream.write(chunk).unwrap();
            assert_eq!(stream.0.bytes.capacity(), output_capacity);
            if let Mode::Rate { ring, .. } = &stream.0.mode {
                assert_eq!(ring.capacity(), ring_capacity);
            }
        }
        stream.finish().unwrap();
        assert_eq!(sink.bytes, expected.bytes);
        assert!(sink.largest <= 1025);
    }

    #[test]
    fn invalid_configuration_never_starts_the_sink() {
        let format = RawS16Format::new(48000, 2).unwrap();
        for (limits, output) in [
            (
                Limits {
                    max_channels: 1,
                    ..Limits::default()
                },
                RawS16Output::F32,
            ),
            (
                Limits {
                    max_packet_bytes: 7,
                    ..Limits::default()
                },
                RawS16Output::F32,
            ),
            (
                Limits {
                    max_packet_bytes: 3,
                    ..Limits::default()
                },
                RawS16Output::S16 { sample_rate: None },
            ),
            (
                Limits {
                    max_packet_bytes: 400,
                    ..Limits::default()
                },
                RawS16Output::S16 {
                    sample_rate: Some(16000),
                },
            ),
            (
                Limits::default(),
                RawS16Output::S16 {
                    sample_rate: Some(0),
                },
            ),
            (
                Limits::default(),
                RawS16Output::S16 {
                    sample_rate: Some(44099),
                },
            ),
        ] {
            let mut sink = Keep::default();
            assert!(RawS16Stream::new(format, output, &limits, &mut sink).is_err());
            assert_eq!(sink.starts, 0);
            assert_eq!(sink.writes, 0);
        }
    }

    #[test]
    fn incomplete_frames_and_input_sample_limits_poison_the_stream() {
        for output in [
            RawS16Output::F32,
            RawS16Output::S16 { sample_rate: None },
            RawS16Output::S16 {
                sample_rate: Some(16000),
            },
        ] {
            for bad in [&[0, 1, 2][..], &[0; 44][..]] {
                let mut sink = Keep::default();
                let limits = Limits {
                    max_output_samples: 20,
                    ..Limits::default()
                };
                let mut stream = RawS16Stream::new(
                    RawS16Format::new(48000, 2).unwrap(),
                    output,
                    &limits,
                    &mut sink,
                )
                .unwrap();
                assert!(stream.write(bad).is_err());
                assert!(stream.write(&[]).is_err());
                assert!(stream.write(&[0; 4]).is_err());
                assert!(stream.finish().is_err());
                assert_eq!(sink.writes, 0);
            }
        }
    }

    #[test]
    fn output_samples_are_bounded_during_write_and_before_the_final_drain() {
        for (frames, limit, fail_write) in [(100, 100, true), (34, 150, false)] {
            let mut sink = Keep::default();
            let limits = Limits {
                max_output_samples: limit,
                ..Limits::default()
            };
            let mut stream = RawS16Stream::new(
                RawS16Format::new(8000, 1).unwrap(),
                RawS16Output::S16 {
                    sample_rate: Some(48000),
                },
                &limits,
                &mut sink,
            )
            .unwrap();
            let result = stream.write(&signal(frames, 1));
            if fail_write {
                assert!(matches!(
                    result,
                    Err(Error::LimitExceeded(LimitKind::OutputSamples))
                ));
                assert!(stream.write(&[]).is_err());
            } else {
                result.unwrap();
            }
            assert!(matches!(
                stream.finish(),
                Err(Error::LimitExceeded(LimitKind::OutputSamples)) | Err(Error::Invalid(_))
            ));
            assert_eq!(sink.writes, 0);
        }

        let rate = Rate::new(8000, 48000).unwrap();
        let expected_frames = rate.output_frames(34).unwrap();
        let limits = Limits {
            max_output_samples: expected_frames,
            ..Limits::default()
        };
        let mut sink = Keep::default();
        let mut stream = RawS16Stream::new(
            RawS16Format::new(8000, 1).unwrap(),
            RawS16Output::S16 {
                sample_rate: Some(48000),
            },
            &limits,
            &mut sink,
        )
        .unwrap();
        stream.write(&signal(34, 1)).unwrap();
        let summary = stream.finish().unwrap();
        assert_eq!(summary.output_frames, expected_frames);
        assert_eq!(sink.bytes.len() as u64, expected_frames * 2);
    }

    #[test]
    fn downstream_failures_are_terminal_including_finish() {
        let format = RawS16Format::new(8000, 1).unwrap();
        let mut sink = Keep {
            fail_start: true,
            ..Keep::default()
        };
        assert!(matches!(
            RawS16Stream::new(format, RawS16Output::F32, &Limits::default(), &mut sink),
            Err(Error::Output(_))
        ));
        assert_eq!(sink.starts, 1);
        assert_eq!(sink.writes, 0);
        for output in [
            RawS16Output::S16 { sample_rate: None },
            RawS16Output::F32,
            RawS16Output::S16 {
                sample_rate: Some(48000),
            },
        ] {
            let mut sink = Keep {
                fail_write: true,
                ..Keep::default()
            };
            let limits = Limits {
                max_packet_bytes: 128,
                ..Limits::default()
            };
            let mut stream = RawS16Stream::new(format, output, &limits, &mut sink).unwrap();
            assert!(matches!(
                stream.write(&signal(200, 1)),
                Err(Error::Output(_))
            ));
            assert!(stream.write(&signal(200, 1)).is_err());
            assert!(stream.finish().is_err());
            assert_eq!(sink.writes, 1);
        }
        for output in [
            RawS16Output::F32,
            RawS16Output::S16 {
                sample_rate: Some(48000),
            },
        ] {
            let mut sink = Keep {
                fail_write: true,
                ..Keep::default()
            };
            let mut stream =
                RawS16Stream::new(format, output, &Limits::default(), &mut sink).unwrap();
            stream.write(&signal(34, 1)).unwrap();
            assert!(matches!(stream.finish(), Err(Error::Output(_))));
            assert_eq!(sink.writes, 1);
        }
    }
}
