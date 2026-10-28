//! Picture sizes from the frames a container holds (`docs/design/
//! frame-sizes.md`): the parameter sets (H.264, HEVC), sequence headers
//! (AV1) and frame headers (VP8, VP9, ProRes) in each frame, read through a
//! [`Source`] without reading coded data. ISO base media tracks use it
//! through [`crate::Movie::scan_frame_sizes`]; other containers that store
//! the same codecs (Matroska) call [`read_frame`] for each frame.

use crate::bitstream::leb128;
use crate::size::{self, Size, Vp9Size};
use crate::source::read_head;
use crate::{Error, Source};

/// Most bytes of one parameter set read from a frame.
const MAX_PARAMETER_SET: u64 = 4096;
/// Bytes read from the start of a VP8, VP9 or ProRes frame: enough for the
/// fields before the frame size.
const HEADER_BYTES: u64 = 64;
/// Bytes of the stack buffer header reads go to ([`read_head`]): the most
/// any of them reads, [`HEADER_BYTES`] (NAL unit lengths and headers, OBU
/// headers and the VP9 superframe index are shorter).
const STACK_BYTES: usize = HEADER_BYTES as usize;
/// The largest VP9 superframe index: 2 marker bytes and 8 sizes of 4 bytes.
const MAX_SUPERFRAME_INDEX: u64 = 34;
/// Bytes of an Annex B frame read at once while looking for start codes.
const ANNEX_B_CHUNK: u64 = 64 << 10;

/// How a codec's frames carry their picture size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameFormat {
    /// H.264 NAL units, each after a big-endian length of `length_size`
    /// bytes (ISO/IEC 14496-15 5.3.2); sequence parameter sets give the size.
    H264 {
        /// Bytes of each NAL unit length: 1, 2 or 4.
        length_size: u8,
    },
    /// HEVC NAL units, as for H.264 (ISO/IEC 14496-15 8.3.2).
    Hevc {
        /// Bytes of each NAL unit length: 1, 2 or 4.
        length_size: u8,
    },
    /// H.264 NAL units in the byte stream format, each after a start code
    /// (ITU-T H.264 Annex B), as some Matroska writers store H.264 without
    /// a configuration record.
    H264AnnexB,
    /// HEVC NAL units in the byte stream format (ITU-T H.265 Annex B).
    HevcAnnexB,
    /// AV1 OBUs (AV1 5.3); sequence headers give the largest frame size.
    Av1,
    /// VP8 frames (RFC 6386 9.1); key frames give the size.
    Vp8,
    /// VP9 frames and superframes (VP9 6.2 and annex B).
    Vp9,
    /// ProRes frames (SMPTE RDD 36); every frame header gives the size.
    ProRes,
}

impl FrameFormat {
    /// Whether the scan reads only the start of each sample (ProRes and
    /// VP8 frame headers; VP8 is scanned only in Matroska, which gives no
    /// advice), not positions inside it found by walking the sample (NAL
    /// units, OBUs, VP9 superframes).
    pub(crate) fn header_only(self) -> bool {
        matches!(self, Self::Vp8 | Self::ProRes)
    }
}

impl FrameFormat {
    /// The format of H.264 or HEVC frames whose configuration record
    /// (`avcC`, `hvcC`) is `record`: its NAL unit length size. `None` for
    /// other records, or when the record is too short to tell.
    pub fn from_record(kind: &[u8; 4], record: &[u8]) -> Option<Self> {
        match kind {
            b"avcC" => record.get(4).map(|b| Self::H264 {
                length_size: (b & 3) + 1,
            }),
            b"hvcC" => record.get(21).map(|b| Self::Hevc {
                length_size: (b & 3) + 1,
            }),
            _ => None,
        }
    }
}

/// The bound, in each dimension, of the picture sizes the frames read so
/// far define, or the fact that one of them could not be read the way a
/// decoder might read it (then no size is a bound).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSizes {
    bound: Option<Size>,
    unverifiable: bool,
}

impl FrameSizes {
    /// Starts from the size a codec configuration defines, if any.
    pub fn new(configured: Option<Size>) -> Self {
        Self {
            bound: configured,
            unverifiable: false,
        }
    }

    /// Widens the bound to include `size`.
    pub fn add(&mut self, size: Size) {
        self.bound = Some(match self.bound {
            None => size,
            Some((w, h)) => (w.max(size.0), h.max(size.1)),
        });
    }

