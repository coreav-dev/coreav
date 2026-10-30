//! Resource limits (`docs/design/mpegts.md`).

/// Bounds on the work and memory of one request.
///
/// Every entry point takes a `Limits` value. [`Limits::default`] gives the
/// defaults below; callers may raise or lower any of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Bytes from the start of the file the probe reads to find the tables
    /// and each stream's parameters (16 MiB); a packet that ends past them
    /// is not read.
    pub max_probe_bytes: u64,
    /// Bytes at the end of the file the probe reads for the last
    /// timestamps (4 MiB).
    pub max_tail_bytes: u64,
    /// How many times the probe reads a tail twice as long as the last
    /// one for the streams that one showed no end of (0).
    pub tail_retries: u32,
    /// Bytes searched for the next sync byte after a lost packet, and at
    /// the start of the file (1 MiB).
    pub max_resync_bytes: u64,
    /// Programs in the PAT (256).
    pub max_programs: u32,
    /// Streams across every program (1,024).
    pub max_streams: u32,
    /// Bytes of one reassembled PES packet or frame (64 MiB).
    pub max_packet_bytes: u64,
    /// Bytes held at once across every PID: PES packets being reassembled
    /// and audio frames carried from one PES packet to the next (256 MiB).
    pub max_buffered_bytes: u64,
    /// Transport packets read by one packet listing (2^32: about 800 GB of
    /// 188-byte packets).
    pub max_listing_packets: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_probe_bytes: 16 << 20,
            max_tail_bytes: 4 << 20,
            tail_retries: 0,
            max_resync_bytes: 1 << 20,
            max_programs: 256,
            max_streams: 1_024,
            max_packet_bytes: 64 << 20,
            max_buffered_bytes: 256 << 20,
            max_listing_packets: 1 << 32,
        }
    }
}
