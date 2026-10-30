//! The `coreav.error/1` object printed on stderr (`docs/schema/probe-v1.md`).

use coreav_isobmff::{Error, LimitKind};
use serde_json::{Value, json};

/// Why a probe produced no output.
pub enum Failure {
    /// The library rejected the file or could not read it.
    Library(Error),
    /// The Matroska library rejected the file or could not read it.
    Matroska(coreav_mkv::Error),
    /// The transport stream library rejected the file or could not read it.
    Mpegts(coreav_mpegts::Error),
    /// The file could not be inspected before probing.
    Io(std::io::ErrorKind),
    /// The file cannot be processed as asked, for the reason given.
    Unsupported(String),
    /// A remote object could not be read, for the reason given (an I/O
    /// error: exit 3).
    Remote(String),
    /// The audio decoder rejected the stream: the `coreav.error/1` object
    /// built from its error (`crate::decode_audio`).
    Decode(Value),
    /// Video decoding failed, or no decoder backend could decode the
    /// stream (`coreav thumbnail`).
    VideoDecode(coreav_decode::Error),
    /// The encoder program of `coreav transcode` failed, was stopped at
    /// the deadline, or wrote a stream that does not match its frames.
    Encode(coreav_decode::Error),
    /// The time asked for is at or after the end of the video.
    OutOfRange,
    /// Strict-safe mode refused what would run an `ffmpeg` program, for
    /// the reason given, which names the way out (`crate::strict_safe`):
    /// exit status 4.
    StrictSafe(String),
}

impl Failure {
    /// Wraps a library error.
    pub fn library(error: Error) -> Self {
        Self::Library(error)
    }

    /// Wraps a transport stream library error.
    pub fn mpegts(error: coreav_mpegts::Error) -> Self {
        Self::Mpegts(error)
    }

    /// Wraps an I/O error from the standard library.
    pub fn io(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }

    /// The file could not be opened or read, as opposed to being rejected.
    pub fn is_io(&self) -> bool {
        matches!(
            self,
            Self::Io(_)
                | Self::Remote(_)
                | Self::Library(Error::Io(_))
                | Self::Matroska(coreav_mkv::Error::Io(_))
                | Self::Mpegts(coreav_mpegts::Error::Io(_))
        ) || matches!(self, Self::VideoDecode(e) | Self::Encode(e) if e.kind == coreav_decode::ErrorKind::Io)
    }

    /// Strict-safe mode's refusal: exit status 4.
    pub fn is_strict_safe(&self) -> bool {
        matches!(self, Self::StrictSafe(_))
    }

    /// The error object.
    pub fn to_json(&self) -> Value {
        match self {
            Self::StrictSafe(message) => object("unsupported_feature", None, None, message),
            Self::Io(kind) => object("io", None, None, &format!("I/O error: {kind}")),
            Self::Remote(reason) => object("io", None, None, &format!("remote: {reason}")),
            Self::Decode(value) => value.clone(),
            Self::OutOfRange => object(
                "out_of_range",
                None,
                None,
                "the time is at or after the end of the video",
            ),
            Self::VideoDecode(error) => {
                let (kind, limit) = decode_kind(error.kind);
                object(kind, limit, None, &format!("decoding: {}", error.message))
            }
            // A failed backend: `decode`, its message saying which side. Only
            // the deadline, a limit and I/O keep their kinds; never a
            // refusal's (`unsupported_feature`), so that a defect of the
            // encoding side cannot pass as one (`transcode-v1.md`).
            Self::Encode(error) => {
                use coreav_decode::ErrorKind as K;
                let (kind, limit) = match error.kind {
                    K::Limit | K::Timeout | K::Io => decode_kind(error.kind),
                    K::CodecNotAllowed | K::Unsupported | K::InvalidData | K::Backend => {
                        ("decode", None)
                    }
                };
                object(kind, limit, None, &format!("encoding: {}", error.message))
            }
            Self::Unsupported(message) => object(
                "unsupported_feature",
                None,
                None,
                &format!("unsupported: {message}"),
            ),
            Self::Library(error) => {
                let (kind, offset) = match error {
                    Error::Truncated { offset } => ("truncated_structure", Some(*offset)),
                    Error::InvalidBox { offset, .. } | Error::Overflow { offset } => {
                        ("invalid_structure", Some(*offset))
                    }
                    Error::LimitExceeded { offset, .. } => ("limit", Some(*offset)),
                    Error::NotIsobmff => ("unsupported_container", None),
                    Error::Unsupported { .. } => ("unsupported_feature", None),
                    Error::CodecNotAllowed { .. } => ("codec_not_allowed", None),
                    // Only the commands that write ask for a track: the file
                    // has none of the kind asked for, or no stream with the
                    // index given. A refusal like the other unsupported cases.
                    Error::NoSuchTrack => ("unsupported_feature", None),
                    Error::Io(_) => ("io", None),
                };
                let limit = match error {
                    Error::LimitExceeded { limit, .. } => Some(limit_name(*limit)),
                    _ => None,
                };
                object(kind, limit, offset, &error.to_string())
            }
            Self::Matroska(error) => {
                use coreav_mkv::Error as E;
                let (kind, offset) = match error {
                    E::Truncated { offset } => ("truncated_structure", Some(*offset)),
                    E::InvalidElement { offset, .. } | E::Overflow { offset } => {
                        ("invalid_structure", Some(*offset))
                    }
                    E::LimitExceeded { offset, .. } => ("limit", Some(*offset)),
                    E::NotMatroska => ("unsupported_container", None),
                    E::Unsupported { .. } => ("unsupported_feature", None),
                    E::CodecNotAllowed { .. } => ("codec_not_allowed", None),
                    E::Io(_) => ("io", None),
                };
                let limit = match error {
                    E::LimitExceeded { limit, .. } => Some(matroska_limit_name(*limit)),
                    _ => None,
                };
                object(kind, limit, offset, &error.to_string())
            }
            Self::Mpegts(error) => {
                use coreav_mpegts::Error as E;
                let (kind, offset) = match error {
                    E::Invalid { offset, .. } => ("invalid_structure", Some(*offset)),
                    E::LimitExceeded { offset, .. } => ("limit", Some(*offset)),
                    E::NotTransportStream => ("unsupported_container", None),
                    // No program table within `max_probe_bytes`: the bytes
                    // are shaped like a transport stream but name no stream.
                    E::NoPrograms => ("invalid_structure", None),
                    E::CodecNotAllowed { .. } => ("codec_not_allowed", None),
                    E::Io(_) => ("io", None),
                };
                let limit = match error {
                    E::LimitExceeded { limit, .. } => Some(mpegts_limit_name(*limit)),
                    _ => None,
                };
                object(kind, limit, offset, &error.to_string())
            }
        }
    }
}

