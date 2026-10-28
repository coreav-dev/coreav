//! The inflated size of a zlib-compressed frame (`ContentCompAlgo` 0, RFC
//! 9559 5.1.4.1.31.6; zlib and deflate, RFC 1950 and 1951).
//!
//! A frame is read from the source in chunks and inflated into a 32 KiB
//! window that wraps, so memory stays constant however large the frame
//! claims to inflate; the decompressor, the window and the read buffer are
//! allocated once per listing and reset for each frame. Only the size is
//! kept: no caller of the packet listing needs a compressed track's frames.

use crate::{Error, Source};
use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

/// Bytes of the stored frame read at a time.
const CHUNK: u64 = 64 << 10;
/// The output window: deflate's largest distance, a power of two.
const WINDOW: usize = 32 << 10;
/// What each frame costs against a listing's budget besides the bytes it
/// inflates to, so that a flood of tiny or empty frames is bounded too.
pub(crate) const FRAME_CHARGE: u64 = 4 << 10;

/// Why a frame has no inflated size.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// Reading the frame failed.
    Read(Error),
    /// The frame inflates to more than `max` bytes.
    TooLarge,
    /// The frame is not a complete zlib stream; the bytes it inflated to
    /// before that.
    Invalid(u64),
}

/// A decompressor and its buffers, reused from frame to frame.
pub(crate) struct Inflater {
    state: Box<DecompressorOxide>,
    window: Vec<u8>,
    chunk: Vec<u8>,
}

impl Inflater {
    pub(crate) fn new() -> Self {
        Self {
            state: Box::default(),
            window: vec![0u8; WINDOW],
            chunk: Vec::new(),
        }
    }

    /// The size of the `stored` bytes at `offset` once inflated, at most
    /// `max`.
    pub(crate) fn size<S: Source>(
        &mut self,
        source: &mut S,
        offset: u64,
        stored: u64,
        max: u64,
    ) -> Result<u64, Failure> {
        let end = offset.checked_add(stored).ok_or(Failure::Invalid(0))?;
        self.state.init();
        self.chunk.clear();
        let (mut at, mut used, mut total, mut position) = (offset, 0usize, 0u64, 0usize);
        loop {
            if used == self.chunk.len() {
                let take = end.saturating_sub(at).min(CHUNK);
                self.chunk.resize(
                    usize::try_from(take).map_err(|_| Failure::Invalid(total))?,
                    0,
                );
                source
                    .read_exact_at(at, &mut self.chunk)
                    .map_err(|e| Failure::Read(Error::from(e)))?;
                at = at.saturating_add(take);
                used = 0;
            }
            let more = at < end;
            let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
                | inflate_flags::TINFL_FLAG_COMPUTE_ADLER32
                | if more {
                    inflate_flags::TINFL_FLAG_HAS_MORE_INPUT
                } else {
                    0
                };
            let input = self.chunk.get(used..).ok_or(Failure::Invalid(total))?;
            let (status, read, wrote) =
                decompress(&mut self.state, input, &mut self.window, position, flags);
            used = used.saturating_add(read);
            position = (position + wrote) % WINDOW;
            total = total.saturating_add(wrote as u64);
            if total > max {
                return Err(Failure::TooLarge);
            }
            match status {
                // Bytes after the end of the stream, inside the frame, are
                // not read.
                TINFLStatus::Done => return Ok(total),
                // The window is full: inflate on into it.
                TINFLStatus::HasMoreOutput => {}
                TINFLStatus::NeedsMoreInput if more => {}
                _ => return Err(Failure::Invalid(total)),
            }
            if read == 0 && wrote == 0 && used < self.chunk.len() {
                // No progress on the same input: never loop.
                return Err(Failure::Invalid(total));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SliceSource;

    fn zlib(data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
    }

    fn size(inflater: &mut Inflater, data: &[u8], max: u64) -> Result<u64, Failure> {
        inflater.size(&mut SliceSource::new(data), 0, data.len() as u64, max)
    }

    #[test]
    fn sizes_of_valid_streams_with_one_inflater() {
        let mut inflater = Inflater::new();
        assert_eq!(size(&mut inflater, &zlib(b""), 100), Ok(0));
        assert_eq!(size(&mut inflater, &zlib(b"hello"), 100), Ok(5));
        // Larger than the window and than one read.
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(size(&mut inflater, &zlib(&big), 1 << 20), Ok(300_000));
        // A failed frame leaves nothing behind for the next.
        assert_eq!(
            size(&mut inflater, b"not zlib", 100),
            Err(Failure::Invalid(0))
        );
        assert_eq!(size(&mut inflater, &zlib(b"again"), 100), Ok(5));
    }

    #[test]
    fn a_bomb_stops_at_the_limit() {
        let mut inflater = Inflater::new();
        let bomb = zlib(&vec![0u8; 8 << 20]);
        assert_eq!(size(&mut inflater, &bomb, 1 << 20), Err(Failure::TooLarge));
        assert_eq!(size(&mut inflater, &bomb, 8 << 20), Ok(8 << 20));
    }

    #[test]
    fn broken_streams_are_invalid() {
        let mut inflater = Inflater::new();
        let good = zlib(&[7u8; 1000]);
        assert!(matches!(
            size(&mut inflater, &good[..good.len() / 2], 1 << 20),
            Err(Failure::Invalid(_))
        ));
        assert_eq!(size(&mut inflater, b"", 1 << 20), Err(Failure::Invalid(0)));
        let mut bad_check = good.clone();
        let last = bad_check.len() - 1;
        bad_check[last] ^= 1;
        // The whole stream inflated before its check failed.
        assert_eq!(
            size(&mut inflater, &bad_check, 1 << 20),
            Err(Failure::Invalid(1000))
        );
    }
}
