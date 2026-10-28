//! MP3 frames from a byte stream, for containers whose samples are not one
//! frame each (some MOV and MP4 writers store MP3 in samples of any size,
//! which `ffmpeg` re-parses too). A packet that is one whole frame passes
//! through, and so does one that does not start with a frame header while
//! nothing is buffered (a damaged packet, which the decoder conceals).
//! Otherwise the bytes are buffered and cut at frame headers.
//!
//! At a frame boundary any Layer III header starts a frame, so that a frame
//! of another sample rate reaches the decoder, which refuses it. Past bytes
//! that are not a frame, only a header of the stream's version and sample
//! rate starts one again (a sync word inside data must not change the
//! stream's format), and the bytes skipped, however many packets they span,
//! go to the decoder as one damaged packet, concealed and counted like any
//! other, unless they are fewer than [`MIN_DAMAGED`]. Work and memory are
//! bounded: the buffer holds at most a packet and [`MAX_FRAME`] bytes, a
//! free-format frame's end or the end of a run of junk is looked for within
//! them, and so each packet costs at most a scan of that much.

use super::{Header, Version};

/// Bytes of the longest Layer III frame: free format at 640 kbit/s and
/// 8 kHz is 5,760 bytes.
const MAX_FRAME: usize = 8192;

/// Runs of bytes that are not a frame and shorter than this (less than a
/// header and its side information: stray padding) are dropped; longer ones
/// stand for a damaged frame and are concealed.
const MIN_DAMAGED: usize = 32;

pub(crate) struct Reframer {
    version: Version,
    sample_rate: u32,
    buffer: Vec<u8>,
}

impl Reframer {
    /// A reframer for the stream whose first frame has `header`.
    pub(crate) fn new(header: &Header) -> Self {
        Self {
            version: header.version,
            sample_rate: header.sample_rate,
            buffer: Vec::new(),
        }
    }

    /// Whether `packet` goes to the decoder as it is: nothing is buffered,
    /// and it is one whole frame (or a free-format one, whose length its
    /// header does not give), or it holds no frame header of the stream at
    /// all (a damaged packet, which the decoder conceals). A packet that
    /// holds one past its start (a stream cut anywhere, its first bytes the
    /// end of a frame) is cut at it.
    pub(crate) fn passes(&self, packet: &[u8]) -> bool {
        if !self.buffer.is_empty() {
            return false;
        }
        match Header::parse(packet) {
            Ok(header) => header.frame_bytes().is_none_or(|n| n == packet.len()),
            Err(_) => !(1..packet.len())
                .any(|at| self.is_stream_header(packet.get(at..).unwrap_or_default())),
        }
    }

    /// Adds a packet's bytes.
    pub(crate) fn push(&mut self, packet: &[u8]) {
        self.buffer.extend_from_slice(packet);
    }

    /// Whether `bytes` start with a header of the stream's version and
    /// sample rate.
    fn is_stream_header(&self, bytes: &[u8]) -> bool {
        Header::parse(bytes)
            .is_ok_and(|h| h.version == self.version && h.sample_rate == self.sample_rate)
    }

    /// Whether a header of the stream starts at `at` of the buffer.
    fn stream_header(&self, at: usize) -> bool {
        self.is_stream_header(self.buffer.get(at..).unwrap_or_default())
    }

    /// The next whole frame, or the next run of bytes that is not one, in
    /// `frame`; `false` when the buffer holds neither yet.
    pub(crate) fn next(&mut self, frame: &mut Vec<u8>) -> bool {
        if let Ok(header) = Header::parse(&self.buffer) {
            let len = match header.frame_bytes() {
                Some(len) => Some(len),
                // A free-format frame ends at the next header.
                None => (4..self.buffer.len().min(MAX_FRAME)).find(|&n| self.stream_header(n)),
            };
            return match len {
                Some(len) if len <= self.buffer.len() => self.take(len, frame),
                // Not all of it yet.
                Some(_) => false,
                // A free-format frame longer than any frame: not one.
                None if self.buffer.len() >= MAX_FRAME => self.take(MAX_FRAME, frame),
                None => false,
            };
        }
        // Bytes that are not a frame, up to the stream's next header: one
        // run, however many packets it spans, is one damaged packet. A run
        // that reaches MAX_FRAME without a header is one at that point (the
        // last 3 bytes stay, as they may begin a header).
        let at = match (1..self.buffer.len()).find(|&at| self.stream_header(at)) {
            Some(at) => at,
            None if self.buffer.len() >= MAX_FRAME => self.buffer.len() - 3,
            None => return false,
        };
        if at < MIN_DAMAGED {
            self.buffer.drain(..at);
            return self.next(frame);
        }
        self.take(at, frame)
    }

    /// At the end of the stream: what the buffer still holds, a last frame
    /// cut short (which the decoder decodes as far as it goes) or bytes that
    /// are not one.
    pub(crate) fn rest(&mut self, frame: &mut Vec<u8>) -> bool {
        let len = self.buffer.len();
        let frame_starts = Header::parse(&self.buffer).is_ok();
        if len == 0 || !frame_starts && len < MIN_DAMAGED {
            self.buffer.clear();
            return false;
        }
        self.take(len, frame)
    }

    fn take(&mut self, len: usize, frame: &mut Vec<u8>) -> bool {
        frame.clear();
        frame.extend(self.buffer.drain(..len.min(self.buffer.len())));
        true
    }
}
