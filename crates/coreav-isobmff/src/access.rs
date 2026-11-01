//! Random access in the coded data of a frame (`docs/design/random-access.md`):
//! what an H.264 frame's own NAL units say about decoding from it, next to
//! the container's sync sample flag ([`crate::Packet::keyframe`], the `stss`
//! box as stored). Read through a [`Source`] without reading coded data:
//! each NAL unit's length and first bytes (its header, and a slice's
//! header), and the SEI messages, bounded per frame.

use crate::bitstream::{avc_max_num_ref_frames, leb128, rbsp};
use crate::bytes::Bits;
use crate::source::read_head;
use crate::{Codec, Error, Movie, Packet, Packets, Source};

/// Bytes read from the start of each NAL unit: its header, then for a
/// coded slice `first_mb_in_slice` and `slice_type` (ITU-T H.264 7.3.3),
/// two Exp-Golomb codes of at most 63 bits each as read here, with room for
/// emulation prevention.
const SLICE_HEADER_BYTES: u64 = 32;
/// Bytes of the stack buffer the reads of NAL unit lengths, slice headers
/// and OBU headers go to ([`read_head`]).
const STACK_BYTES: usize = SLICE_HEADER_BYTES as usize;
/// Most bytes of one SEI NAL unit read; recovery point messages after this
/// many bytes are not seen.
const MAX_SEI_BYTES: u64 = 4096;
/// `nal_unit_type` of an IDR picture's coded slice (ITU-T H.264 table 7-1).
const NAL_IDR: u8 = 5;
/// `nal_unit_type` of a non-IDR coded slice, and of data partition A, which
/// starts with the slice header.
const NAL_SLICE: u8 = 1;
const NAL_PARTITION_A: u8 = 2;
/// `nal_unit_type` of supplemental enhancement information.
const NAL_SEI: u8 = 6;
/// SEI `payloadType` of a recovery point (ITU-T H.264 D.1.8).
const SEI_RECOVERY_POINT: u32 = 6;

/// How a track's frames carry their NAL units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessFormat {
    /// H.264 NAL units, each after a big-endian length of `length_size`
    /// bytes (ISO/IEC 14496-15 5.3.2).
    H264 {
        /// Bytes of each NAL unit length: 1, 2 or 4.
        length_size: u8,
    },
}

/// What the coded data of one frame says about random access.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameAccess {
    /// The frame holds an IDR picture: a coded slice of `nal_unit_type` 5
    /// (ITU-T H.264 7.4.1.2.4), from which decoding can start.
    pub idr: bool,
    /// The frame has at least one coded slice and every one is an I or SI
    /// slice (`slice_type` 2, 4, 7 or 9, ITU-T H.264 7.4.3): it references
    /// no other picture, though later pictures may still reference
    /// pictures before it unless it is an IDR picture.
    pub intra: bool,
    /// `recovery_frame_cnt` of a recovery point SEI message (ITU-T H.264
    /// D.2.8) in the frame: decoding from here gives correct pictures from
    /// that many frames on.
    pub recovery_frames: Option<u32>,
    /// That recovery point promises exact pictures: `exact_match_flag` 1
    /// (D.2.8). Without it, the pictures from the recovery point on are
    /// only approximately correct. `broken_link_flag` is not read: it
    /// warns about decoding that started at an earlier IDR picture, not
    /// about decoding that starts here.
    pub recovery_exact: bool,
    /// The frame has at least one coded slice and every one is of a
    /// reference picture (`nal_ref_idc` not 0, ITU-T H.264 7.4.1): later
    /// frames may reference it.
    pub reference: bool,
}

impl FrameAccess {
    /// Whether decoding can start at this frame with every frame from it
    /// on in output order decoded correctly, in a stream whose sequence
    /// parameter set keeps `max_num_ref_frames` reference frames
    /// (`docs/design/cut-points.md`): an IDR picture (ITU-T H.264 8.2.1);
    /// an exact recovery point whose `recovery_frame_cnt` is 0 (D.2.8); or an
    /// intra reference picture in a stream that keeps at most one
    /// reference frame, so that no later frame can reference one before
    /// it. Frames decoded after it but shown before it (leading pictures)
    /// are not counted: a cut there does not present them.
    pub fn starts_decoding(&self, max_num_ref_frames: Option<u32>) -> bool {
        self.idr
            || (self.recovery_frames == Some(0) && self.recovery_exact)
            || (self.intra && self.reference && max_num_ref_frames.is_some_and(|refs| refs <= 1))
    }
}

impl Movie {
    /// How the frames of track `index` carry what [`read_frame_access`]
    /// reads: `Some` for an H.264 track whose configuration record is
    /// readable and whose samples are in this file, `None` otherwise.
    pub fn access_format(&self, index: usize) -> Option<AccessFormat> {
        let track = self.tracks.get(index)?;
        let description = &track.description;
        if description.codec != Codec::H264
            || !track.tables_valid
            || track.external_data
            || description.parameter_record_unreadable
        {
            return None;
        }
        let record = description.codec_config.as_ref()?.data.as_slice();
        let length_size = (record.get(4)? & 3) + 1;
        // ISO/IEC 14496-15 5.3.3.1.2: lengthSizeMinusOne 2 is reserved.
        (length_size != 3).then_some(AccessFormat::H264 { length_size })
    }

    /// `max_num_ref_frames` of the first sequence parameter set of an H.264
    /// track's configuration record (ITU-T H.264 7.4.2.1.1): the most
    /// reference frames the decoding process keeps. Parameter sets in the
    /// samples (`avc3`) and other sample descriptions are not read. `None`
    /// when the track is not H.264, or its record or that set cannot be
    /// read.
    pub fn max_num_ref_frames(&self, index: usize) -> Option<u32> {
        let description = &self.tracks.get(index)?.description;
        if description.codec != Codec::H264 || description.parameter_record_unreadable {
            return None;
        }
        avc_max_num_ref_frames(&description.codec_config.as_ref()?.data)
    }
}