/// The `coreav.error/1` kind (and limit) of a decode boundary error.
fn decode_kind(kind: coreav_decode::ErrorKind) -> (&'static str, Option<&'static str>) {
    use coreav_decode::ErrorKind as K;
    match kind {
        K::CodecNotAllowed => ("codec_not_allowed", None),
        K::Limit => ("limit", None),
        K::Timeout => ("limit", Some("timeout")),
        K::Unsupported => ("unsupported_feature", None),
        K::InvalidData | K::Backend => ("decode", None),
        K::Io => ("io", None),
    }
}

/// The name of the [`coreav_mpegts::Limits`] field.
pub fn mpegts_limit_name(limit: coreav_mpegts::LimitKind) -> &'static str {
    use coreav_mpegts::LimitKind as L;
    match limit {
        L::ProbeBytes => "max_probe_bytes",
        L::ResyncBytes => "max_resync_bytes",
        L::Programs => "max_programs",
        L::Streams => "max_streams",
        L::PacketBytes => "max_packet_bytes",
        L::BufferedBytes => "max_buffered_bytes",
        L::ListingPackets => "max_listing_packets",
    }
}

/// The name of the [`coreav_mkv::Limits`] field.
pub fn matroska_limit_name(limit: coreav_mkv::LimitKind) -> &'static str {
    use coreav_mkv::LimitKind as L;
    match limit {
        L::Elements => "max_elements",
        L::ElementBytes => "max_element_bytes",
        L::RetainedBytes => "max_retained_bytes",
        L::IndexBytes => "max_index_bytes",
        L::PacketBytes => "max_packet_bytes",
        L::BlockWork => "max_block_work",
        L::Tracks => "max_tracks",
    }
}

fn object(kind: &str, limit: Option<&str>, offset: Option<u64>, message: &str) -> Value {
    json!({
        "schema": "coreav.error/1",
        "kind": kind,
        "limit": limit,
        "offset": offset,
        "message": message,
    })
}

/// The name of the [`coreav_isobmff::Limits`] field, or of the remote
/// reader's counter.
pub fn limit_name(limit: LimitKind) -> &'static str {
    match limit {
        LimitKind::TopLevelBoxes => "max_top_level_boxes",
        LimitKind::Depth => "max_depth",
        LimitKind::TotalBoxes => "max_total_boxes",
        LimitKind::BoxBytes => "max_box_bytes",
        LimitKind::RetainedBytes => "max_retained_bytes",
        LimitKind::TableBytes => "max_table_bytes",
        LimitKind::PacketBytes => "max_packet_bytes",
        LimitKind::SampleWork => "max_sample_work",
        LimitKind::Fragments => "max_fragments",
        LimitKind::Tracks => "max_tracks",
        LimitKind::MovieBytes => "max_movie_bytes",
        LimitKind::MovieRatio => "max_movie_ratio",
        LimitKind::ReadBytes => "read_bytes",
        LimitKind::ReadRequests => "read_requests",
    }
}
