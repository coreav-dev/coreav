use super::{FlacDecoder, StreamInfo, validate_stream};
use crate::{Error, LimitKind, Limits, OutputFormat, Pcm, SampleFormat, Sink};

/// A completely validated immutable payload with bounded emission scratch.
pub struct PreparedStream<'a> {
    data: &'a [u8],
    decoder: FlacDecoder,
    pcm: Pcm,
    packed: Vec<u8>,
    format: OutputFormat,
}

/// Validates known-length mono/stereo 16/24-bit FLAC before any sink calls.
/// Both decode walks cost 128 work units per channel-sample, and encoded
/// payload memory must already be deducted from the retained-byte allowance.
pub fn prepare_stream<'a>(
    info: StreamInfo,
    data: &'a [u8],
    format: SampleFormat,
    limits: &Limits,
) -> Result<PreparedStream<'a>, Error> {
    if !matches!(info.channels, 1 | 2)
        || !matches!(info.bits_per_sample, 16 | 24)
        || !matches!(format, SampleFormat::S16 | SampleFormat::F32)
    {
        return Err(Error::Unsupported("standalone FLAC PCM representation"));
    }
    let samples = info
        .total_samples
        .checked_mul(u64::from(info.channels))
        .ok_or(Error::LimitExceeded(LimitKind::OutputSamples))?;
    if samples > limits.max_output_samples || samples > limits.isobmff.max_sample_work / 128 {
        return Err(Error::LimitExceeded(LimitKind::OutputSamples));
    }
    let block = usize::from(info.max_block_size);
    let scalars = block * usize::from(info.channels);
    let memory_failure = || {
        Error::Isobmff(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::RetainedBytes,
            offset: 0,
        })
    };
    let length = scalars * format.bytes();
    if (length + scalars * 4 + block * 8 + 1024) as u64 > limits.isobmff.max_retained_bytes {
        return Err(memory_failure());
    }
    let mut packed = Vec::new();
    packed
        .try_reserve_exact(length)
        .map_err(|_| memory_failure())?;
    let mut scratch_limits = *limits;
    scratch_limits.isobmff.max_retained_bytes = limits
        .isobmff
        .max_retained_bytes
        .checked_sub(packed.capacity() as u64)
        .ok_or_else(memory_failure)?;
    validate_stream(info, data, &scratch_limits)?;
    if (scalars * 4 + 1024) as u64 > scratch_limits.isobmff.max_retained_bytes {
        return Err(memory_failure());
    }
    let mut pcm = Pcm::new();
    pcm.try_int_buffer(
        usize::from(info.channels),
        block,
        u32::from(info.bits_per_sample),
    )?;
    if pcm.retained_bytes().saturating_add(1024) > scratch_limits.isobmff.max_retained_bytes {
        return Err(memory_failure());
    }
    Ok(PreparedStream {
        data,
        decoder: FlacDecoder::new(info, limits)?,
        pcm,
        packed,
        format: OutputFormat {
            sample_rate: info.sample_rate,
            channels: u16::from(info.channels),
            channel_mask: crate::convert::default_mask(u16::from(info.channels)),
            format,
        },
    })
}

impl PreparedStream<'_> {
    /// Emits the same validated bytes once, without rereading the source.
    pub fn write(mut self, sink: &mut dyn Sink) -> Result<(), Error> {
        sink.start(&self.format)?;
        while !self.data.is_empty() {
            let frame = self.decoder.decode(self.data, &mut self.pcm)?;
            crate::convert::interleave(
                &self.pcm,
                0,
                self.pcm.frames(),
                self.format.format,
                &mut self.packed,
                0,
            );
            self.packed
                .truncate(self.pcm.frames() * self.pcm.channels() * self.format.format.bytes());
            sink.write(&self.packed)?;
            self.data = self.data.get(frame.bytes..).ok_or(Error::Truncated)?;
        }
        Ok(())
    }
}