impl<S: Source> Packets<'_, S> {
    /// [`read_frame_access`] of `packet`, one of this listing's packets,
    /// through its source and charged to its work budget.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the source fails to read;
    /// [`Error::LimitExceeded`] when the work budget is spent.
    pub fn frame_access(
        &mut self,
        format: AccessFormat,
        packet: &Packet,
    ) -> Result<Option<FrameAccess>, Error> {
        self.frame_access_at(format, packet.offset, packet.size)
    }

    /// [`Packets::frame_access`] of the packet of `size` bytes at `offset`,
    /// one of this listing's packets seen earlier.
    ///
    /// # Errors
    ///
    /// As for [`Packets::frame_access`].
    pub fn frame_access_at(
        &mut self,
        format: AccessFormat,
        offset: u64,
        size: u32,
    ) -> Result<Option<FrameAccess>, Error> {
        let (source, budget) = self.parts_mut();
        read_frame_access(format, source, offset, u64::from(size), &mut || {
            budget.charge(1)
        })
    }

    /// [`read_av1_sync`] of the packet of `size` bytes at `offset`, one of
    /// this listing's packets seen earlier, charged as
    /// [`Packets::frame_access`] is.
    ///
    /// # Errors
    ///
    /// As for [`Packets::frame_access`].
    pub fn av1_sync_at(&mut self, offset: u64, size: u32) -> Result<Option<bool>, Error> {
        let (source, budget) = self.parts_mut();
        read_av1_sync(source, offset, u64::from(size), &mut || budget.charge(1))
    }

    /// [`read_av1_frame_start`] of the packet of `size` bytes at `offset`,
    /// with the sequence header state `reduced` carried by the caller,
    /// charged as [`Packets::frame_access`] is.
    ///
    /// # Errors
    ///
    /// As for [`Packets::frame_access`].
    pub fn av1_frame_start_at(
        &mut self,
        offset: u64,
        size: u32,
        reduced: &mut Option<bool>,
    ) -> Result<Option<Av1FrameStart>, Error> {
        let (source, budget) = self.parts_mut();
        read_av1_frame_start(source, offset, u64::from(size), reduced, &mut || {
            budget.charge(1)
        })
    }

    /// [`read_starts_decoding`] of the packet of `size` bytes at `offset`,
    /// one of this listing's packets seen earlier, charged as
    /// [`Packets::frame_access`] is.
    ///
    /// # Errors
    ///
    /// As for [`Packets::frame_access`].
    pub fn starts_decoding_at(
        &mut self,
        format: AccessFormat,
        offset: u64,
        size: u32,
        max_num_ref_frames: Option<u32>,
    ) -> Result<Option<bool>, Error> {
        let (source, budget) = self.parts_mut();
        read_starts_decoding(
            format,
            source,
            offset,
            u64::from(size),
            max_num_ref_frames,
            &mut || budget.charge(1),
        )
    }
}