    /// Records that a frame could not be read: no size is a bound now.
    pub fn mark_unverifiable(&mut self) {
        self.unverifiable = true;
    }

    /// Whether a frame could not be read.
    pub fn is_unverifiable(&self) -> bool {
        self.unverifiable
    }

    /// The bound: `None` when no frame or configuration stated a size, or
    /// when a frame could not be read.
    pub fn bound(&self) -> Option<Size> {
        if self.unverifiable { None } else { self.bound }
    }
}

/// What the parameter sets of a codec configuration record say about the
/// picture size (`frame-sizes.md`, decision 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordSize {
    /// The record holds no parameter set (an `avc3`-style record, an
    /// `av1C` without configuration OBUs, or a record of another kind).
    Absent,
    /// The bound, in each dimension, of the sizes its parameter sets define.
    Bound(Size),
    /// The record cannot be read: a version other than 1 (`avcC`, `hvcC`),
    /// a structure past its end, a parameter set that does not parse. A
    /// decoder may frame the samples differently or apply what this crate
    /// cannot read, so no size read from the frames is a bound either.
    Unreadable,
}

/// The picture size the parameter sets of an `avcC`, `hvcC` or `av1C`
/// record define.
pub fn record_size(kind: &[u8; 4], record: &[u8]) -> RecordSize {
    match size::configured(kind, record) {
        Ok(Some(size)) => RecordSize::Bound(size),
        Ok(None) => RecordSize::Absent,
        Err(()) => RecordSize::Unreadable,
    }
}

/// Reads the size-bearing headers of one frame of `len` bytes at `offset`
/// and adds what they say to `sizes`, or marks it unverifiable when the
/// frame cannot be walked (a length or OBU size past its end, a header that
/// does not parse). `charge` is called once per NAL unit, OBU or frame
/// header visited, so that the caller's work limit bounds the walk.
pub fn read_frame<S: Source>(
    format: FrameFormat,
    source: &mut S,
    offset: u64,
    len: u64,
    sizes: &mut FrameSizes,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<(), Error> {
    let end = offset.checked_add(len).ok_or(Error::Overflow { offset })?;
    let (mut stack, mut heap) = ([0u8; STACK_BYTES], Vec::new());
    match format {
        FrameFormat::H264 { length_size } => {
            nal_sizes(source, false, offset, end, length_size, sizes, charge)
        }
        FrameFormat::Hevc { length_size } => {
            nal_sizes(source, true, offset, end, length_size, sizes, charge)
        }
        FrameFormat::H264AnnexB => annex_b_sizes(source, false, offset, end, sizes, charge),
        FrameFormat::HevcAnnexB => annex_b_sizes(source, true, offset, end, sizes, charge),
        FrameFormat::Av1 => obu_sizes(source, offset, end, sizes, charge),
        FrameFormat::Vp9 => vp9_sizes(source, offset, len, sizes, charge),
        FrameFormat::Vp8 => {
            charge()?;
            let head = read_head(source, offset, len.min(HEADER_BYTES), &mut stack, &mut heap)?;
            match size::vp8_size(head) {
                Some(Some(size)) => sizes.add(size),
                Some(None) => {}
                None => sizes.mark_unverifiable(),
            }
            Ok(())
        }
        FrameFormat::ProRes => {
            charge()?;
            let head = read_head(source, offset, len.min(HEADER_BYTES), &mut stack, &mut heap)?;
            match size::prores_size(head) {
                Some(size) => sizes.add(size),
                None => sizes.mark_unverifiable(),
            }
            Ok(())
        }
    }
}

/// Walks the length-prefixed NAL units of one frame and reads each
/// sequence parameter set (ISO/IEC 14496-15 5.3.2 and 8.3.2).
fn nal_sizes<S: Source>(
    source: &mut S,
    hevc: bool,
    start: u64,
    end: u64,
    length_size: u8,
    sizes: &mut FrameSizes,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<(), Error> {
    let length_size = u64::from(length_size);
    let header = if hevc { 2 } else { 1 };
    let (mut stack, mut heap) = ([0u8; STACK_BYTES], Vec::new());
    let mut at = start;
    while at < end {
        charge()?;
        let Some(body) = at.checked_add(length_size).filter(|b| *b <= end) else {
            // Fewer bytes than a length field: no NAL unit can start here.
            return Ok(());
        };
        let prefix = read_head(source, at, length_size, &mut stack, &mut heap)?;
        let len = prefix.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b));
        if len == 0 {
            // A zero length holds no NAL unit (zero-filled data reads this
            // way); the next length follows.
            at = body;
            continue;
        }
        let Some(next) = body.checked_add(len).filter(|n| *n <= end && len >= header) else {
            sizes.mark_unverifiable();
            return Ok(());
        };
        let first = read_head(source, body, header, &mut stack, &mut heap)?;
        if is_sps(hevc, first) && !read_sps(source, hevc, body, len, sizes, &mut heap)? {
            return Ok(());
        }
        at = next;
    }
    Ok(())
}

