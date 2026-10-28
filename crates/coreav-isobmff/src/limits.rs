//! Resource limits.

/// Bounds on the work and memory of one request.
///
/// Every entry point takes a `Limits` value. [`Limits::default`] gives the
/// decided defaults; callers may raise or lower any of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Top-level boxes, not counting movie fragments.
    pub max_top_level_boxes: u32,
    /// Box nesting depth.
    pub max_depth: u32,
    /// Boxes visited in total.
    pub max_total_boxes: u32,
    /// Bytes of one box payload read into memory (sample tables excluded).
    pub max_box_bytes: u64,
    /// Metadata bytes retained across the whole movie.
    pub max_retained_bytes: u64,
    /// Bytes of sample tables retained across all tracks.
    pub max_table_bytes: u64,
    /// Bytes of one packet.
    pub max_packet_bytes: u64,
    /// Sample and table-entry visits per request, across all tracks.
    pub max_sample_work: u64,
    /// Movie fragments per request.
    pub max_fragments: u32,
    /// Tracks in one movie.
    pub max_tracks: u32,
    /// Bytes of a compressed movie box (QuickTime `cmov`) once inflated,
    /// held in memory while it is read (with one 64 KiB read window).
    pub max_movie_bytes: u64,
    /// Most bytes a compressed movie box may inflate to per compressed
    /// byte.
    pub max_movie_ratio: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_top_level_boxes: 10_000,
            max_depth: 32,
            max_total_boxes: 1_000_000,
            max_box_bytes: 1 << 20,
            max_retained_bytes: 16 << 20,
            max_table_bytes: 256 << 20,
            max_packet_bytes: 128 << 20,
            max_sample_work: 1 << 26,
            max_fragments: 1_000_000,
            max_tracks: 1_024,
            max_movie_bytes: 16 << 20,
            max_movie_ratio: 256,
        }
    }
}
