//! The output's sample rate and channels when they differ from the
//! stream's (`docs/design/audio-decode.md`, "Resampling and downmixing"):
//! a [`Sink`] between the stream and the caller's sink that downmixes,
//! then resamples, then converts to the asked sample format.
//!
//! The stream writes 32-bit floating point to it, already trimmed (the
//! timeline is the decoded one), so the conversion is the same on any
//! number of threads.

use crate::convert::interleave;
use crate::decode::{OutputFormat, SampleFormat, SilenceSpan, Sink, Summary};
use crate::mix::{Matrix, deinterleave};
use crate::resample::{Resampler, check_rates};
use crate::{Error, LimitKind, Limits, Pcm};

/// The lowest and highest output sample rates.
pub(crate) const RATES: core::ops::RangeInclusive<u32> = 1_000..=768_000;
/// The most output samples per channel written at once: upsampling makes
/// a packet's samples many times more, and memory stays bounded by this
/// rather than by the packet times the ratio.
const PIECE: usize = 1 << 16;

/// What the stream's output becomes.
pub(crate) struct Plan {
    output: OutputFormat,
    matrix: Option<Matrix>,
    from: u32,
    max_output_samples: u64,
}

impl Plan {
    /// The conversion from the stream's `stream` output (its rate,
    /// channels and speaker positions) to the rate and channels asked, or
    /// `None` when both are the stream's.
    pub(crate) fn new(
        stream: &OutputFormat,
        sample_rate: Option<u32>,
        channels: Option<u16>,
        format: SampleFormat,
        limits: &Limits,
    ) -> Result<Option<Self>, Error> {
        let rate = sample_rate.unwrap_or(stream.sample_rate);
        let target = channels.unwrap_or(stream.channels);
        if rate == stream.sample_rate && target == stream.channels {
            return Ok(None);
        }
        // Only a rate that is resampled to: a downmix keeps the stream's.
        if rate != stream.sample_rate && !RATES.contains(&rate) {
            return Err(Error::Unsupported(
                "an output sample rate outside 1,000 to 768,000 Hz",
            ));
        }
        let matrix = (target != stream.channels)
            .then(|| Matrix::new(stream.channels, stream.channel_mask, target))
            .transpose()?;
        let output = OutputFormat {
            sample_rate: rate,
            channels: target,
            channel_mask: matrix.as_ref().map_or(stream.channel_mask, |m| m.mask),
            format,
        };
        // Checked before the first packet: a rate the resampler refuses.
        if rate != stream.sample_rate {
            check_rates(stream.sample_rate, rate)?;
        }
        Ok(Some(Self {
            output,
            matrix,
            from: stream.sample_rate,
            max_output_samples: limits.max_output_samples,
        }))
    }
}

/// The conversion as a sink in front of the caller's.
pub(crate) struct Converter<'a> {
    writer: Writer<'a>,
    matrix: Option<Matrix>,
    from: u32,
    /// The stream's channels.
    channels: usize,
    resampler: Option<Resampler>,
    /// The downmixed samples, per channel, when not resampled.
    planes: Vec<Vec<f32>>,
}

/// The output side: the asked format, the limit, the caller's sink.
struct Writer<'a> {
    output: OutputFormat,
    max_output_samples: u64,
    sink: &'a mut dyn Sink,
    pcm: Pcm,
    bytes: Vec<u8>,
    written: u64,
}

impl Writer<'_> {
    /// Writes `count` samples per channel, which `fill` writes planar.
    fn emit(&mut self, count: usize, fill: impl FnOnce(&mut [f32])) -> Result<(), Error> {
        if count == 0 {
            return Ok(());
        }
        let channels = usize::from(self.output.channels);
        let written = self.written.saturating_add(count as u64);
        if written.saturating_mul(channels as u64) > self.max_output_samples {
            return Err(Error::LimitExceeded(LimitKind::OutputSamples));
        }
        fill(self.pcm.float_buffer(channels, count));
        interleave(&self.pcm, 0, count, self.output.format, &mut self.bytes, 0);
        let len = count
            .saturating_mul(channels)
            .saturating_mul(self.output.format.bytes());
        self.sink.write(self.bytes.get(..len).unwrap_or_default())?;
        self.written = written;
        Ok(())
    }
}

impl Writer<'_> {
    /// The resampler's next `count` samples per channel, [`PIECE`] at a
    /// time (the resampler carries its position from one to the next, so
    /// the samples are the same as in one piece).
    fn resampled(&mut self, resampler: &mut Resampler, mut count: usize) -> Result<(), Error> {
        // The limit checked for all of them before the first piece is
        // written, as for a packet's samples written at once.
        let channels = u64::from(self.output.channels);
        let written = self.written.saturating_add(count as u64);
        if written.saturating_mul(channels) > self.max_output_samples {
            return Err(Error::LimitExceeded(LimitKind::OutputSamples));
        }
        while count > 0 {
            let n = count.min(PIECE);
            self.emit(n, |out| resampler.produce(n, out))?;
            count -= n;
        }
        Ok(())
    }
}