/// Reads the NAL unit headers, slice types and SEI messages of one frame of
/// `len` bytes at `offset`. `Ok(None)` when the frame cannot be walked: a
/// NAL unit length past its end, or a slice header that does not parse. A
/// zero length ends the walk: the rest of the frame is padding. `charge`
/// is called once per NAL unit visited, so that the caller's work limit
/// bounds the walk.
///
/// # Errors
///
/// What `source` or `charge` returns.
pub fn read_frame_access<S: Source>(
    format: AccessFormat,
    source: &mut S,
    offset: u64,
    len: u64,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<Option<FrameAccess>, Error> {
    walk(format, source, offset, len, charge, true)
}

/// [`FrameAccess::starts_decoding`] of the frame of `len` bytes at
/// `offset`, read as [`read_frame_access`] reads it, `Ok(None)` when it
/// cannot be walked. SEI messages are read only for a frame without an IDR
/// picture, which they can make a start: an encoder's SEI messages can be
/// far longer than the slice headers, and an IDR picture starts decoding
/// whatever they hold.
///
/// # Errors
///
/// What `source` or `charge` returns.
pub fn read_starts_decoding<S: Source>(
    format: AccessFormat,
    source: &mut S,
    offset: u64,
    len: u64,
    max_num_ref_frames: Option<u32>,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<Option<bool>, Error> {
    let Some(frame) = walk(format, source, offset, len, charge, false)? else {
        return Ok(None);
    };
    if frame.idr {
        return Ok(Some(true));
    }
    Ok(walk(format, source, offset, len, charge, true)?
        .map(|frame| frame.starts_decoding(max_num_ref_frames)))
}

/// AV1 OBU types (AV1 specification 6.2.2).
const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_FRAME_HEADER: u8 = 3;
const OBU_FRAME: u8 = 6;

/// Most OBUs walked in one AV1 sample before its first frame header.
const MAX_OBUS: u32 = 64;

/// Whether the AV1 sample of `len` bytes at `offset` is a sync sample as the
/// AV1 ISOBMFF binding defines one (2.4): it holds a sequence header OBU,
/// and its first frame header, after it, is a key frame shown at once
/// ([`read_av1_frame_start`]). Such a frame refreshes every reference frame
/// (AV1 specification 7.20), so decoding can start there. `Ok(None)` when
/// the OBUs cannot be walked. `charge` is called once per OBU visited.
///
/// # Errors
///
/// What `source` or `charge` returns.
pub fn read_av1_sync<S: Source>(
    source: &mut S,
    offset: u64,
    len: u64,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<Option<bool>, Error> {
    let mut reduced = None;
    Ok(
        read_av1_frame_start(source, offset, len, &mut reduced, charge)?
            .map(|start| start.sequence_header && start.key_frame == Some(true)),
    )
}

/// The first frame of an AV1 sample, as [`read_av1_frame_start`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Av1FrameStart {
    /// The sample holds a sequence header OBU before its first frame
    /// header.
    pub sequence_header: bool,
    /// Whether its first frame header is a key frame shown at once
    /// (`show_existing_frame` 0, `frame_type` KEY_FRAME and `show_frame` 1,
    /// AV1 specification 5.9.2; every frame under a reduced still picture
    /// header is one); `Some(false)` for a sample without a frame header;
    /// `None` when no sequence header is known, so the frame header cannot
    /// be read.
    pub key_frame: Option<bool>,
}

/// The first frame of the AV1 sample of `len` bytes at `offset`.
/// `reduced` is the `reduced_still_picture_header` flag (5.5.1) of the
/// latest sequence header seen, in this sample or an earlier one, `None`
/// before any: a sequence header in the sample updates it, and a frame
/// header is read with it, as a decoder carries the sequence header from
/// one temporal unit to the next (7.5). `Ok(None)` when the OBUs cannot be
/// walked: a forbidden bit set, a size past the sample's end, or more than
/// 64 OBUs before the first frame header. `charge` is called once per OBU
/// visited.
///
/// # Errors
///
/// What `source` or `charge` returns.
pub fn read_av1_frame_start<S: Source>(
    source: &mut S,
    offset: u64,
    len: u64,
    reduced: &mut Option<bool>,
    charge: &mut dyn FnMut() -> Result<(), Error>,
) -> Result<Option<Av1FrameStart>, Error> {
    let end = offset.checked_add(len).ok_or(Error::Overflow { offset })?;
    let mut sequence_header = false;
    let (mut stack, mut heap) = ([0u8; STACK_BYTES], Vec::new());
    let mut at = offset;
    for _ in 0..MAX_OBUS {
        if at >= end {
            break;
        }
        charge()?;
        // The header byte, an optional extension byte, then up to 8 bytes
        // of LEB128 size (5.3.1, 4.10.5).
        let head = read_head(source, at, (end - at).min(10), &mut stack, &mut heap)?;
        let Some(&first) = head.first() else {
            return Ok(None);
        };
        if first & 0x80 != 0 {
            return Ok(None);
        }
        let obu_type = (first >> 3) & 0x0f;
        let mut cursor = 1usize + usize::from(first & 0x04 != 0);
        let size = if first & 0x02 != 0 {
            let Some(size) = leb128(head, &mut cursor) else {
                return Ok(None);
            };
            size
        } else {
            // Without a size field the OBU extends to the sample's end.
            (end - at).saturating_sub(cursor as u64)
        };
        let payload = at.saturating_add(cursor as u64);
        let Some(next) = payload.checked_add(size).filter(|n| *n <= end) else {
            return Ok(None);
        };
        match obu_type {
            OBU_SEQUENCE_HEADER => {
                // seq_profile f(3), still_picture f(1),
                // reduced_still_picture_header f(1).
                let Some(&byte) =
                    read_head(source, payload, size.min(1), &mut stack, &mut heap)?.first()
                else {
                    return Ok(None);
                };
                *reduced = Some(byte & 0x08 != 0);
                sequence_header = true;
            }
            OBU_FRAME_HEADER | OBU_FRAME => {
                let key_frame = match *reduced {
                    None => None,
                    Some(true) => Some(true),
                    Some(false) => {
                        let Some(&byte) =
                            read_head(source, payload, size.min(1), &mut stack, &mut heap)?.first()
                        else {
                            return Ok(None);
                        };
                        // show_existing_frame f(1), frame_type f(2),
                        // show_frame f(1).
                        let show_existing = byte & 0x80 != 0;
                        let key = (byte >> 5) & 0x03 == 0;
                        let shown = byte & 0x10 != 0;
                        Some(!show_existing && key && shown)
                    }
                };
                return Ok(Some(Av1FrameStart {
                    sequence_header,
                    key_frame,
                }));
            }
            _ => {}
        }
        at = next;
    }
    if at < end {
        // More OBUs than walked before a frame header: unknown.
        return Ok(None);
    }
    // No frame header: nothing to start decoding from.
    Ok(Some(Av1FrameStart {
        sequence_header,
        key_frame: Some(false),
    }))
}

/// The `reduced_still_picture_header` flag (AV1 specification 5.5.1) of the
/// last sequence header among the configuration OBUs of an `av1C` box
/// payload (AV1 ISOBMFF binding 2.3.3: four bytes, then the OBUs), or
/// `None` when it holds none or cannot be read.
pub fn av1_config_reduced(av1c: &[u8]) -> Option<bool> {
    let mut source = crate::SliceSource::new(av1c);
    let len = u64::try_from(av1c.len()).ok()?;
    let mut reduced = None;
    read_av1_frame_start(
        &mut source,
        4.min(len),
        len.saturating_sub(4),
        &mut reduced,
        &mut || Ok(()),
    )
    .ok()?;
    reduced
}

/// What the NAL units of one frame seen so far add up to, so that the
/// length-prefixed walk ([`walk`]) and the byte stream walk
/// ([`annex_b_frame_access`]) classify a unit by the same rules. Both
/// read the same first bytes of a unit, its NAL unit header byte among
/// them, and hand on the bytes after the header.
#[derive(Clone, Copy, Debug)]
struct Units {
    access: FrameAccess,
    slices: u64,
    intra: bool,
    reference: bool,
}

impl Units {
    fn new() -> Self {
        Self {
            access: FrameAccess::default(),
            slices: 0,
            intra: true,
            reference: true,
        }
    }

    /// Whether an SEI unit's messages are still wanted: only until the
    /// frame has a recovery point.
    fn wants_sei(&self) -> bool {
        self.access.recovery_frames.is_none()
    }

    /// One coded slice, or data partition A, which starts with the slice
    /// header: `header` its NAL unit header byte and `head` the bytes
    /// after it within the unit's first [`SLICE_HEADER_BYTES`]. `None`
    /// when the slice header does not parse.
    fn slice(&mut self, header: u8, head: &[u8]) -> Option<()> {
        let slice_type = slice_type(head)?;
        self.slices = self.slices.saturating_add(1);
        self.intra &= matches!(slice_type % 5, 2 | 4);
        self.reference &= header & 0x60 != 0;
        self.access.idr |= header & 0x1f == NAL_IDR;
        Some(())
    }

    /// One SEI unit's payload, the NAL unit header byte removed.
    fn sei(&mut self, payload: &[u8]) {
        if let Some((frames, exact)) = recovery_point(payload) {
            self.access.recovery_frames = Some(frames);
            self.access.recovery_exact = exact;
        }
    }

    fn finish(self) -> FrameAccess {
        FrameAccess {
            intra: self.slices > 0 && self.intra,
            reference: self.slices > 0 && self.reference,
            ..self.access
        }
    }
}

/// Start code prefixes of the byte stream format (ITU-T H.264 Annex B):
/// `start_code_prefix_one_3bytes`, with or without the leading
/// `zero_byte`.
const START_CODE: [u8; 3] = [0, 0, 1];

/// [`read_frame_access`] of one frame stored as a byte stream (ITU-T
/// H.264 Annex B), which Matroska holds for an H.264 track written
/// without a configuration record (RFC 9559 codec mappings,
/// `V_MPEG4/ISO/AVC`).
///
/// `frame` holds the frame's bytes from its first start code. `complete`
/// says whether they are all of it: a caller that read a prefix of the
/// frame passes `false`, and a prefix that ends before the frame's first
/// coded slice then gives `None`, since it says nothing about the frame.
/// Where a prefix does hold a slice, `idr` and the recovery point are the
/// frame's (all slices of a picture are IDR slices or none is, 7.4.1.2.4,
/// and a recovery point message precedes them), while `intra` and
/// `reference`, which are claims about every slice, are `false`: a slice
/// past the prefix could contradict them.
///
/// `None` also when `frame` holds no start code, so it is no byte stream,
/// or when a slice header does not parse, which discards what the units
/// before it said, as the length-prefixed walk does.
///
/// Emulation prevention (7.4.1.1) keeps a start code out of a NAL unit's
/// bytes, so the units are exactly the stretches between the start codes,
/// less the `trailing_zero_8bits` after each (B.1.1).
#[must_use]
pub fn annex_b_frame_access(frame: &[u8], complete: bool) -> Option<FrameAccess> {
    let mut units = Units::new();
    let mut at = find_start_code(frame, 0)?;
    loop {
        let next = find_start_code(frame, at);
        // The unit ends where the next start code begins, three bytes
        // before `next`, less the `trailing_zero_8bits` that may follow a
        // NAL unit and the `zero_byte` of a four-byte start code (B.1.1);
        // a NAL unit's last byte is never 0x00 (7.4.1).
        let end = next.map_or(frame.len(), |n| n.saturating_sub(START_CODE.len()));
        let unit = frame.get(at..end).unwrap_or(&[]);
        let kept = unit.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        let unit = unit.get(..kept).unwrap_or(&[]);
        // Two start codes with nothing but zero bytes between them hold no
        // unit.
        if let Some(header) = unit.first() {
            // The unit's first `bytes` bytes, its header among them, as
            // the length-prefixed walk reads them, less the header.
            let head = |bytes: u64| {
                let kept = usize::try_from(bytes).unwrap_or(usize::MAX).min(unit.len());
                unit.get(1..kept).unwrap_or_default()
            };
            match header & 0x1f {
                NAL_SLICE | NAL_PARTITION_A | NAL_IDR => {
                    units.slice(*header, head(SLICE_HEADER_BYTES))?;
                }
                NAL_SEI if units.wants_sei() => units.sei(head(MAX_SEI_BYTES)),
                _ => {}
            }
        }
        match next {
            Some(next) => at = next,
            None => {
                return (complete || units.slices > 0).then(|| {
                    let frame = units.finish();
                    if complete {
                        frame
                    } else {
                        // Claims about every slice are not made of a
                        // prefix: a slice past it could contradict them.
                        FrameAccess {
                            intra: false,
                            reference: false,
                            ..frame
                        }
                    }
                });
            }
        }
    }
}

/// The next start code (`00 00 01`) at or after `from`, as the offset of
/// its first zero byte.
///
/// Eight bytes at a time while none of them is zero, since a start code
/// has two; then three at a time, since a start code ends in a byte of 1,
/// so a byte above 1 rules out every position a code could begin at within
/// those three and a byte of 1 leaves only the first. A byte-at-a-time
/// scan of the elementary stream was the hottest part of the packet
/// listing (43% of its instructions on a 110 MB capture).
pub fn next_start_code(data: &[u8], from: usize) -> Option<usize> {
    // A word has a zero byte when borrowing from every byte leaves a high
    // bit set that the word itself does not have.
    const LOW: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let mut i = from;
    loop {
        while let Some(eight) = data.get(i..i.checked_add(8)?) {
            let Ok(bytes) = <[u8; 8]>::try_from(eight) else {
                break;
            };
            let word = u64::from_le_bytes(bytes);
            if word.wrapping_sub(LOW) & !word & HIGH != 0 {
                break;
            }
            i = i.checked_add(8)?;
        }
        let third = *data.get(i.checked_add(2)?)?;
        if third > 1 {
            i = i.checked_add(3)?;
        } else if third == 1 {
            if data.get(i) == Some(&0) && data.get(i.checked_add(1)?) == Some(&0) {
                return Some(i);
            }
            i = i.checked_add(3)?;
        } else {
            i = i.checked_add(1)?;
        }
    }
}

/// The offset just after the next start code at or after `from`, or
/// `None` when none follows. It is at least `from + 3`, so the unit
/// before it is never read past.
fn find_start_code(frame: &[u8], from: usize) -> Option<usize> {
    next_start_code(frame, from)?.checked_add(START_CODE.len())
}

/// The walk of [`read_frame_access`], reading SEI messages when `sei`.
fn walk<S: Source>(
    format: AccessFormat,
    source: &mut S,
    offset: u64,
    len: u64,
    charge: &mut dyn FnMut() -> Result<(), Error>,
    sei: bool,
) -> Result<Option<FrameAccess>, Error> {
    let AccessFormat::H264 { length_size } = format;
    let end = offset.checked_add(len).ok_or(Error::Overflow { offset })?;
    let length_size = u64::from(length_size);
    let mut units = Units::new();
    let (mut stack, mut heap) = ([0u8; STACK_BYTES], Vec::new());
    let mut at = offset;
    while at < end {
        charge()?;
        let Some(body) = at.checked_add(length_size).filter(|b| *b <= end) else {
            // Fewer bytes than a length field: no NAL unit starts here.
            break;
        };
        let prefix = read_head(source, at, length_size, &mut stack, &mut heap)?;
        let nal_len = prefix.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b));
        if nal_len == 0 {
            // Zero-filled padding reads this way; it holds no NAL unit, and
            // walking it a length field at a time would cost work for none.
            break;
        }
        let Some(next) = body.checked_add(nal_len).filter(|n| *n <= end) else {
            return Ok(None);
        };
        let head = read_head(
            source,
            body,
            nal_len.min(SLICE_HEADER_BYTES),
            &mut stack,
            &mut heap,
        )?;
        let Some(&header) = head.first() else {
            return Ok(None);
        };
        match header & 0x1f {
            NAL_SLICE | NAL_PARTITION_A | NAL_IDR => {
                if units
                    .slice(header, head.get(1..).unwrap_or_default())
                    .is_none()
                {
                    return Ok(None);
                }
            }
            NAL_SEI if sei && units.wants_sei() => {
                let payload = read_head(
                    source,
                    body,
                    nal_len.min(MAX_SEI_BYTES),
                    &mut stack,
                    &mut heap,
                )?;
                units.sei(payload.get(1..).unwrap_or_default());
            }
            _ => {}
        }
        at = next;
    }
    Ok(Some(units.finish()))
}

