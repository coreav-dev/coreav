//! The replaced-stream remux (`docs/design/remux.md`, "One video stream
//! replaced"): the caller's description of the samples that take the place
//! of one source video stream, and its validation. The remuxes themselves
//! are [`crate::remux_mp4_replacing`] and [`crate::remux_matroska_replacing`].

use coreav_isobmff::{Error, FourCc, LimitKind, Limits, Source};

/// One sample of a [`Replacement`], in decode order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplacementSample {
    /// Size in bytes. The sample's bytes follow the previous sample's in
    /// [`Replacement::data`], the first at offset 0.
    pub size: u32,
    /// Sync sample (`stss`): for H.264, an IDR access unit.
    pub sync: bool,
    /// Decode time, in the units and on the timeline of the replaced
    /// stream's own timestamps (see [`Replacement`]). Strictly increasing
    /// from sample to sample; may be negative.
    pub decode_time: i64,
    /// Presentation (composition) time, in the same units and on the same
    /// timeline; at least `decode_time`.
    pub presentation_time: i64,
}

/// Samples that replace one video stream of a remuxed source: what
/// [`crate::remux_mp4_replacing`] and [`crate::remux_matroska_replacing`]
/// write in that stream's place.
///
/// # Time units and timeline
///
/// Sample times are in the units, and on the timeline, of the replaced
/// source stream's own sample timestamps:
///
/// - **MP4 and MOV:** media ticks of the source track's media timescale
///   (`mdhd`), on its media timeline: the decode times and composition
///   times its sample tables give (`coreav_isobmff::Packet::dts` and
///   `coreav_isobmff::Packet::cts`, the decode time plus the composition
///   offset). The source track's edit list keeps mapping that timeline to
///   the movie: a frame with presentation time `P` is presented at the
///   movie time at which the plain [`crate::remux_mp4`] presents a copied
///   source sample whose composition time is `P`.
/// - **Matroska and WebM:** Segment Ticks (units of the segment's
///   `TimestampScale`, nanoseconds × `TimestampScale`) on the segment's
///   timeline: the blocks' timestamps as `coreav_mkv::Packet::pts` reports
///   them for a video track (block timestamp; no `CodecDelay` for video).
///   A frame with presentation time `P` is presented at segment time `P`,
///   as [`crate::remux_matroska`] presents a copied frame with that
///   timestamp.
///
/// Decode times may start anywhere (negative included), and presentation
/// times need not start at the source's first one: frames the source does
/// not present can be left out. The output's decode times start at 0; the
/// writer moves the edit list by the difference.
#[derive(Clone, Copy, Debug)]
pub struct Replacement<'a, R> {
    /// The source stream replaced: its position among all tracks of the
    /// source (`Track::stream_index`), which must be a video stream.
    pub stream_index: usize,
    /// One complete sample entry box, header included, for example from
    /// [`crate::mux::avc1_sample_entry`]. Its type must be `avc1` or
    /// `avc3` (H.264), `hvc1` or `hev1` (HEVC), `av01` (AV1) or `vp09`
    /// (VP9), and it must be a visual sample entry (ISO/IEC 14496-12
    /// 12.1.3) with data reference index 1; its contents are written as
    /// given.
    pub sample_entry: &'a [u8],
    /// The track header's presentation width, 16.16 fixed point.
    pub width: u32,
    /// The track header's presentation height, 16.16 fixed point.
    pub height: u32,
    /// The samples, in decode order; at least one.
    pub samples: &'a [ReplacementSample],
    /// Duration of the last sample (in decode order), in the units of the
    /// sample times; at least 1.
    pub last_duration: u32,
    /// The samples' bytes back to back from offset 0, in decode order (the
    /// transcoder's spool file). Bytes after the last sample are ignored.
    pub data: R,
}

/// The codec a replacement's sample entry type names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
}

impl EntryCodec {
    pub(crate) fn isobmff(self) -> coreav_isobmff::Codec {
        match self {
            Self::H264 => coreav_isobmff::Codec::H264,
            Self::Hevc => coreav_isobmff::Codec::Hevc,
            Self::Av1 => coreav_isobmff::Codec::Av1,
            Self::Vp9 => coreav_isobmff::Codec::Vp9,
        }
    }

    pub(crate) fn matroska(self) -> coreav_mkv::Codec {
        match self {
            Self::H264 => coreav_mkv::Codec::H264,
            Self::Hevc => coreav_mkv::Codec::Hevc,
            Self::Av1 => coreav_mkv::Codec::Av1,
            Self::Vp9 => coreav_mkv::Codec::Vp9,
        }
    }
}