impl<'a> Converter<'a> {
    pub(crate) fn new(plan: Plan, sink: &'a mut dyn Sink) -> Self {
        Self {
            writer: Writer {
                output: plan.output,
                max_output_samples: plan.max_output_samples,
                sink,
                pcm: Pcm::new(),
                bytes: Vec::new(),
                written: 0,
            },
            matrix: plan.matrix,
            from: plan.from,
            channels: 0,
            resampler: None,
            planes: Vec::new(),
        }
    }

    /// The end of the stream: the resampler's last samples, then the
    /// summary in the output's terms.
    pub(crate) fn finish(mut self, mut summary: Summary) -> Result<Summary, Error> {
        if let Some(resampler) = self.resampler.as_mut() {
            let count = resampler.finish();
            self.writer.resampled(resampler, count)?;
        }
        // Positions in the stream's samples become the output's: the
        // first output sample at or after each.
        let resampler = self.resampler.as_ref();
        let at = |samples: u64| resampler.map_or(samples, |r| r.output_position(samples));
        summary.timeline.start = at(summary.timeline.start);
        for span in &mut summary.timeline.silence {
            let start = at(span.at);
            let end = at(span.at.saturating_add(span.samples));
            *span = SilenceSpan {
                at: start,
                samples: end.saturating_sub(start),
            };
        }
        summary.output = self.writer.output;
        summary.samples = self.writer.written;
        Ok(summary)
    }
}

impl Sink for Converter<'_> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        self.channels = usize::from(format.channels);
        let output = self.writer.output;
        let outputs = usize::from(output.channels);
        if format.sample_rate != output.sample_rate {
            self.resampler = Some(Resampler::new(self.from, output.sample_rate, outputs)?);
        }
        self.planes = (0..outputs).map(|_| Vec::new()).collect();
        self.writer.sink.start(&output)
    }

    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        // Downmixed, or copied, straight into the resampler's input.
        let planes = match self.resampler.as_mut() {
            Some(resampler) => resampler.inputs_mut(),
            None => &mut self.planes,
        };
        match &self.matrix {
            Some(matrix) => matrix.mix(samples, planes),
            None => {
                for (c, plane) in planes.iter_mut().enumerate() {
                    deinterleave(samples, self.channels, c, plane);
                }
            }
        }
        match self.resampler.as_mut() {
            Some(resampler) => {
                let count = resampler.available();
                self.writer.resampled(resampler, count)
            }
            None => {
                let count = self.planes.first().map_or(0, Vec::len);
                let planes = &mut self.planes;
                let result = self.writer.emit(count, |out| {
                    for (plane, out) in planes.iter().zip(out.chunks_exact_mut(count.max(1))) {
                        // Every plane holds `count` samples (each frame
                        // gave one to each); zipped, a mismatch cannot
                        // panic.
                        for (o, &v) in out.iter_mut().zip(plane) {
                            *o = v;
                        }
                    }
                });
                planes.iter_mut().for_each(Vec::clear);
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(sample_rate: u32) -> OutputFormat {
        OutputFormat {
            sample_rate,
            channels: 2,
            channel_mask: 0x3,
            format: SampleFormat::S16,
        }
    }

    /// The range of output rates applies to a rate resampled to; a downmix
    /// alone keeps any rate the stream has.
    #[test]
    fn a_downmix_keeps_any_stream_rate() {
        let limits = Limits::default();
        for rate in [800, 1_000_000] {
            let plan = Plan::new(&stream(rate), None, Some(1), SampleFormat::S16, &limits);
            assert!(matches!(plan, Ok(Some(_))), "{rate}");
            let plan = Plan::new(
                &stream(rate),
                Some(rate),
                Some(1),
                SampleFormat::S16,
                &limits,
            );
            assert!(matches!(plan, Ok(Some(_))), "{rate}");
        }
        for rate in [999, 768_001] {
            let plan = Plan::new(
                &stream(48_000),
                Some(rate),
                None,
                SampleFormat::S16,
                &limits,
            );
            assert!(matches!(plan, Err(Error::Unsupported(_))), "{rate}");
        }
        let plan = Plan::new(
            &stream(48_000),
            Some(16_000),
            Some(1),
            SampleFormat::S16,
            &limits,
        );
        assert!(matches!(plan, Ok(Some(_))));
    }
}