/// `slice_type` of a slice header (ITU-T H.264 7.3.3), the NAL unit header
/// byte removed: after `first_mb_in_slice`, 0 to 9.
fn slice_type(payload: &[u8]) -> Option<u32> {
    let data = rbsp(payload, usize::try_from(SLICE_HEADER_BYTES).ok()?);
    let mut bits = Bits::new(&data);
    bits.ue()?;
    bits.ue().filter(|t| *t <= 9)
}

/// `recovery_frame_cnt` of the first recovery point message of an SEI
/// payload (ITU-T H.264 7.3.2.3, D.1.8), the NAL unit header byte removed,
/// and whether it is exact (`exact_match_flag` 1);
/// `None` when it holds none, or its messages run past the bytes read.
fn recovery_point(payload: &[u8]) -> Option<(u32, bool)> {
    let data = rbsp(payload, payload.len());
    let mut at = 0usize;
    // More data while the rest is not the RBSP trailing bits (0x80).
    while data.get(at).is_some_and(|b| *b != 0x80) {
        let kind = sei_value(&data, &mut at)?;
        let size = usize::try_from(sei_value(&data, &mut at)?).ok()?;
        let body = data.get(at..at.checked_add(size)?)?;
        if kind == SEI_RECOVERY_POINT {
            let mut bits = Bits::new(body);
            let frames = bits.ue()?;
            return Some((frames, bits.flag()?));
        }
        at = at.checked_add(size)?;
    }
    None
}

