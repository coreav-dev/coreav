//! Transcoding to H.264 in MP4 (`docs/design/transcode.md`; experimental for
//! sources other than H.264):
//! one video stream decoded through CoreAV's decode boundary, encoded by an
//! operator-provided encoder program (an `x264` binary, or `ffmpeg -c:v
//! libx264`) run as a separate, resource-limited process, and muxed with
//! the source's audio copied.
//!
//! - [`annexb`] reads the encoder's H.264 byte stream back into MP4
//!   samples: access units, picture order counts and presentation order,
//!   and the `avcC` record.
//! - [`timing`] gives the samples' decode and presentation times from the
//!   source frames' timestamps and the pictures' presentation order.
//! - [`encoder`] runs the encoder program: its command line, the frames to
//!   its standard input and its byte stream from its standard output.
//! - [`rawmkv`] writes the frames with their times as Matroska, for
//!   `ffmpeg -c:v libx264`, which cannot read times with raw video.
//! - [`scale`] reads `--scale W:H` and gives the output size.
//! - [`pipeline`] is the whole transcode: [`transcode`] decodes the source's
//!   video stream, encodes its presented frames and writes the MP4 with the
//!   source's audio copied.

#![forbid(unsafe_code)]

pub mod aac;
pub mod annexb;
pub mod audio;
pub mod encoder;
pub mod pipeline;
pub mod rawmkv;
pub mod scale;
pub mod timing;

pub use aac::AacAudio;
pub use audio::{AudioSummary, OpusAudio};
pub use coreav_frame::PixelFormat;
pub use pipeline::{
    AudioMode, Error, Remuxed, SourceKind, Summary, TranscodeOptions, VideoSummary, transcode,
};
pub use scale::{Scale, Side, parse_scale};
