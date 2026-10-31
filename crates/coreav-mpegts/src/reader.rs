//! Transport packets from a [`Source`]: the packet size, then each packet
//! in turn, read in large aligned chunks, with resynchronisation after a
//! lost sync byte.

use crate::error::{Warning, warn};
use crate::packet::{PacketSize, SYNC, TS_PACKET};
use crate::{Error, LimitKind, Limits, Source};

/// Packets read from the source at a time.
const CHUNK_PACKETS: u64 = 512;
/// Sync bytes that must repeat at the stride to accept a packet size, or
/// a resynchronisation point (fewer where the file ends first).
const CONFIRM: u64 = 3;

/// Finds the packet size and the offset of the first stored packet within
/// the first `max_resync_bytes` of the file.
pub(crate) fn detect<S: Source>(
    source: &mut S,
    len: u64,
    limits: &Limits,
) -> Result<(PacketSize, u64), Error> {
    let window = len.min(limits.max_resync_bytes.saturating_add(CONFIRM * 204 + 8));
    let mut head = vec![0u8; usize::try_from(window).map_err(|_| Error::NotTransportStream)?];
    source.read_exact_at(0, &mut head)?;
    let search = window.min(limits.max_resync_bytes.max(1));
    for start in 0..search {
        for size in PacketSize::ALL {
            // A candidate is confirmed by [`CONFIRM`] packets. Fewer count
            // only for a file too short to hold them at all: a lone sync
            // byte near the end of a longer file is not a transport
            // stream, and taking it for one would make this reader answer
            // for files no reader recognises.
            let need = CONFIRM * size.stride() + size.prefix() + TS_PACKET as u64;
            if len >= need && start.saturating_add(need) > len {
                continue;
            }
            if synced(&head, start, size, len) {
                return Ok((size, start));
            }
        }
    }
    Err(Error::NotTransportStream)
}

/// Whether stored packets of `size` start at `start` of `data` (which
/// holds the file from offset 0, or from the chunk's start): the sync byte
/// at each of the next [`CONFIRM`] packets that the file holds, at least
/// one.
fn synced(data: &[u8], start: u64, size: PacketSize, file_left: u64) -> bool {
    let stride = size.stride();
    let mut seen = 0u64;
    for k in 0..CONFIRM {
        let at = start + k * stride + size.prefix();
        if at + TS_PACKET as u64 > file_left {
            break;
        }
        match usize::try_from(at).ok().and_then(|at| data.get(at)) {
            Some(&SYNC) => seen += 1,
            Some(_) => return false,
            // Past the bytes at hand but inside the file: not confirmed.
            None => break,
        }
    }
    seen > 0
        && (seen == CONFIRM || start + seen * stride + size.prefix() + TS_PACKET as u64 > file_left)
}

/// Reads transport packets one after the other.
pub(crate) struct Reader {
    size: PacketSize,
    len: u64,
    /// Offset of the next stored packet.
    at: u64,
    /// The bytes of the file at `chunk_start..chunk_start + chunk.len()`.
    chunk: Vec<u8>,
    chunk_start: u64,
    max_resync: u64,
}

impl Reader {
    pub(crate) fn new(size: PacketSize, len: u64, at: u64, limits: &Limits) -> Self {
        Self {
            size,
            len,
            at,
            chunk: Vec::new(),
            chunk_start: 0,
            max_resync: limits.max_resync_bytes,
        }
    }

    /// Offset of the next stored packet.
    pub(crate) fn position(&self) -> u64 {
        self.at
    }

    /// Makes `[offset, offset + want)` available in the chunk, as far as
    /// the file goes; returns the bytes there are.
    fn window<S: Source>(
        &mut self,
        source: &mut S,
        offset: u64,
        want: u64,
    ) -> Result<&[u8], Error> {
        if offset >= self.len {
            return Ok(&[]);
        }
        let end = offset.saturating_add(want).min(self.len);
        let have = offset >= self.chunk_start && end <= self.chunk_start + self.chunk.len() as u64;
        if !have {
            let read = (self.size.stride() * CHUNK_PACKETS).max(want);
            let read_end = offset.saturating_add(read).min(self.len);
            let n = usize::try_from(read_end.saturating_sub(offset))
                .map_err(|_| Error::Io(std::io::ErrorKind::Other))?;
            self.chunk.resize(n, 0);
            source.read_exact_at(offset, &mut self.chunk)?;
            self.chunk_start = offset;
        }
        let from = usize::try_from(offset - self.chunk_start).unwrap_or(usize::MAX);
        let to = usize::try_from(end - self.chunk_start).unwrap_or(usize::MAX);
        Ok(self.chunk.get(from..to).unwrap_or_default())
    }

