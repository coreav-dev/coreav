//! Errors and warnings.

use core::fmt;

/// Which limit of [`crate::Limits`] a request went past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// [`crate::Limits::max_channels`].
    Channels,
    /// [`crate::Limits::max_packet_bytes`].
    PacketBytes,
    /// [`crate::Limits::max_output_samples`].
    OutputSamples,
    /// [`crate::Limits::max_decode_errors`].
    DecodeErrors,
}

impl LimitKind {
    /// The limit's field name in [`crate::Limits`].
    pub fn name(self) -> &'static str {
        match self {
            Self::Channels => "max_channels",
            Self::PacketBytes => "max_packet_bytes",
            Self::OutputSamples => "max_output_samples",
            Self::DecodeErrors => "max_decode_errors",
        }
    }
}

/// Why a stream, a configuration or a packet could not be decoded.
#[derive(Debug)]
pub enum Error {
    /// The bitstream or codec configuration breaks its specification.
    Invalid(&'static str),
    /// Valid, but outside what this decoder implements.
    Unsupported(&'static str),
    /// A packet ended before its data did.
    Truncated,
    /// A limit of [`crate::Limits`] was reached.
    LimitExceeded(LimitKind),
    /// The MP4/MOV demuxer failed.
    Isobmff(coreav_isobmff::Error),
    /// The Matroska/WebM demuxer failed.
    Matroska(coreav_mkv::Error),
    /// The file has no stream that can be decoded as asked.
    NoSuchStream,
    /// The chosen stream's codec, named, is not one this crate decodes.
    CodecNotDecoded(String),
    /// The input is neither an ISO base media file nor a Matroska file.
    UnknownContainer,
    /// Writing the output failed.
    Output(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(what) => write!(f, "invalid bitstream: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Truncated => f.write_str("truncated packet"),
            Self::LimitExceeded(kind) => write!(f, "limit exceeded: {}", kind.name()),
            Self::Isobmff(error) => write!(f, "{error}"),
            Self::Matroska(error) => write!(f, "{error}"),
            Self::NoSuchStream => f.write_str("no such stream"),
            Self::CodecNotDecoded(codec) => write!(f, "codec {codec} is not decoded"),
            Self::UnknownContainer => f.write_str("not an MP4, MOV, Matroska or WebM file"),
            Self::Output(error) => write!(f, "write error: {}", error.kind()),
        }
    }
}

impl std::error::Error for Error {}

impl From<coreav_isobmff::Error> for Error {
    fn from(error: coreav_isobmff::Error) -> Self {
        Self::Isobmff(error)
    }
}

impl From<coreav_mkv::Error> for Error {
    fn from(error: coreav_mkv::Error) -> Self {
        Self::Matroska(error)
    }
}

/// A recoverable problem, reported with the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// Packets that could not be decoded; each was replaced by silence of
    /// its duration.
    Concealed {
        /// How many packets.
        packets: u64,
        /// The first packet's error.
        first: String,
    },
    /// FLAC frames whose CRC-16 did not match; their samples were kept.
    FrameCrc {
        /// How many frames.
        frames: u64,
    },
    /// The container states a sample rate or channel count that differs
    /// from the codec configuration's; the codec configuration is used.
    ContainerMismatch {
        /// What differs: `sample_rate` or `channels`.
        field: &'static str,
        /// The container's value.
        container: u64,
        /// The codec configuration's value, which is used.
        codec: u64,
    },
    /// Bytes followed the end of a packet's frame and were ignored.
    TrailingBytes {
        /// How many packets had them.
        packets: u64,
    },
    /// The output's sizes do not fit the WAV header's 32-bit fields; they
    /// are written as `0xFFFFFFFF`, as for an output of unknown length.
    WavTooLarge,
    /// The container could not be read past some point (a truncated or
    /// damaged end of the file); the samples decoded before it are kept.
    DemuxStopped {
        /// Packets decoded before it.
        after_packets: u64,
        /// The demuxer's error.
        error: String,
    },
    /// The MP4 edit list has a shape the demuxer does not apply exactly
    /// (several edits, or a rate other than 1); every sample is decoded.
    EditListIgnored,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Concealed { packets, first } => {
                write!(
                    f,
                    "{packets} packets could not be decoded and were replaced by silence (first: {first})"
                )
            }
            Self::FrameCrc { frames } => {
                write!(f, "{frames} FLAC frames failed their CRC-16 check")
            }
            Self::ContainerMismatch {
                field,
                container,
                codec,
            } => write!(
                f,
                "the container's {field} ({container}) differs from the codec configuration's ({codec}), which is used"
            ),
            Self::TrailingBytes { packets } => {
                write!(
                    f,
                    "{packets} packets had bytes after their frame, which were ignored"
                )
            }
            Self::DemuxStopped {
                after_packets,
                error,
            } => write!(
                f,
                "the file could not be read after {after_packets} packets ({error}); the output ends there"
            ),
            Self::EditListIgnored => {
                f.write_str("the edit list was not applied; every sample was decoded")
            }
            Self::WavTooLarge => f.write_str(
                "the output is too large for WAV's 32-bit sizes, which are written as 0xFFFFFFFF",
            ),
        }
    }
}
