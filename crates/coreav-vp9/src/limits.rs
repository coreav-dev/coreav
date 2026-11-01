//! Resource limits (`docs/design/vp9.md`, "Architecture").

/// Bounds on the work and memory of one decoder.
///
/// Every decoder takes a `Limits` value. [`Limits::default`] allows
/// frames up to 16384 samples on a side and 2^26 luma samples (8192x8192),
/// and packets up to 64 MiB; callers that decode untrusted uploads lower
/// them. A frame over a limit is refused when its header is read, before
/// anything is allocated for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Samples across one frame (`FrameWidth`).
    pub max_width: u32,
    /// Samples down one frame (`FrameHeight`).
    pub max_height: u32,
    /// Luma samples of one frame, `FrameWidth * FrameHeight`.
    pub max_pixels: u64,
    /// Bytes of one packet: a frame, or a superframe with its index.
    pub max_packet_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_width: 16_384,
            max_height: 16_384,
            max_pixels: 1 << 26,
            max_packet_bytes: 64 << 20,
        }
    }
}

impl Limits {
    /// Limits for frames of at most `width` x `height` samples, with the
    /// default packet size.
    pub fn for_size(width: u32, height: u32) -> Self {
        Limits {
            max_width: width,
            max_height: height,
            max_pixels: u64::from(width) * u64::from(height),
            ..Limits::default()
        }
    }
}
