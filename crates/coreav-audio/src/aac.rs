//! AAC LC through `coreav-aac`, behind the opt-in feature `aac`
//! (`docs/design/audio-decode.md`, "AAC-LC"). Decoding AAC may need a
//! patent licence where you are; no default build has this module.

use crate::{Error, LimitKind, Limits, Pcm};

/// An AAC LC decoder for one stream.
#[derive(Clone)]
pub(crate) struct AacDecoder {
    decoder: coreav_aac::Decoder,
}

/// The configuration of a track, as its container gives it.
pub(crate) enum Source<'a> {
    /// An `AudioSpecificConfig`: MP4's `DecoderSpecificInfo`, Matroska's
    /// `CodecPrivate`.
    Config(&'a [u8]),
    /// No configuration: the rate and channels the container states (a
    /// Matroska track with a legacy codec ID such as `A_AAC/MPEG4/LC`).
    Declared {
        sample_rate: Option<u32>,
        channels: Option<u32>,
    },
}

impl AacDecoder {
    /// A decoder for the configuration `source`. Other object types and
    /// HE-AAC, signalled in the configuration or by an SBR payload in the
    /// first packet `first`, are refused by name.
    pub(crate) fn new(
        source: Source<'_>,
        first: Option<&[u8]>,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let aac_limits = coreav_aac::Limits {
            max_channels: usize::try_from(limits.max_channels).unwrap_or(usize::MAX),
        };
        let config = match source {
            Source::Config(data) => coreav_aac::Config::parse(data, &aac_limits),
            Source::Declared {
                sample_rate,
                channels,
            } => match (sample_rate, channels) {
                (Some(rate), Some(channels)) => coreav_aac::Config::from_channels(
                    rate,
                    usize::try_from(channels).unwrap_or(usize::MAX),
                    1024,
                    &aac_limits,
                ),
                _ => return Err(Error::Invalid("an AAC track without a configuration")),
            },
        }
        .map_err(|error| match error {
            coreav_aac::Error::Unsupported(name) => Error::CodecNotDecoded(name.to_owned()),
            error => convert(error),
        })?;
        let decoder = Self {
            decoder: coreav_aac::Decoder::new(&config),
        };
        // Implicit SBR signalling: the first packet's fill elements. The
        // trial decode is on a copy, so the stream starts from a reset
        // decoder at position 0.
        if let Some(first) = first {
            let mut trial = decoder.clone();
            if let Err(Error::Unsupported(name)) = trial.decode(first, &mut Pcm::new()) {
                return Err(Error::CodecNotDecoded(name.to_owned()));
            }
        }
        Ok(decoder)
    }

    pub(crate) fn config(&self) -> &coreav_aac::Config {
        self.decoder.config()
    }

    /// Samples per channel of every frame.
    pub(crate) fn frame_length(&self) -> usize {
        self.decoder.config().frame_length()
    }

    /// Forgets the frames before (a job of the pool starts).
    pub(crate) fn reset(&mut self) {
        self.decoder.reset();
    }

    /// The stream position of the next packet: its noise depends on it.
    pub(crate) fn set_position(&mut self, position: u64) {
        self.decoder.set_position(position);
    }

    /// Decodes one packet into `pcm`; its samples per channel.
    pub(crate) fn decode(&mut self, data: &[u8], pcm: &mut Pcm) -> Result<u64, Error> {
        let channels = self.decoder.config().channels();
        let frame = self.frame_length();
        let out = pcm.float_buffer(channels, frame);
        let decoded = self.decoder.decode(data, out).map_err(convert)?;
        Ok(decoded.samples as u64)
    }
}

/// The crate's error for a packet's or a configuration's.
fn convert(error: coreav_aac::Error) -> Error {
    match error {
        coreav_aac::Error::Invalid(what) => Error::Invalid(what),
        coreav_aac::Error::Unsupported(what) => Error::Unsupported(what),
        coreav_aac::Error::TooManyChannels { .. } => Error::LimitExceeded(LimitKind::Channels),
    }
}