/// Checks a replacement against itself, its data and `limits`, before
/// anything is written: a sample entry of a known type that is one box,
/// at least one sample, decode times strictly increasing with differences
/// that fit a 32-bit duration, presentation times at or after their decode
/// times, a last duration of at least 1, and every sample's bytes within
/// the data. Returns the codec of the sample entry.
pub(crate) fn validate<R: Source>(
    replacement: &mut Replacement<'_, R>,
    limits: &Limits,
) -> Result<EntryCodec, Error> {
    let entry = replacement.sample_entry;
    let size = entry
        .get(..4)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_be_bytes);
    if size.is_none_or(|s| s < 8 || usize::try_from(s).ok() != Some(entry.len())) {
        return Err(unsupported("the replacement sample entry is not one box"));
    }
    if u64::try_from(entry.len()).unwrap_or(u64::MAX) > limits.max_box_bytes {
        return Err(Error::LimitExceeded {
            limit: LimitKind::BoxBytes,
            offset: 0,
        });
    }
    let kind = entry
        .get(4..8)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(FourCc);
    let codec = match kind.map(|k| k.0) {
        Some(ref k) if k == b"avc1" || k == b"avc3" => EntryCodec::H264,
        Some(ref k) if k == b"hvc1" || k == b"hev1" => EntryCodec::Hevc,
        Some(ref k) if k == b"av01" => EntryCodec::Av1,
        Some(ref k) if k == b"vp09" => EntryCodec::Vp9,
        _ => {
            return Err(unsupported(
                "the replacement sample entry is not avc1, avc3, hvc1, hev1, av01 or vp09",
            ));
        }
    };
    // A visual sample entry (ISO/IEC 14496-12 12.1.3): 78 bytes of fields
    // after the header, six reserved zero bytes, then data reference 1 (the
    // output's one `dref` entry).
    if entry.len() < 86
        || entry.get(8..14) != Some(&[0u8; 6][..])
        || entry.get(14..16) != Some(&[0, 1][..])
    {
        return Err(unsupported(
            "the replacement sample entry is not a visual sample entry with data reference 1",
        ));
    }
    let samples = replacement.samples;
    if samples.is_empty() {
        return Err(unsupported("the replacement has no samples"));
    }
    if u32::try_from(samples.len()).is_err() {
        return Err(unsupported("more than 2^32 - 1 samples in one track"));
    }
    if replacement.last_duration == 0 {
        return Err(unsupported("the replacement's last sample lasts 0"));
    }
    let mut bytes = 0u64;
    let mut previous: Option<i64> = None;
    for sample in samples {
        if let Some(previous) = previous {
            let step = sample
                .decode_time
                .checked_sub(previous)
                .filter(|d| *d > 0)
                .ok_or(unsupported(
                    "the replacement's decode times do not strictly increase",
                ))?;
            if u32::try_from(step).is_err() {
                return Err(unsupported(
                    "a replacement sample lasts too long for a 32-bit duration",
                ));
            }
        }
        previous = Some(sample.decode_time);
        if sample.presentation_time < sample.decode_time {
            return Err(unsupported(
                "a replacement sample is presented before it is decoded",
            ));
        }
        if u64::from(sample.size) > limits.max_packet_bytes {
            return Err(Error::LimitExceeded {
                limit: LimitKind::PacketBytes,
                offset: bytes,
            });
        }
        bytes = bytes
            .checked_add(u64::from(sample.size))
            .ok_or(Error::Overflow { offset: bytes })?;
    }
    // The end of the last sample's presentation must be representable.
    let last = samples
        .iter()
        .map(|s| s.presentation_time)
        .max()
        .unwrap_or(0);
    let first = samples.first().map_or(0, |s| s.decode_time);
    last.checked_add(i64::from(replacement.last_duration))
        .and_then(|end| end.checked_sub(first))
        .ok_or(Error::Overflow { offset: 0 })?;
    if bytes > replacement.data.len()? {
        return Err(unsupported(
            "the replacement's samples run past the end of its data",
        ));
    }
    Ok(codec)
}

/// The replacement's samples as (decode time, duration) pairs: each lasts
/// up to the next one's decode time, the last [`Replacement::last_duration`].
/// [`validate`] has checked that every duration fits.
pub(crate) fn durations<'s>(
    samples: &'s [ReplacementSample],
    last_duration: u32,
) -> impl Iterator<Item = (&'s ReplacementSample, u32)> + 's {
    samples.iter().enumerate().map(move |(i, s)| {
        let duration = samples
            .get(i.saturating_add(1))
            .and_then(|next| next.decode_time.checked_sub(s.decode_time))
            .and_then(|d| u32::try_from(d).ok())
            .unwrap_or(last_duration);
        (s, duration)
    })
}

fn unsupported(reason: &'static str) -> Error {
    Error::Unsupported { reason }
}
