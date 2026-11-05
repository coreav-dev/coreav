//! Decoded samples.

/// The decoded samples of one packet: `frames` samples for each of
/// `channels` channels, stored planar (all of channel 0, then all of
/// channel 1, ...), either as integers of a stated bit depth (lossless
/// codecs) or as floating point with full scale at ±1.0 (lossy codecs).
///
/// A decoder reuses the buffer from packet to packet, so decoding a stream
/// allocates once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pcm {
    channels: usize,
    frames: usize,
    data: Data,
}

#[derive(Clone, Debug, PartialEq)]
enum Data {
    /// Integers of `bits` significant bits, sign-extended.
    Int { bits: u32, samples: Vec<i32> },
    /// Floating point, full scale ±1.0.
    Float(Vec<f32>),
}

impl Default for Data {
    fn default() -> Self {
        Self::Int {
            bits: 16,
            samples: Vec::new(),
        }
    }
}

/// One channel's samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plane<'a> {
    /// Integers of the given bit depth.
    Int {
        /// Significant bits, 1 to 32.
        bits: u32,
        /// The samples.
        samples: &'a [i32],
    },
    /// Floating point, full scale ±1.0.
    Float(&'a [f32]),
}

impl Pcm {
    /// An empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Samples per channel.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Whether the samples are floating point.
    pub fn is_float(&self) -> bool {
        matches!(self.data, Data::Float(_))
    }

    /// The bit depth of integer samples, `None` for floating point.
    pub fn bits(&self) -> Option<u32> {
        match self.data {
            Data::Int { bits, .. } => Some(bits),
            Data::Float(_) => None,
        }
    }

    /// The samples of channel `channel`, `None` past the last channel.
    pub fn plane(&self, channel: usize) -> Option<Plane<'_>> {
        if channel >= self.channels {
            return None;
        }
        let start = channel.checked_mul(self.frames)?;
        let end = start.checked_add(self.frames)?;
        match &self.data {
            Data::Int { bits, samples } => Some(Plane::Int {
                bits: *bits,
                samples: samples.get(start..end)?,
            }),
            Data::Float(samples) => Some(Plane::Float(samples.get(start..end)?)),
        }
    }

    /// Empties the buffer, keeping its memory.
    pub fn clear(&mut self) {
        self.frames = 0;
        match &mut self.data {
            Data::Int { samples, .. } => samples.clear(),
            Data::Float(samples) => samples.clear(),
        }
    }

    pub(crate) fn retained_bytes(&self) -> u64 {
        let bytes = match &self.data {
            Data::Int { samples, .. } => samples.capacity().saturating_mul(size_of::<i32>()),
            Data::Float(samples) => samples.capacity().saturating_mul(size_of::<f32>()),
        };
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }

    /// Makes the buffer hold `channels × frames` integer samples of `bits`
    /// bits and returns them, planar. Their values are left over from
    /// earlier packets: the decoder writes every one.
    pub(crate) fn try_int_buffer(
        &mut self,
        channels: usize,
        frames: usize,
        bits: u32,
    ) -> Result<&mut [i32], crate::Error> {
        let failure = || crate::Error::LimitExceeded(crate::LimitKind::OutputSamples);
        let length = channels.checked_mul(frames).ok_or_else(failure)?;
        if matches!(self.data, Data::Float(_)) {
            let mut samples = Vec::new();
            samples.try_reserve_exact(length).map_err(|_| failure())?;
            self.data = Data::Int { bits, samples };
        }
        match &mut self.data {
            Data::Int {
                bits: depth,
                samples,
            } => {
                samples
                    .try_reserve_exact(length.saturating_sub(samples.len()))
                    .map_err(|_| failure())?;
                samples.resize(length, 0);
                *depth = bits;
                self.channels = channels;
                self.frames = frames;
                Ok(samples)
            }
            Data::Float(_) => Err(crate::Error::Invalid("integer PCM buffer")),
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    pub(crate) fn int_buffer(&mut self, channels: usize, frames: usize, bits: u32) -> &mut [i32] {
        self.try_int_buffer(channels, frames, bits).unwrap()
    }

    /// Makes the buffer hold `channels × frames` floating-point samples and
    /// returns them, planar. Their values are left over from earlier
    /// packets: the decoder writes every one.
    /// Takes `samples` as `channels` planes of `frames` samples each, and
    /// hands back the buffer it held in their place, for reuse.
    pub(crate) fn swap_float(&mut self, channels: usize, frames: usize, samples: &mut Vec<f32>) {
        self.channels = channels;
        self.frames = frames;
        let mut held = match core::mem::take(&mut self.data) {
            Data::Float(held) => held,
            Data::Int { .. } => Vec::new(),
        };
        core::mem::swap(&mut held, samples);
        self.data = Data::Float(held);
    }

    pub(crate) fn float_buffer(&mut self, channels: usize, frames: usize) -> &mut [f32] {
        self.channels = channels;
        self.frames = frames;
        let mut samples = match core::mem::take(&mut self.data) {
            Data::Float(samples) => samples,
            Data::Int { .. } => Vec::new(),
        };
        samples.resize(channels.saturating_mul(frames), 0.0);
        self.data = Data::Float(samples);
        match &mut self.data {
            Data::Float(samples) => samples,
            Data::Int { .. } => &mut [],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_allocation_overflow_is_fallible() {
        for frames in [1, 2] {
            let mut pcm = Pcm::new();
            assert!(matches!(
                pcm.try_int_buffer(usize::MAX, frames, 16),
                Err(crate::Error::LimitExceeded(crate::LimitKind::OutputSamples))
            ));
            assert_eq!(pcm.frames(), 0);
            assert_eq!(pcm.channels(), 0);
        }
    }
}
