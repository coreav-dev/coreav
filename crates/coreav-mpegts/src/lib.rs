//! Memory-safe probing and packet listing for MPEG transport streams,
//! written from ISO/IEC 13818-1 and the specifications listed in
//! `docs/design/mpegts.md`.
//!
//! Every entry point takes [`Limits`] and an [`Allowlist`]; malformed or
//! hostile input produces an [`Error`] or a [`Warning`], never a panic or
//! unbounded work. The design, the limits and the rejected alternatives
//! are recorded in `docs/design/mpegts.md`.
//!
//! ```
//! use coreav_mpegts::{probe, Allowlist, Limits, SliceSource};
//!
//! let not_ts = b"\x00\x00\x00\x18ftypisom";
//! let result = probe(SliceSource::new(not_ts), &Limits::default(), &Allowlist::default());
//! assert!(result.is_err());
//! ```

mod codec;
mod demux;
mod error;
mod frames;
mod limits;
mod packet;
mod packets;
mod pes;
mod probe;
mod reader;
mod section;
mod video;

pub use codec::{Allowlist, Codec, StreamKind, arib_caption_profile};
pub use coreav_isobmff::{BufferedSource, FileSource, SliceSource, Source};
pub use error::{Error, LimitKind, Warning};
pub use limits::Limits;
pub use packet::PacketSize;
pub use packets::{Packet, Packets};
pub use probe::{Audio, DroppedStream, Program, Stream, TransportStream, Video, probe};
pub use section::{Descriptor, Service};

/// Whether `source` holds stored transport packets, and where: the packet
/// size and the offset of the first stored packet, which the reader
/// searches for within `max_resync_bytes` of the start of the file, as
/// [`probe`] does. A stream may start after a prefix of other bytes, so
/// this reads more than [`sniff`] and answers for the same files [`probe`]
/// accepts.
///
/// # Errors
///
/// [`Error::NotTransportStream`] when no packet size fits the start of the
/// file, and errors of the source.
pub fn detect<S: Source>(source: &mut S, limits: &Limits) -> Result<(PacketSize, u64), Error> {
    let len = source.len()?;
    reader::detect(source, len, limits)
}

/// Whether `prefix` (the first bytes of a file) starts like a transport
/// stream: the sync byte at the start of three consecutive packets of one
/// of the sizes, at offset 0.
pub fn sniff(prefix: &[u8]) -> bool {
    PacketSize::ALL.iter().any(|size| {
        let stride = usize::try_from(size.stride()).unwrap_or(usize::MAX);
        let prefix_len = usize::try_from(size.prefix()).unwrap_or(0);
        (0..3).all(|k| prefix.get(prefix_len + k * stride) == Some(&0x47))
    })
}

/// Sans-IO entry points for the fuzz targets of `fuzz/` (the PSI sections
/// and the PES reassembly, `docs/design/mpegts.md`); not a stable API.
#[doc(hidden)]
pub mod fuzzing {
    use crate::{frames, pes, section, video};

    /// Feeds `data` to a section assembler as 184-byte packet payloads (a
    /// payload is a unit start when its first byte's low bit is set) and
    /// reads every section it completes as a PAT or a PMT. Both section
    /// bounds are tried: the 1,021 bytes of the tables of ISO/IEC 13818-1
    /// and the 4,093 of an Event Information Table.
    pub fn sections(data: &[u8]) -> usize {
        sections_bounded(data, section::MAX_SECTION)
            + sections_bounded(data, section::MAX_EIT_SECTION)
    }

    fn sections_bounded(data: &[u8], max: usize) -> usize {
        let mut assembler = section::Assembler::new(max);
        let mut out = Vec::new();
        for (i, chunk) in data.chunks(184).enumerate() {
            let start = i == 0 || chunk.first().is_some_and(|b| b & 1 == 1);
            let _ = assembler.push(chunk, start, &mut out);
        }
        let mut read = 0;
        for data in &out {
            if let Ok((long, body)) = section::long(data) {
                read += if long.table_id == section::PAT {
                    section::pat_entries(body).len()
                } else {
                    section::pmt(body).map_or(0, |p| p.streams.len())
                };
            }
        }
        read
    }

    /// Reads `data` as a PES packet: its header, then its payload as
    /// every frame syntax and as video access units.
    pub fn pes(data: &[u8]) -> usize {
        let payload = match pes::parse(data) {
            pes::Parsed::Done(header) => data.get(header.header_len..).unwrap_or_default(),
            _ => data,
        };
        let mut frames_found = 0;
        for syntax in [
            frames::Syntax::Adts,
            frames::Syntax::MpegAudio,
            frames::Syntax::Ac3,
            frames::Syntax::Latm,
            frames::Syntax::Opus,
            frames::Syntax::Dts,
        ] {
            let mut at = 0usize;
            while let Some(frame) = payload
                .get(at..)
                .and_then(|rest| frames::header(syntax, rest))
            {
                frames_found += 1;
                at = at.saturating_add(frame.len.max(1));
            }
        }
        for nal in [video::Nal::H264, video::Nal::Hevc] {
            let mut sets = video::ParameterSets::default();
            sets.add(nal, payload);
            let _ = sets.record(nal);
            let _ = video::keyframe(nal, payload, Some(1));
        }
        let _ = video::mpeg2_size(payload);
        let _ = video::mpeg2_profile(payload);
        frames_found
    }
}
