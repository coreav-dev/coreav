//! Resource limits (`docs/design/audio-decode.md`).

/// Bounds on the work and memory of one decoding request.
///
/// Every entry point takes a `Limits` value; [`Limits::default`] gives the
/// defaults below, and callers may raise or lower any of them. The
/// container limits are the demuxers' own, with their defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The MP4/MOV demuxer's limits.
    pub isobmff: coreav_isobmff::Limits,
    /// The Matroska/WebM demuxer's limits.
    pub matroska: coreav_mkv::Limits,
    /// Channels of one stream. Default 32.
    pub max_channels: u32,
    /// Bytes of one packet read into memory. Default 16 MiB, eight times
    /// the largest FLAC frame (65,535 verbatim 32-bit samples on each of 8
    /// channels).
    pub max_packet_bytes: u64,
    /// Samples written, counted per channel (one second of 48 kHz stereo
    /// is 96,000). Default 2^33, about 24 hours of 48 kHz stereo: decoding
    /// multiplies sizes (a 49-byte FLAC frame can code 2 MiB of PCM, RFC
    /// 9639 section 11), so the output is bounded by itself, not by the
    /// input's size.
    pub max_output_samples: u64,
    /// Packets that fail to decode and are replaced by silence before the
    /// request fails. Default 100.
    pub max_decode_errors: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            isobmff: coreav_isobmff::Limits::default(),
            matroska: coreav_mkv::Limits::default(),
            max_channels: 32,
            max_packet_bytes: 16 << 20,
            max_output_samples: 1 << 33,
            max_decode_errors: 100,
        }
    }
}
