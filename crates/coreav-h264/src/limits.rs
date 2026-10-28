//! Resource limits (`docs/design/h264.md`).
#![forbid(unsafe_code)]

/// Bounds on the work and memory of one decoding session.
///
/// Every entry point takes a `Limits` value. [`Limits::default`] allows what
/// a level 6.2 stream may use, so conforming streams decode; callers that
/// decode untrusted uploads lower them. There is no hidden global default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Macroblocks in one picture: MaxFS of Table A-1, level 6.2 (a
    /// 8192x4320 picture is 138,240).
    pub max_mbs: usize,
    /// Pictures the decoded picture buffer holds. The specification's own
    /// maximum is 16 (clause A.3.1), which is the default.
    ///
    /// It bounds what one decoder retains: at most `max_dpb_frames + 1`
    /// pictures in the buffer, plus a few whose buffers are kept for
    /// reuse. A stream whose `max_num_ref_frames` is above it is refused
    /// with [`crate::LimitKind::DpbFrames`] rather than decoded with fewer
    /// references than it needs. Pictures the caller has not taken yet are
    /// the caller's own memory.
    pub max_dpb_frames: usize,
    /// Slices of one picture. The specification allows one per macroblock.
    pub max_slices_per_picture: usize,
    /// Bytes of one NAL unit, which bounds what [`crate::AnnexB`] holds
    /// while it looks for the next start code.
    pub max_nal_bytes: usize,
    /// Threads one decoder may decode with ([`crate::Decoder::with_threads`]),
    /// which is also how many pictures it decodes at once. Each picture in
    /// flight holds its samples, about half as many again in the copies
    /// its bands keep of their neighbours, and the state of every
    /// macroblock.
    pub max_threads: usize,
    /// Bytes the decoder may hold ([`crate::Decoder::memory_needed`]): its
    /// pictures with their motion and the copies bands keep, the state of
    /// each thread, the slice data it holds (`max_nal_bytes` bounds that),
    /// and a copy of every picture one flush releases, which
    /// is what a caller that copies out each picture it takes holds at
    /// once (the decode boundary does). Checked before the first picture
    /// of each size: the thread count is lowered to fit, down to one, and
    /// a size that does not fit one thread is refused with
    /// [`crate::LimitKind::Memory`].
    pub max_memory_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_mbs: 139_264,
            max_dpb_frames: 16,
            max_slices_per_picture: 139_264,
            max_nal_bytes: 64 << 20,
            max_threads: 16,
            max_memory_bytes: 8 << 30,
        }
    }
}

impl Limits {
    /// Limits for one picture of at most `width` by `height` samples, with
    /// everything else as [`Limits::default`].
    ///
    /// Thumbnails and policy gates know the size they are willing to decode
    /// before they open the stream.
    pub fn for_size(width: usize, height: usize) -> Limits {
        let mbs = width
            .div_ceil(16)
            .saturating_mul(height.div_ceil(16))
            .max(1);
        Limits {
            max_mbs: mbs,
            ..Limits::default()
        }
    }
}