/// Whether the NAL unit whose header starts `first` is a sequence parameter
/// set: H.264 `nal_unit_type` 7, HEVC 33.
fn is_sps(hevc: bool, first: &[u8]) -> bool {
    if hevc {
        first.first().is_some_and(|b| (b >> 1) & 0x3f == 33)
    } else {
        first.first().is_some_and(|b| b & 0x1f == 7)
    }
}

/// Reads one sequence parameter set of `len` bytes at `body` into `sizes`;
/// whether it parsed.
fn read_sps<S: Source>(
    source: &mut S,
    hevc: bool,
    body: u64,
    len: u64,
    sizes: &mut FrameSizes,
    heap: &mut Vec<u8>,
) -> Result<bool, Error> {
    let len = usize::try_from(len.min(MAX_PARAMETER_SET))
        .map_err(|_| Error::Overflow { offset: body })?;
    heap.clear();
    source.read_append(body, len, heap)?;
    let size = if hevc {
        size::hevc_sps_size(heap)
    } else {
        size::avc_sps_size(heap)
    };
    match size {
        Some(size) => {
            sizes.add(size);
            Ok(true)
        }
        None => {
            sizes.mark_unverifiable();
            Ok(false)
        }
    }
}

/// Walks the NAL units of one frame in the byte stream format (ITU-T H.264
/// and H.265, Annex B): each starts after a three-byte start code
/// `00 00 01` and runs to the next one, less the zero bytes before it.
/// Emulation prevention keeps the pattern out of NAL units, so every
/// occurrence is a start code, and each sequence parameter set is read.
/// The whole frame is read, in chunks of 64 KiB; each chunk and each NAL
/// unit is charged, and memory does not grow with the number of units.
fn annex_b_sizes<S: Source>(
    source: &mut S,
    hevc: bool,
    start: u64,
    end: u64,
    sizes: &mut FrameSizes,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<(), Error> {
    // The first byte of the NAL unit being read, until a start code ends it.
    let mut unit: Option<u64> = None;
    let mut zeros = 0u64;
    let (mut stack, mut heap) = ([0u8; STACK_BYTES], Vec::new());
    let mut at = start;
    while at < end {
        charge()?;
        let chunk = read_head(
            source,
            at,
            (end - at).min(ANNEX_B_CHUNK),
            &mut stack,
            &mut heap,
        )?;
        for (i, byte) in chunk.iter().enumerate() {
            match byte {
                0 => zeros += 1,
                1 if zeros >= 2 => {
                    let position = at.saturating_add(i as u64);
                    // The zero bytes before a start code belong to neither
                    // unit (trailing_zero_8bits, zero_byte).
                    if let Some(body) = unit.take() {
                        let unit_end = position.saturating_sub(zeros);
                        if !annex_b_unit(source, hevc, body, unit_end, sizes)? {
                            return Ok(());
                        }
                    }
                    charge()?;
                    unit = Some(position.saturating_add(1));
                    zeros = 0;
                }
                _ => zeros = 0,
            }
        }
        if chunk.is_empty() {
            break;
        }
        at = at.saturating_add(chunk.len() as u64);
    }
    match unit {
        Some(body) => annex_b_unit(source, hevc, body, end, sizes).map(|_| ()),
        None => Ok(()),
    }
}

/// Reads the NAL unit from `body` to `end` of a byte stream if it is a
/// sequence parameter set; whether the walk may go on (false once a size
/// cannot be read).
fn annex_b_unit<S: Source>(
    source: &mut S,
    hevc: bool,
    body: u64,
    end: u64,
    sizes: &mut FrameSizes,
) -> Result<bool, Error> {
    let header = if hevc { 2 } else { 1 };
    let len = end.saturating_sub(body);
    if len < header {
        return Ok(true);
    }
    let (mut stack, mut heap) = ([0u8; STACK_BYTES], Vec::new());
    let first = read_head(source, body, header, &mut stack, &mut heap)?;
    if is_sps(hevc, first) {
        return read_sps(source, hevc, body, len, sizes, &mut heap);
    }
    Ok(true)
}

/// Walks the OBUs of one AV1 frame and reads each sequence header (AV1
/// 5.3; an OBU without a size field extends to the end of the frame).
fn obu_sizes<S: Source>(
    source: &mut S,
    start: u64,
    end: u64,
    sizes: &mut FrameSizes,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<(), Error> {
    let (mut stack, mut heap) = ([0u8; STACK_BYTES], Vec::new());
    let mut at = start;
    while at < end {
        charge()?;
        // Header byte, optional extension byte, then up to 8 LEB128 bytes.
        let head = read_head(source, at, (end - at).min(10), &mut stack, &mut heap)?;
        let Some(&first) = head.first() else {
            sizes.mark_unverifiable();
            return Ok(());
        };
        let obu_type = (first >> 3) & 0x0f;
        let mut cursor = 1usize + usize::from(first & 0x04 != 0);
        let size = if first & 0x02 != 0 {
            match leb128(head, &mut cursor) {
                Some(size) => size,
                None => {
                    sizes.mark_unverifiable();
                    return Ok(());
                }
            }
        } else {
            (end - at).saturating_sub(cursor as u64)
        };
        let payload = at.saturating_add(cursor as u64);
        let Some(next) = payload.checked_add(size).filter(|n| *n <= end) else {
            sizes.mark_unverifiable();
            return Ok(());
        };
        if obu_type == 1 {
            let data = read_head(
                source,
                payload,
                size.min(MAX_PARAMETER_SET),
                &mut stack,
                &mut heap,
            )?;
            match size::av1_sequence_size(data) {
                Some(size) => sizes.add(size),
                None => {
                    sizes.mark_unverifiable();
                    return Ok(());
                }
            }
        }
        at = next;
    }
    Ok(())
}

/// Reads the uncompressed header of each frame of one VP9 sample.
fn vp9_sizes<S: Source>(
    source: &mut S,
    start: u64,
    len: u64,
    sizes: &mut FrameSizes,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<(), Error> {
    let tail_len = len.min(MAX_SUPERFRAME_INDEX);
    let (mut stack, mut heap) = ([0u8; STACK_BYTES], Vec::new());
    let tail = read_head(
        source,
        start + len - tail_len,
        tail_len,
        &mut stack,
        &mut heap,
    )?;
    let Some(frames) = size::vp9_frames(len, tail) else {
        sizes.mark_unverifiable();
        return Ok(());
    };
    for (offset, frame_len) in frames {
        charge()?;
        if frame_len == 0 {
            continue;
        }
        let head = read_head(
            source,
            start + offset,
            frame_len.min(HEADER_BYTES),
            &mut stack,
            &mut heap,
        )?;
        match size::vp9_size(head) {
            Some(Vp9Size::Coded(size)) => sizes.add(size),
            Some(Vp9Size::Earlier) => {}
            None => {
                sizes.mark_unverifiable();
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LimitKind, SliceSource};

    fn scan(format: FrameFormat, frames: &[&[u8]]) -> (FrameSizes, u64) {
        let data: Vec<u8> = frames.concat();
        let mut source = SliceSource::new(&data);
        let mut sizes = FrameSizes::new(None);
        let mut work = 0u64;
        let mut at = 0u64;
        for frame in frames {
            let len = frame.len() as u64;
            read_frame(format, &mut source, at, len, &mut sizes, &mut || {
                work += 1;
                Ok(())
            })
            .unwrap();
            at += len;
        }
        (sizes, work)
    }

    #[test]
    fn vp8_key_frames_bound_inter_frames() {
        let small = [0x50, 0x2a, 0x00, 0x9d, 0x01, 0x2a, 0x40, 0x01, 0xf0, 0x00];
        let large = [0x50, 0x2a, 0x00, 0x9d, 0x01, 0x2a, 0x80, 0x07, 0x38, 0x04];
        let inter = [0x51, 0x2a, 0x00, 0xff, 0xff];
        let (sizes, work) = scan(FrameFormat::Vp8, &[&small, &inter, &large, &inter]);
        assert_eq!(sizes.bound(), Some((1920, 1080)));
        assert_eq!(work, 4);
        // A key frame without its start code: no bound.
        let broken = [0x50, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x80, 0x07, 0x38, 0x04];
        let (sizes, _) = scan(FrameFormat::Vp8, &[&small, &broken]);
        assert!(sizes.is_unverifiable());
        assert_eq!(sizes.bound(), None);
    }

    #[test]
    fn nal_walk_skips_other_units_and_stops_past_the_end() {
        // A 2-byte length, a non-SPS NAL unit, then a length past the end.
        let frame = [0x00, 0x02, 0x65, 0x88, 0x00, 0x09, 0x41];
        let (sizes, work) = scan(FrameFormat::H264 { length_size: 2 }, &[&frame]);
        assert!(sizes.is_unverifiable());
        assert_eq!(work, 2);
        // Zero lengths and a short tail are skipped.
        let frame = [0x00, 0x00, 0x00, 0x02, 0x65, 0x88, 0x00];
        let (sizes, _) = scan(FrameFormat::H264 { length_size: 2 }, &[&frame]);
        assert!(!sizes.is_unverifiable());
        assert_eq!(sizes.bound(), None);
    }

    /// An H.264 SPS NAL unit (Baseline, 20 × 15 macroblocks: 320 × 240),
    /// as an encoder writes it.
    const SPS_320X240: [u8; 9] = [0x67, 0x42, 0xc0, 0x15, 0x8c, 0x68, 0x14, 0x1f, 0x20];

    #[test]
    fn annex_b_frames_are_walked_by_start_codes() {
        let slice = [0x65, 0x88, 0x84, 0x21];
        // A 4-byte start code, an SPS, trailing zeros, a 3-byte start code.
        let frame = [&[0, 0, 0, 1][..], &SPS_320X240, &[0, 0], &[0, 0, 1], &slice].concat();
        let (sizes, work) = scan(FrameFormat::H264AnnexB, &[&frame]);
        assert_eq!(sizes.bound(), Some((320, 240)));
        assert_eq!(work, 3, "one chunk and two NAL units");
        // A start code across the 64 KiB chunk boundary.
        let mut long = vec![0x65u8; (ANNEX_B_CHUNK as usize) - 2];
        long.extend([0, 0, 1]);
        long.extend(SPS_320X240);
        let (sizes, _) = scan(FrameFormat::H264AnnexB, &[&long]);
        assert_eq!(sizes.bound(), Some((320, 240)));
        // An SPS that does not parse: no bound.
        let broken = [0, 0, 1, 0x67, 0xff];
        let (sizes, _) = scan(FrameFormat::H264AnnexB, &[&broken]);
        assert!(sizes.is_unverifiable());
        // No start code: nothing to read.
        let (sizes, _) = scan(FrameFormat::H264AnnexB, &[&slice]);
        assert_eq!((sizes.bound(), sizes.is_unverifiable()), (None, false));
    }

    #[test]
    fn configuration_starts_the_bound() {
        let mut sizes = FrameSizes::new(Some((640, 360)));
        sizes.add((320, 720));
        assert_eq!(sizes.bound(), Some((640, 720)));
        assert_eq!(record_size(b"avcC", &[]), RecordSize::Unreadable);
        assert_eq!(record_size(b"vpcC", &[1, 2]), RecordSize::Absent);
        assert_eq!(
            FrameFormat::from_record(b"avcC", &[1, 0x64, 0, 0x28, 0xff]),
            Some(FrameFormat::H264 { length_size: 4 })
        );
        assert_eq!(FrameFormat::from_record(b"hvcC", &[1; 21]), None);
    }

    #[test]
    fn the_charge_stops_the_walk() {
        let frame = [0u8; 64];
        let mut source = SliceSource::new(&frame);
        let mut sizes = FrameSizes::new(None);
        let mut left = 3u32;
        let result = read_frame(
            FrameFormat::H264 { length_size: 1 },
            &mut source,
            0,
            64,
            &mut sizes,
            &mut || {
                left = left.checked_sub(1).ok_or(Error::LimitExceeded {
                    limit: LimitKind::SampleWork,
                    offset: 0,
                })?;
                Ok(())
            },
        );
        assert!(matches!(result, Err(Error::LimitExceeded { .. })));
    }
}
