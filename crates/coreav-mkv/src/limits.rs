//! Resource limits (defaults in the crate docs).

/// Bounds on the work and memory of one request.
///
/// Every entry point takes a `Limits` value. [`Limits::default`] gives the
/// defaults listed in the crate documentation; callers may raise or lower
/// any of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Element headers read while probing (clusters excluded).
    pub max_elements: u32,
    /// Bytes of one element read into memory: the EBML header, `SeekHead`,
    /// `Info`, one `TrackEntry`, one `CuePoint`, or the header and lacing of
    /// one block.
    pub max_element_bytes: u64,
    /// Metadata bytes retained across the whole segment (codec private data,
    /// strings).
    pub max_retained_bytes: u64,
    /// Bytes of cue points retained when the index is read.
    pub max_index_bytes: u64,
    /// Bytes of one frame.
    pub max_packet_bytes: u64,
    /// Element headers visited per packet iteration or cluster scan.
    pub max_block_work: u64,
    /// Tracks in one segment.
    pub max_tracks: u32,
    /// The budget of a packet listing that sizes zlib-compressed frames
    /// ([`crate::Packets::with_inflated_sizes`]), across its tracks: each
    /// frame is charged the bytes it inflated to (all of its bound for one
    /// that went past it) plus 4 KiB, and frames past the budget have an
    /// unknown size. Each frame is also bounded by `max_packet_bytes`.
    pub max_inflated_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_elements: 1_000_000,
            max_element_bytes: 1 << 20,
            max_retained_bytes: 16 << 20,
            max_index_bytes: 256 << 20,
            max_packet_bytes: 128 << 20,
            max_block_work: 1 << 26,
            max_tracks: 1_024,
            max_inflated_bytes: 256 << 20,
        }
    }
}