    /// The next transport packet and the offset of its stored packet;
    /// `None` at the end of the file (a cut-off last packet included).
    pub(crate) fn next<S: Source>(
        &mut self,
        source: &mut S,
        warnings: &mut Vec<Warning>,
    ) -> Result<Option<(u64, &[u8; TS_PACKET])>, Error> {
        let (stride, prefix) = (self.size.stride(), self.size.prefix());
        loop {
            if self.at.saturating_add(prefix + TS_PACKET as u64) > self.len {
                return Ok(None);
            }
            let at = self.at;
            if self.window(source, at + prefix, TS_PACKET as u64)?.first() == Some(&SYNC) {
                self.at = at + stride;
                let from = usize::try_from(at + prefix - self.chunk_start).unwrap_or(usize::MAX);
                let packet = self
                    .chunk
                    .get(from..from.saturating_add(TS_PACKET))
                    .and_then(|bytes| <&[u8; TS_PACKET]>::try_from(bytes).ok())
                    .ok_or(Error::Io(std::io::ErrorKind::UnexpectedEof))?;
                return Ok(Some((at, packet)));
            }
            self.resync(source, warnings)?;
        }
    }

    /// Moves to the next offset where stored packets resume.
    fn resync<S: Source>(
        &mut self,
        source: &mut S,
        warnings: &mut Vec<Warning>,
    ) -> Result<(), Error> {
        let from = self.at;
        let size = self.size;
        let left = self.len.saturating_sub(from);
        let limit = self.max_resync.min(left).max(1);
        // Bytes after a candidate offset that confirm it.
        let need = CONFIRM * size.stride() + size.prefix() + TS_PACKET as u64;
        // The window grows as the search goes on, so that a lost packet
        // costs a few kilobytes, not `max_resync_bytes`; the bytes are
        // searched where they are, never copied.
        let mut want = 16u64 << 10;
        let mut skip = 1u64;
        while skip < limit {
            let span = want.saturating_add(need).min(left);
            let scan_to = if span == left {
                limit
            } else {
                span.saturating_sub(need).min(limit)
            };
            let data = self.window(source, from, span)?;
            if let Some(found) = (skip..scan_to).find(|&k| synced(data, k, size, left)) {
                warn(
                    warnings,
                    Warning::LostSync {
                        offset: from,
                        skipped: found,
                    },
                );
                self.at = from + found;
                return Ok(());
            }
            skip = scan_to.max(skip + 1);
            want = want.saturating_mul(2);
        }
        if self.max_resync >= left {
            // No packet resumes before the end of the file.
            warn(
                warnings,
                Warning::LostSync {
                    offset: from,
                    skipped: left,
                },
            );
            self.at = self.len;
            return Ok(());
        }
        Err(Error::LimitExceeded {
            limit: LimitKind::ResyncBytes,
            offset: from,
        })
    }

    /// Moves to the stored packet at or after `offset` (for the tail read):
    /// the next offset from there where packets are in sync.
    pub(crate) fn seek<S: Source>(&mut self, source: &mut S, offset: u64) -> Result<(), Error> {
        self.at = offset.min(self.len);
        let prefix = self.size.prefix();
        let at = self.at;
        if self.at < self.len && self.window(source, at + prefix, 1)?.first() != Some(&SYNC) {
            // A seek that lands between packets is not a lost packet: no
            // warning.
            self.resync(source, &mut Vec::new())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SliceSource;

    fn stream(size: PacketSize, count: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..count {
            if size == PacketSize::Ts192 {
                out.extend([0, 0, 0, i as u8]);
            }
            let mut p = vec![0x47, 0, i as u8, 0x10];
            p.resize(TS_PACKET, 0xFF);
            out.extend(p);
            if size == PacketSize::Ts204 {
                out.extend([0u8; 16]);
            }
        }
        out
    }

    #[test]
    fn detects_each_packet_size() {
        for size in PacketSize::ALL {
            let data = stream(size, 5);
            let got = detect(
                &mut SliceSource::new(&data),
                data.len() as u64,
                &Limits::default(),
            );
            assert_eq!(got, Ok((size, 0)), "{size:?}");
        }
        // Garbage before the first packet.
        let mut data = vec![1u8; 77];
        data.extend(stream(PacketSize::Ts188, 4));
        let got = detect(
            &mut SliceSource::new(&data),
            data.len() as u64,
            &Limits::default(),
        );
        assert_eq!(got, Ok((PacketSize::Ts188, 77)));
        let junk = vec![0x47u8; 3];
        assert!(
            detect(
                &mut SliceSource::new(&[1u8; 1000]),
                1000,
                &Limits::default()
            )
            .is_err()
        );
        assert!(detect(&mut SliceSource::new(&junk), 3, &Limits::default()).is_err());
    }

    #[test]
    fn resynchronises_after_lost_bytes() {
        let mut data = stream(PacketSize::Ts188, 3);
        data.extend([0u8; 50]);
        data.extend(stream(PacketSize::Ts188, 4));
        let mut source = SliceSource::new(&data);
        let mut reader = Reader::new(PacketSize::Ts188, data.len() as u64, 0, &Limits::default());
        let mut warnings = Vec::new();
        let mut offsets = Vec::new();
        while let Some((at, _)) = reader.next(&mut source, &mut warnings).unwrap() {
            offsets.push(at);
        }
        assert_eq!(offsets, vec![0, 188, 376, 614, 802, 990, 1178]);
        assert_eq!(
            warnings,
            vec![Warning::LostSync {
                offset: 564,
                skipped: 50
            }]
        );
    }
}