/// An SEI `payloadType` or `payloadSize`: bytes of 0xFF, each adding 255,
/// then the last byte.
fn sei_value(data: &[u8], at: &mut usize) -> Option<u32> {
    let mut value = 0u32;
    loop {
        let byte = *data.get(*at)?;
        *at = at.checked_add(1)?;
        value = value.checked_add(u32::from(byte))?;
        if byte != 0xFF {
            return Some(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_codes_are_found_whatever_the_stride_skips() {
        // Against a byte-at-a-time reference, on data with runs of zeros,
        // of bytes of 1 and of bytes above 1, so that each of the scan's
        // three cases is taken at every alignment.
        fn reference(data: &[u8], from: usize) -> Option<usize> {
            (from..data.len().saturating_sub(2)).find(|&i| data[i..i + 3] == [0, 0, 1])
        }
        let mut data = Vec::new();
        let mut state = 12345u64;
        for _ in 0..4000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            data.push(match (state >> 33) % 5 {
                0 | 1 => 0,
                2 => 1,
                other => other as u8 + 40,
            });
        }
        for from in 0..data.len() {
            assert_eq!(
                next_start_code(&data, from),
                reference(&data, from),
                "from {from}"
            );
        }
        assert_eq!(next_start_code(&[0, 0, 1], 0), Some(0));
        assert_eq!(next_start_code(&[0, 0], 0), None);
        assert_eq!(next_start_code(&[], 0), None);
        assert_eq!(next_start_code(&[0, 0, 1], 1), None);
    }
    use crate::SliceSource;

    /// A length-prefixed NAL unit of 4-byte length.
    fn nal(bytes: &[u8]) -> Vec<u8> {
        let mut out = (bytes.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(bytes);
        out
    }

    /// The header byte, then `first_mb_in_slice` 0 (`1`) and `slice_type`
    /// as Exp-Golomb, padded.
    fn slice(kind: u8, slice_type: u32) -> Vec<u8> {
        // ue(v): slice_type + 1 in binary after as many zeros as bits - 1.
        let code = slice_type + 1;
        let width = 32 - code.leading_zeros();
        let mut bits: Vec<u8> = vec![1];
        bits.extend(std::iter::repeat_n(0, (width - 1) as usize));
        bits.extend((0..width).rev().map(|i| ((code >> i) & 1) as u8));
        bits.push(1);
        while bits.len() % 8 != 0 {
            bits.push(0);
        }
        let mut out = vec![0x60 | kind];
        out.extend(
            bits.chunks(8)
                .map(|c| c.iter().fold(0u8, |v, b| (v << 1) | b)),
        );
        out
    }

    fn access(frame: &[u8]) -> Option<FrameAccess> {
        let mut source = SliceSource::new(frame);
        read_frame_access(
            AccessFormat::H264 { length_size: 4 },
            &mut source,
            0,
            frame.len() as u64,
            &mut || Ok(()),
        )
        .unwrap()
    }

    /// An AV1 OBU with a one-byte size field (AV1 specification 5.3).
    fn obu(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![(kind << 3) | 0x02, payload.len() as u8];
        out.extend_from_slice(payload);
        out
    }

    /// A sequence header's first byte: `reduced_still_picture_header`.
    fn sequence(reduced: bool) -> Vec<u8> {
        obu(OBU_SEQUENCE_HEADER, &[if reduced { 0x08 } else { 0x00 }, 0])
    }

    /// A frame OBU whose header starts with `show_existing_frame`,
    /// `frame_type` and `show_frame`.
    fn frame(show_existing: bool, frame_type: u8, shown: bool) -> Vec<u8> {
        let byte = (u8::from(show_existing) << 7) | (frame_type << 5) | (u8::from(shown) << 4);
        obu(OBU_FRAME, &[byte, 0, 0])
    }

    fn start(sample: &[u8], reduced: &mut Option<bool>) -> Option<Av1FrameStart> {
        let len = sample.len() as u64;
        read_av1_frame_start(&mut SliceSource::new(sample), 0, len, reduced, &mut || {
            Ok(())
        })
        .unwrap()
    }

    fn sync(sample: &[u8]) -> Option<bool> {
        let len = sample.len() as u64;
        read_av1_sync(&mut SliceSource::new(sample), 0, len, &mut || Ok(())).unwrap()
    }

    #[test]
    fn av1_key_frames_and_sync_samples() {
        let key = [sequence(false), frame(false, 0, true)].concat();
        let mut reduced = None;
        assert_eq!(
            start(&key, &mut reduced),
            Some(Av1FrameStart {
                sequence_header: true,
                key_frame: Some(true)
            })
        );
        assert_eq!(reduced, Some(false));
        assert_eq!(sync(&key), Some(true));
        // A key frame after the sequence header of an earlier sample: a key
        // frame, but not a sync sample (AV1 ISOBMFF binding 2.4).
        let bare = frame(false, 0, true);
        assert_eq!(
            start(&bare, &mut reduced),
            Some(Av1FrameStart {
                sequence_header: false,
                key_frame: Some(true)
            })
        );
        assert_eq!(sync(&bare), Some(false));
        // Without any sequence header the frame header cannot be read.
        assert_eq!(start(&bare, &mut None).unwrap().key_frame, None);
        // Not shown, shown again, intra-only, inter.
        for other in [
            frame(false, 0, false),
            frame(true, 0, true),
            frame(false, 2, true),
            frame(false, 1, true),
        ] {
            let sample = [sequence(false), other].concat();
            assert_eq!(start(&sample, &mut None).unwrap().key_frame, Some(false));
            assert_eq!(sync(&sample), Some(false));
        }
        // Every frame under a reduced still picture header is a key frame.
        let still = [sequence(true), frame(false, 1, false)].concat();
        assert_eq!(sync(&still), Some(true));
        // No frame header; a size past the end.
        assert_eq!(
            start(&sequence(false), &mut None).unwrap().key_frame,
            Some(false)
        );
        assert_eq!(start(&[0x32, 9, 0], &mut None), None);
    }

    #[test]
    fn av1_configuration_sequence_header() {
        let config = [vec![0x81, 0, 0, 0], sequence(false)].concat();
        assert_eq!(av1_config_reduced(&config), Some(false));
        assert_eq!(av1_config_reduced(&[0x81, 0, 0, 0]), None);
        assert_eq!(av1_config_reduced(&[]), None);
    }

    #[test]
    fn slices_and_idr_pictures() {
        let idr = [nal(&[0x67, 0x42]), nal(&slice(5, 7))].concat();
        assert_eq!(
            access(&idr),
            Some(FrameAccess {
                idr: true,
                intra: true,
                recovery_frames: None,
                recovery_exact: false,
                reference: true,
            })
        );
        // A non-IDR I picture of two slices, then one with a P slice.
        let i = [nal(&slice(1, 2)), nal(&slice(1, 7))].concat();
        assert_eq!(access(&i).map(|a| (a.idr, a.intra)), Some((false, true)));
        let mixed = [nal(&slice(1, 2)), nal(&slice(1, 0))].concat();
        assert_eq!(access(&mixed).map(|a| a.intra), Some(false));
        assert_eq!(access(&nal(&slice(1, 1))).map(|a| a.intra), Some(false));
        assert_eq!(access(&nal(&slice(1, 4))).map(|a| a.intra), Some(true));
        // No slice at all is not intra.
        assert_eq!(access(&nal(&[0x67, 0x42])).map(|a| a.intra), Some(false));
    }

    #[test]
    fn byte_stream_frames() {
        // A start code before each unit, with and without the leading
        // zero byte, as a writer that muxed a byte stream leaves them.
        let sets = [vec![0, 0, 0, 1, 0x67, 0x42], vec![0, 0, 1, 0x68, 0xCE]].concat();
        let idr = [sets.clone(), vec![0, 0, 1], slice(5, 7)].concat();
        assert_eq!(
            annex_b_frame_access(&idr, true),
            Some(FrameAccess {
                idr: true,
                intra: true,
                recovery_frames: None,
                recovery_exact: false,
                reference: true,
            })
        );
        // A picture without its parameter sets: the same P slice the
        // length-prefixed walk reads.
        let p = [vec![0, 0, 0, 1], slice(1, 0)].concat();
        assert_eq!(annex_b_frame_access(&p, true), access(&nal(&slice(1, 0))));
        assert_eq!(
            annex_b_frame_access(&p, true).map(|a| a.starts_decoding(None)),
            Some(false)
        );
        // Bytes with no start code are no byte stream.
        assert_eq!(annex_b_frame_access(&nal(&slice(5, 7)), true), None);
        assert_eq!(annex_b_frame_access(&[], true), None);
        // Zero bytes between two start codes are no NAL unit, and
        // trailing zero bytes after a unit are not part of it.
        let padded = [vec![0, 0, 1, 0, 0, 0, 1], slice(5, 7), vec![0, 0]].concat();
        assert_eq!(
            annex_b_frame_access(&padded, true).map(|a| (a.idr, a.intra)),
            Some((true, true))
        );
        // A frame that ends at a start code.
        assert_eq!(
            annex_b_frame_access(&[0, 0, 1], true).map(|a| a.intra),
            Some(false)
        );
        // A slice header that does not parse, alone and with a unit after
        // it: the start code that follows is no part of the slice, so it
        // cannot complete the header's Exp-Golomb codes.
        let short = [vec![0, 0, 1], vec![0x65]].concat();
        assert_eq!(annex_b_frame_access(&short, true), None);
        let truncated = [0x61, 0x21];
        assert_eq!(
            annex_b_frame_access(&[vec![0, 0, 1], truncated.to_vec()].concat(), true),
            access(&nal(&truncated))
        );
        for tail in [vec![0x67, 0x42], vec![0x06, 0x80], vec![0x09, 0x10]] {
            let frame = [vec![0, 0, 1], truncated.to_vec(), vec![0, 0, 0, 1], tail].concat();
            assert_eq!(annex_b_frame_access(&frame, true), None);
        }
        // A recovery point message makes a frame without an IDR picture a
        // start, as it does in a length-prefixed frame:
        // recovery_frame_cnt 0 (ue 1), exact_match_flag 1,
        // broken_link_flag 0, changing_slice_group_idc 00.
        let exact = [0x06, 6, 1, 0b1100_0000, 0x80];
        let recovery = [vec![0, 0, 0, 1], exact.to_vec(), vec![0, 0, 1], slice(1, 2)].concat();
        assert_eq!(
            annex_b_frame_access(&recovery, true).map(|a| a.starts_decoding(None)),
            Some(true)
        );
        assert_eq!(
            annex_b_frame_access(&recovery, true),
            access(&[nal(&exact), nal(&slice(1, 2))].concat())
        );
        // A prefix of a frame that ends before its first slice says
        // nothing about the frame; one that holds a slice does.
        let long = [
            vec![0, 0, 0, 1],
            vec![0x67, 0x42],
            vec![0, 0, 1],
            slice(5, 7),
        ]
        .concat();
        assert_eq!(annex_b_frame_access(&long[..6], false), None);
        assert_eq!(
            annex_b_frame_access(&long, false).map(|a| a.idr),
            Some(true)
        );
        // Of a prefix, the claims about every slice are not made.
        let i = [vec![0, 0, 0, 1], slice(1, 2)].concat();
        assert_eq!(
            annex_b_frame_access(&i, false).map(|a| (a.intra, a.reference)),
            Some((false, false))
        );
        assert_eq!(
            annex_b_frame_access(&i, true).map(|a| (a.intra, a.reference)),
            Some((true, true))
        );
        // Complete, the same bytes hold a frame with no slice at all.
        assert_eq!(
            annex_b_frame_access(&long[..6], true).map(|a| a.starts_decoding(None)),
            Some(false)
        );
    }

    #[test]
    fn length_sizes_and_partitions() {
        for length_size in [1u8, 2] {
            let frame = [slice(2, 2), slice(1, 7)]
                .iter()
                .flat_map(|n| {
                    let len = (n.len() as u32).to_be_bytes();
                    [&len[4 - usize::from(length_size)..], n.as_slice()].concat()
                })
                .collect::<Vec<u8>>();
            let mut source = SliceSource::new(&frame);
            let access = read_frame_access(
                AccessFormat::H264 { length_size },
                &mut source,
                0,
                frame.len() as u64,
                &mut || Ok(()),
            )
            .unwrap();
            assert_eq!(access.map(|a| (a.idr, a.intra)), Some((false, true)));
        }
        // Partitions B and C carry no slice header and are skipped.
        let frame = [nal(&slice(2, 2)), nal(&[0x63, 0xFF]), nal(&[0x64, 0xFF])].concat();
        assert_eq!(access(&frame).map(|a| a.intra), Some(true));
    }

    #[test]
    fn decodable_starts() {
        let frame = |idr, intra, recovery_frames, reference| FrameAccess {
            idr,
            intra,
            recovery_frames,
            recovery_exact: true,
            reference,
        };
        assert!(frame(true, true, None, true).starts_decoding(Some(4)));
        assert!(frame(false, false, Some(0), true).starts_decoding(Some(4)));
        assert!(!frame(false, false, Some(2), true).starts_decoding(Some(4)));
        // An approximate recovery point is no start.
        let approximate = FrameAccess {
            recovery_exact: false,
            ..frame(false, false, Some(0), true)
        };
        assert!(!approximate.starts_decoding(Some(4)));
        // An intra frame: only with one reference frame, and only when it
        // is itself one.
        assert!(frame(false, true, None, true).starts_decoding(Some(1)));
        assert!(!frame(false, true, None, true).starts_decoding(Some(2)));
        assert!(!frame(false, true, None, true).starts_decoding(None));
        assert!(!frame(false, true, None, false).starts_decoding(Some(1)));
        assert!(!frame(false, false, None, true).starts_decoding(Some(1)));
        // nal_ref_idc 0 on a slice: not a reference picture.
        assert_eq!(access(&nal(&slice(1, 2))).map(|a| a.reference), Some(true));
        let mut unreferenced = slice(1, 2);
        unreferenced[0] &= 0x1f;
        assert_eq!(
            access(&nal(&unreferenced)).map(|a| a.reference),
            Some(false)
        );
    }

    #[test]
    fn recovery_points() {
        // A user data SEI of 2 bytes, then a recovery point of
        // recovery_frame_cnt 3 (ue 00100), exact_match_flag 1 and
        // broken_link_flag 0, then the trailing bits.
        let sei = [0x06, 5, 2, 0xAA, 0xBB, 6, 1, 0b0010_0101, 0x80];
        let frame = [nal(&sei), nal(&slice(1, 0))].concat();
        assert_eq!(
            access(&frame),
            Some(FrameAccess {
                idr: false,
                intra: false,
                recovery_frames: Some(3),
                recovery_exact: true,
                reference: true,
            })
        );
        // A payload size past the bytes: no recovery point, no error.
        let short = [0x06, 6, 9, 0x80];
        assert_eq!(access(&nal(&short)).map(|a| a.recovery_frames), Some(None));
    }

    #[test]
    fn starts_decoding_reads_sei_messages_only_without_an_idr_picture() {
        // `visits` counts the NAL units walked (one charge each).
        let starts = |frame: &[u8], visits: &mut u32| {
            let mut source = SliceSource::new(frame);
            read_starts_decoding(
                AccessFormat::H264 { length_size: 4 },
                &mut source,
                0,
                frame.len() as u64,
                Some(4),
                &mut || {
                    *visits += 1;
                    Ok(())
                },
            )
            .unwrap()
        };
        // A recovery point of recovery_frame_cnt 0 (ue 1) and
        // exact_match_flag 1 before a P slice: a start, found on a second
        // walk that reads the SEI message.
        let sei = [0x06, 6, 1, 0b1100_0000, 0x80];
        let mut visits = 0;
        let frame = [nal(&sei), nal(&slice(1, 0))].concat();
        assert_eq!(starts(&frame, &mut visits), Some(true));
        assert_eq!(visits, 4);
        // An IDR picture is a start on the first walk, its SEI unread.
        let mut visits = 0;
        let frame = [nal(&sei), nal(&slice(5, 7))].concat();
        assert_eq!(starts(&frame, &mut visits), Some(true));
        assert_eq!(visits, 2);
        // A P slice alone is no start; a frame that cannot be walked is
        // unknown.
        assert_eq!(starts(&nal(&slice(1, 0)), &mut 0), Some(false));
        assert_eq!(starts(&[0, 0, 0, 9, 0x65], &mut 0), None);
    }

    #[test]
    fn av1_sync_samples() {
        let sync = |sample: &[u8]| {
            let mut source = SliceSource::new(sample);
            read_av1_sync(&mut source, 0, sample.len() as u64, &mut || Ok(())).unwrap()
        };
        // OBU headers with a size field: temporal delimiter (type 2),
        // sequence header (1) and frame (6), then their sizes.
        let td = [0x12, 0];
        let seq = |reduced: bool| [0x0A, 1, if reduced { 0x08 } else { 0 }];
        // show_existing_frame, frame_type (2 bits), show_frame.
        let frame = |byte: u8| [0x32, 1, byte];
        let key_shown = 0b0001_0000;
        assert_eq!(
            sync(&[&td[..], &seq(false), &frame(key_shown)].concat()),
            Some(true)
        );
        // A key frame without a sequence header before it, one not shown,
        // an inter frame, and an existing frame shown again: no start.
        assert_eq!(sync(&[&td[..], &frame(key_shown)].concat()), Some(false));
        assert_eq!(sync(&[&seq(false)[..], &frame(0)].concat()), Some(false));
        assert_eq!(
            sync(&[&seq(false)[..], &frame(0b0011_0000)].concat()),
            Some(false)
        );
        assert_eq!(sync(&[&seq(false)[..], &frame(0x80)].concat()), Some(false));
        // Under a reduced still picture header every frame is a key frame.
        assert_eq!(
            sync(&[&seq(true)[..], &frame(0b0011_0000)].concat()),
            Some(true)
        );
        // A last OBU without a size field extends to the sample's end.
        assert_eq!(
            sync(&[&seq(false)[..], &[0x30, key_shown, 0xAA]].concat()),
            Some(true)
        );
        // No frame at all: no start.
        assert_eq!(sync(&td), Some(false));
        // More OBUs before the first frame header than are walked: unknown.
        let many = [&td.repeat(64)[..], &seq(false), &frame(key_shown)].concat();
        assert_eq!(sync(&many), None);
        // A forbidden bit set, or a size past the end: unknown.
        assert_eq!(sync(&[0x8A, 1, 0]), None);
        assert_eq!(
            sync(&[&seq(false)[..], &[0x32, 9, key_shown]].concat()),
            None
        );
    }

    #[test]
    fn frames_that_cannot_be_walked() {
        // A length past the frame's end.
        let mut frame = nal(&slice(5, 7));
        frame[3] += 1;
        assert_eq!(access(&frame), None);
        // A slice header that ends early, or a slice_type above 9.
        assert_eq!(access(&nal(&[0x65])), None);
        assert_eq!(access(&nal(&slice(1, 10))), None);
        // Trailing bytes too short for a length are ignored, and a zero
        // length ends the walk: the rest is padding.
        let padded = [nal(&slice(5, 2)), vec![0, 0]].concat();
        assert_eq!(access(&padded).map(|a| a.idr), Some(true));
        let zeros = [nal(&slice(5, 2)), vec![0; 64], nal(&slice(1, 0))].concat();
        assert_eq!(access(&zeros).map(|a| a.intra), Some(true));
    }

    #[test]
    fn every_prefix_and_byte_is_safe() {
        let sei = [0x06, 6, 1, 0b0010_0100, 0x80];
        let frame = [nal(&sei), nal(&slice(5, 7)), nal(&slice(1, 0))].concat();
        let stream = [
            vec![0, 0, 0, 1],
            sei.to_vec(),
            vec![0, 0, 1],
            slice(5, 7),
            vec![0, 0, 1],
            slice(1, 0),
        ]
        .concat();
        for end in 0..frame.len() {
            access(&frame[..end]);
        }
        for end in 0..=stream.len() {
            let _ = annex_b_frame_access(&stream[..end], end == stream.len());
        }
        for i in 0..frame.len() {
            for value in [0x00, 0x03, 0x7F, 0xFF] {
                let mut changed = frame.clone();
                changed[i] = value;
                access(&changed);
            }
        }
        for i in 0..stream.len() {
            for value in [0x00, 0x01, 0x03, 0x7F, 0xFF] {
                let mut changed = stream.clone();
                changed[i] = value;
                let _ = annex_b_frame_access(&changed, i % 2 == 0);
            }
        }
    }
}
