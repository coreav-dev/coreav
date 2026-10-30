//! Memory-safe audio decoding to PCM.
//!
//! The decoders take one codec frame per packet and the codec
//! configuration as containers carry it, and write planar samples into a
//! reused [`Pcm`] buffer. They are sans-IO: bytes in, samples out. The
//! design, the limits, the bars and the rejected alternatives are recorded
//! in `docs/design/audio-decode.md`.
//!
//! - [`flac`]: FLAC (RFC 9639), native.
//! - [`mp3`]: MPEG audio Layer III (ISO/IEC 11172-3, 13818-3), native.
//! - AAC LC (ISO/IEC 14496-3) through `coreav-aac`, only with the opt-in
//!   feature `aac`, off by default: decoding AAC may need a patent licence
//!   where you are. Without it an AAC stream is `codec aac is not decoded`.
//! - [`decode()`]: the audio track of an MP4/MOV or Matroska/WebM file to
//!   interleaved PCM, through the existing demuxers, into a [`Sink`] such
//!   as [`WavSink`] ([`wav`]), at the stream's rate and channels or
//!   resampled and downmixed ([`Options::sample_rate`],
//!   [`Options::channels`]).
//!
//! Every entry point takes [`Limits`]; malformed or hostile input produces
//! an [`Error`], never a panic or unbounded work.
//!
//! ```
//! use coreav_audio::flac::{FlacDecoder, StreamInfo};
//! use coreav_audio::{Limits, Pcm};
//!
//! assert!(StreamInfo::from_config(b"not a configuration").is_err());
//! ```

#[cfg(feature = "aac")]
mod aac;
mod bits;
mod conversion;
mod convert;
mod crc;
mod decode;
mod error;
pub mod flac;
mod limits;
mod mix;
pub mod mp3;
mod pcm;
mod resample;
mod stream;
pub mod wav;

pub use decode::{
    Codec, Options, OutputFormat, SampleFormat, SilenceSpan, Sink, StreamFormat, Summary, Timeline,
    decode,
};
pub use error::{Error, LimitKind, Warning};
pub use limits::Limits;
pub use pcm::{Pcm, Plane};
pub use stream::MAX_THREADS;
pub use wav::WavSink;
