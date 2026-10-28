//! A bounded walk of the source's box tree for the full-file remux
//! (`docs/design/remux.md`): what the parser does not expose (edit lists as
//! stored, track header flags, the raw language code, the children of the
//! first sample entry, the metadata boxes) and every box the output does not
//! carry, for the summary. Box headers only, plus a few small payloads read
//! through fixed buffers and every edit list (bounded by
//! `max_box_bytes`); the boxes visited count against `max_total_boxes`,
//! `max_top_level_boxes` and `max_fragments`, as probe counts them.

use crate::moov::{EditEntry, TrackFlags, overflow};
use coreav_isobmff::{
    BoxHeader, Error, FourCc, LimitKind, Limits, MAX_HEADER_LEN, Parsed, Source, parse_box_header,
};

/// Boxes listed per track and per file outside the tracks; the rest are
/// counted (see [`Report`]).
pub(crate) const MAX_LISTED: usize = 64;

/// The deepest path a reported box can have: `moov` `trak` `mdia` `minf`
/// `stbl` `stsd` entry child.
pub(crate) const MAX_PATH: usize = 8;

/// The path of a box from the top level, without the box's own type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Path {
    kinds: [FourCc; MAX_PATH],
    len: usize,
}

impl Path {
    const EMPTY: Self = Self {
        kinds: [FourCc([0; 4]); MAX_PATH],
        len: 0,
    };

    /// The top level.
    pub(crate) fn top() -> Self {
        Self::EMPTY
    }

    /// This path followed by `kind`; types beyond [`MAX_PATH`] are left out.
    pub(crate) fn with(mut self, kind: FourCc) -> Self {
        if let Some(slot) = self.kinds.get_mut(self.len) {
            *slot = kind;
            self.len = self.len.saturating_add(1);
        }
        self
    }

    /// The types from the top level down to the parent.
    pub(crate) fn kinds(&self) -> &[FourCc] {
        self.kinds.get(..self.len).unwrap_or(&[])
    }
}

/// A box the output does not carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Skipped {
    pub(crate) parent: Path,
    pub(crate) header: BoxHeader,
}

/// Skipped boxes: the first [`MAX_LISTED`] in file order, and how many more.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) listed: Vec<Skipped>,
    pub(crate) more: u64,
}

impl Report {
    fn push(&mut self, parent: Path, header: BoxHeader) {
        if self.listed.len() < MAX_LISTED {
            self.listed.push(Skipped { parent, header });
        } else {
            self.more = self.more.saturating_add(1);
        }
    }
}

/// What the walk found in one `trak`.
#[derive(Clone, Debug, Default)]
pub(crate) struct TrakScan {
    /// Track header fields; `None` without a readable `tkhd`.
    pub(crate) flags: Option<TrackFlags>,
    /// The track header duration as stored, in movie ticks.
    pub(crate) duration: Option<u64>,
    /// The edit list box (the last one, as probe reads it); its entries
    /// are read only for the streams that are kept ([`read_elst`]).
    pub(crate) elst: Option<BoxHeader>,
    /// The media header's language field as stored.
    pub(crate) language: Option<u16>,
    /// `udta` and `meta` children of the `trak`, in order.
    pub(crate) tags: Vec<BoxHeader>,
    /// `stbl` children that hold per-sample information without file
    /// offsets (see [`SAMPLE_INFO`]), in order.
    pub(crate) sample_info: Vec<BoxHeader>,
    /// The first sample entry, and its child boxes in order.
    pub(crate) entry: Option<BoxHeader>,
    pub(crate) entry_children: Vec<BoxHeader>,
    /// The children of a QuickTime sound entry's `wave` box, in order.
    pub(crate) wave_children: Vec<BoxHeader>,
    /// The compressor name and depth of a visual sample entry as stored.
    pub(crate) compressor_name: Option<[u8; 32]>,
    pub(crate) depth: Option<u16>,
    /// Boxes of the track the output does not carry (sample entry children
    /// excluded: which of them are kept depends on the codec).
    pub(crate) skipped: Report,
}

/// What the walk found in the file.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileScan {
    /// The movie box comes before the first media data box.
    pub(crate) moov_first: bool,
    /// `udta` and `meta` children of `moov`, in order.
    pub(crate) tags: Vec<BoxHeader>,
    /// One entry per `trak`, in file order: the parser's `stream_index`.
    pub(crate) traks: Vec<TrakScan>,
    /// Boxes outside the tracks the output does not carry.
    pub(crate) skipped: Report,
    /// Boxes visited, counted against `max_total_boxes`.
    pub(crate) boxes: u64,
}

/// Most `udta` and `meta` boxes remembered per movie or track; more are
/// reported as skipped.
const MAX_TAGS: usize = 8;

/// Top-level boxes rewritten or without content, never reported: the file
/// type, the movie, sample data, fragments, their indexes and padding.
const STRUCTURE: [&[u8; 4]; 11] = [
    b"ftyp", b"moov", b"mdat", b"moof", b"mfra", b"sidx", b"ssix", b"styp", b"free", b"skip",
    b"wide",
];

/// Media information headers and data information, rewritten for the
/// kept tracks.
const MEDIA_HEADERS: [&[u8; 4]; 8] = [
    b"vmhd", b"smhd", b"nmhd", b"sthd", b"hmhd", b"gmhd", b"dinf", b"hdlr",
];

/// Sample table boxes that describe samples by number, never by file
/// offset: valid as stored in any copy that keeps every sample in order
/// (ISO/IEC 14496-12 8.6.4, 8.7.6, 8.7.7, 8.9; QuickTime's `stps`).
pub(crate) const SAMPLE_INFO: [&[u8; 4]; 9] = [
    b"sgpd", b"sbgp", b"sdtp", b"subs", b"cslg", b"stps", b"padb", b"stdp", b"stsh",
];

/// Most sample information boxes remembered per track; more are reported
/// as skipped.
const MAX_SAMPLE_INFO: usize = 16;

/// Sample table boxes the output rebuilds.
const TABLES: [&[u8; 4]; 9] = [
    b"stsd", b"stts", b"ctts", b"stsc", b"stsz", b"stz2", b"stco", b"co64", b"stss",
];

struct Walk<'a, S> {
    source: &'a mut S,
    limits: &'a Limits,
    boxes: u64,
}

impl<S: Source> Walk<'_, S> {
    fn header(&mut self, at: u64, end: u64) -> Result<BoxHeader, Error> {
        let room = end.checked_sub(at).ok_or(Error::Truncated { offset: at })?;
        let mut buf = [0u8; MAX_HEADER_LEN];
        let n = usize::try_from(room.min(MAX_HEADER_LEN as u64)).unwrap_or(MAX_HEADER_LEN);
        let slice = buf.get_mut(..n).ok_or(Error::Truncated { offset: at })?;
        self.source.read_exact_at(at, slice)?;
        let header = match parse_box_header(slice, at, room)? {
            Parsed::Done(header) => header,
            Parsed::NeedMore(_) => return Err(Error::Truncated { offset: at }),
        };
        self.boxes = self.boxes.saturating_add(1);
        if self.boxes > u64::from(self.limits.max_total_boxes) {
            return Err(Error::LimitExceeded {
                limit: LimitKind::TotalBoxes,
                offset: at,
            });
        }
        Ok(header)
    }

    /// Calls `visit` with every child of `parent`, `depth` being the
    /// children's depth (top-level boxes have depth 0). A list may end with
    /// fewer than 8 bytes, such as QuickTime's 32-bit terminator.
    fn children(
        &mut self,
        parent: &BoxHeader,
        skip: u64,
        depth: u32,
        visit: &mut dyn FnMut(&mut Self, BoxHeader) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if depth > self.limits.max_depth {
            return Err(Error::LimitExceeded {
                limit: LimitKind::Depth,
                offset: parent.offset,
            });
        }
        let end = parent.end();
        let mut at = parent.payload_offset().saturating_add(skip);
        while end.saturating_sub(at) >= 8 {
            let header = self.header(at, end)?;
            visit(self, header)?;
            at = header.end();
        }
        Ok(())
    }

    /// The first `buf.len()` payload bytes of `header`, or fewer when the
    /// payload is shorter; returns how many were read.
    fn prefix(&mut self, header: &BoxHeader, buf: &mut [u8]) -> Result<usize, Error> {
        let n = usize::try_from(header.payload_len())
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let slice = buf.get_mut(..n).ok_or_else(overflow)?;
        self.source.read_exact_at(header.payload_offset(), slice)?;
        Ok(n)
    }
}

/// Walks the file. `source` is the file probe accepted, so its structure
/// is known to parse; anything the walk cannot read is an error.
pub(crate) fn scan<S: Source>(source: &mut S, limits: &Limits) -> Result<FileScan, Error> {
    let len = source.len()?;
    let mut walk = Walk {
        source,
        limits,
        boxes: 0,
    };
    let mut file = FileScan::default();
    let mut moov_seen = false;
    let mut mdat_seen = false;
    let (mut top_level, mut fragments) = (0u32, 0u32);
    let mut at = 0u64;
    while len.saturating_sub(at) >= 8 {
        let header = walk.header(at, len)?;
        let kind = &header.kind.0;
        if kind == b"moof" {
            fragments = fragments.saturating_add(1);
            if fragments > limits.max_fragments {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::Fragments,
                    offset: at,
                });
            }
        } else if !(kind == b"mdat" && fragments > 0) {
            top_level = top_level.saturating_add(1);
            if top_level > limits.max_top_level_boxes {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::TopLevelBoxes,
                    offset: at,
                });
            }
        }
        match kind {
            b"moov" if !moov_seen => {
                moov_seen = true;
                file.moov_first = !mdat_seen;
                walk_moov(&mut walk, &header, &mut file)?;
            }
            b"mdat" => mdat_seen = true,
            _ if STRUCTURE.contains(&kind) => {}
            _ => file.skipped.push(Path::EMPTY, header),
        }
        at = header.end();
    }
    file.boxes = walk.boxes;
    Ok(file)
}

fn walk_moov<S: Source>(
    walk: &mut Walk<'_, S>,
    moov: &BoxHeader,
    file: &mut FileScan,
) -> Result<(), Error> {
    let path = Path::EMPTY.with(moov.kind);
    walk.children(moov, 0, 1, &mut |walk, child| {
        match &child.kind.0 {
            b"mvhd" | b"mvex" => {}
            b"trak" => {
                let mut trak = TrakScan::default();
                walk_trak(walk, &child, path, &mut trak)?;
                file.traks
                    .try_reserve(1)
                    .map_err(|_| retained(child.offset))?;
                file.traks.push(trak);
            }
            b"udta" | b"meta" if file.tags.len() < MAX_TAGS => file.tags.push(child),
            _ => file.skipped.push(path, child),
        }
        Ok(())
    })
}

fn walk_trak<S: Source>(
    walk: &mut Walk<'_, S>,
    trak: &BoxHeader,
    parent: Path,
    out: &mut TrakScan,
) -> Result<(), Error> {
    let path = parent.with(trak.kind);
    walk.children(trak, 0, 2, &mut |walk, child| {
        match &child.kind.0 {
            b"tkhd" => (out.flags, out.duration) = tkhd(walk, &child)?,
            b"edts" => {
                let edts = path.with(child.kind);
                walk.children(&child, 0, 3, &mut |_, e| {
                    if e.kind.0 == *b"elst" {
                        // Probe keeps the last edit list; so does the remux.
                        if let Some(earlier) = out.elst.replace(e) {
                            out.skipped.push(edts, earlier);
                        }
                    } else {
                        out.skipped.push(edts, e);
                    }
                    Ok(())
                })?;
            }
            b"mdia" => {
                let mdia = path.with(child.kind);
                // The children of `mdia` may come in any order, as probe
                // accepts them: the handler decides how to read the sample
                // entry, so the media information is walked last.
                let mut handler = None;
                let mut minf = Vec::new();
                walk.children(&child, 0, 3, &mut |walk, m| {
                    match &m.kind.0 {
                        b"mdhd" => out.language = mdhd_language(walk, &m)?,
                        b"hdlr" => {
                            let mut buf = [0u8; 12];
                            if walk.prefix(&m, &mut buf)? == buf.len() {
                                handler = buf.get(8..12).and_then(|h| <[u8; 4]>::try_from(h).ok());
                            }
                        }
                        b"minf" if minf.len() < MAX_TAGS => minf.push(m),
                        _ => out.skipped.push(mdia, m),
                    }
                    Ok(())
                })?;
                for m in &minf {
                    walk_minf(walk, m, mdia, handler, out)?;
                }
            }
            b"udta" | b"meta" if out.tags.len() < MAX_TAGS => out.tags.push(child),
            _ => out.skipped.push(path, child),
        }
        Ok(())
    })
}

fn walk_minf<S: Source>(
    walk: &mut Walk<'_, S>,
    minf: &BoxHeader,
    parent: Path,
    handler: Option<[u8; 4]>,
    out: &mut TrakScan,
) -> Result<(), Error> {
    let path = parent.with(minf.kind);
    walk.children(minf, 0, 4, &mut |walk, child| {
        match &child.kind.0 {
            b"stbl" => {
                let stbl = path.with(child.kind);
                walk.children(&child, 0, 5, &mut |walk, s| {
                    if s.kind.0 == *b"stsd" {
                        stsd(walk, &s, handler, out)?;
                    } else if SAMPLE_INFO.contains(&&s.kind.0)
                        && out.sample_info.len() < MAX_SAMPLE_INFO
                    {
                        out.sample_info.push(s);
                    } else if !TABLES.contains(&&s.kind.0) {
                        out.skipped.push(stbl, s);
                    }
                    Ok(())
                })?;
            }
            kind if MEDIA_HEADERS.contains(&kind) => {}
            _ => out.skipped.push(path, child),
        }
        Ok(())
    })
}

/// The first sample entry and its children. Children start after the
/// fixed fields of a visual entry (ISO/IEC 14496-12 12.1.3) or of an audio
/// entry in its QuickTime version 0, 1 or 2 layout (12.2.3); other
/// handlers' entries are not walked. A child list that does not parse ends
/// the walk there: the parser kept what it could read the same way.
fn stsd<S: Source>(
    walk: &mut Walk<'_, S>,
    stsd: &BoxHeader,
    handler: Option<[u8; 4]>,
    out: &mut TrakScan,
) -> Result<(), Error> {
    // version and flags, entry_count
    let at = stsd.payload_offset().saturating_add(8);
    if stsd.end().saturating_sub(at) < 8 {
        return Ok(());
    }
    let entry = walk.header(at, stsd.end())?;
    out.entry = Some(entry);
    let fixed: u64 = match handler.as_ref() {
        Some(b"vide") => {
            // reserved, data_reference_index, pre_defined and reserved,
            // width, height, resolutions, reserved, frame_count; then the
            // compressor name and the depth.
            let mut buf = [0u8; 42 + 32 + 2];
            if walk.prefix(&entry, &mut buf)? == buf.len() {
                out.compressor_name = buf.get(42..74).and_then(|n| <[u8; 32]>::try_from(n).ok());
                out.depth = buf
                    .get(74..76)
                    .and_then(|d| <[u8; 2]>::try_from(d).ok())
                    .map(u16::from_be_bytes);
            }
            78
        }
        Some(b"soun") => {
            let mut buf = [0u8; 10];
            if walk.prefix(&entry, &mut buf)? < buf.len() {
                return Ok(());
            }
            match buf.get(8..10) {
                Some([0, 1]) => 44,
                Some([0, 2]) => 64,
                _ => 28,
            }
        }
        _ => return Ok(()),
    };
    if entry.payload_len() < fixed {
        return Ok(());
    }
    let mut children = Vec::new();
    let result = walk.children(&entry, fixed, 7, &mut |_, child| {
        if children.len() < MAX_LISTED {
            children.push(child);
        }
        Ok(())
    });
    match result {
        Ok(()) | Err(Error::InvalidBox { .. } | Error::Truncated { .. }) => {}
        Err(error) => return Err(error),
    }
    // QuickTime wraps a sound entry's configuration in `wave`.
    if let Some(wave) = children.iter().find(|c| c.kind.0 == *b"wave").copied() {
        let mut inner = Vec::new();
        let result = walk.children(&wave, 0, 8, &mut |_, child| {
            if inner.len() < MAX_LISTED {
                inner.push(child);
            }
            Ok(())
        });
        match result {
            Ok(()) | Err(Error::InvalidBox { .. } | Error::Truncated { .. }) => {}
            Err(error) => return Err(error),
        }
        out.wave_children = inner;
    }
    out.entry_children = children;
    Ok(())
}

/// Flags, layer, alternate group and volume of a track header, and its
/// duration (ISO/IEC 14496-12 8.3.2); `None` when the box is too short.
fn tkhd<S: Source>(
    walk: &mut Walk<'_, S>,
    header: &BoxHeader,
) -> Result<(Option<TrackFlags>, Option<u64>), Error> {
    // Version 1: version and flags, 32 bytes of times, track_ID, reserved
    // and duration, 8 reserved, then layer, alternate group and volume.
    let mut buf = [0u8; 4 + 32 + 8 + 6];
    let n = walk.prefix(header, &mut buf)?;
    let bytes = buf.get(..n).unwrap_or(&[]);
    let version = bytes.first().copied().unwrap_or(0);
    let flags = be_u32(bytes, 0).map(|f| f & 0x00ff_ffff);
    // After version and flags: times, track_ID, reserved and duration (20
    // or 32 bytes), then 8 reserved bytes.
    let at = if version == 1 { 4 + 32 + 8 } else { 4 + 20 + 8 };
    let field = |i: usize| be_i16(bytes, at + 2 * i);
    let fields = match (flags, field(0), field(1), field(2)) {
        (Some(flags), Some(layer), Some(alternate_group), Some(volume)) => Some(TrackFlags {
            flags,
            layer,
            alternate_group,
            volume,
        }),
        _ => None,
    };
    // After version and flags: creation and modification times, track_ID
    // and a reserved field, then the duration.
    let duration = if version == 1 {
        be_u64(bytes, 4 + 24)
    } else {
        be_u32(bytes, 4 + 16).map(u64::from)
    };
    Ok((fields, duration))
}

/// The media header's 16-bit language field (8.4.2).
fn mdhd_language<S: Source>(
    walk: &mut Walk<'_, S>,
    header: &BoxHeader,
) -> Result<Option<u16>, Error> {
    let mut buf = [0u8; 36];
    let n = walk.prefix(header, &mut buf)?;
    let bytes = buf.get(..n).unwrap_or(&[]);
    let at = if bytes.first() == Some(&1) {
        4 + 28
    } else {
        4 + 16
    };
    Ok(be_i16(bytes, at).map(|v| u16::from_be_bytes(v.to_be_bytes())))
}

/// Bytes charged per edit list entry: the source's entry and the output's
/// (24 bytes each), and its serialized copy in the output's `elst` (at most
/// 20).
pub(crate) const EDIT_BYTES: u64 = 68;

/// Every entry of an edit list as stored (ISO/IEC 14496-12 8.6.6). The
/// payload is bounded by `max_box_bytes`; the entries are charged to
/// `charge` at [`EDIT_BYTES`] each before they are kept.
pub(crate) fn read_elst<S: Source>(
    source: &mut S,
    header: &BoxHeader,
    limits: &Limits,
    charge: &mut dyn FnMut(u64) -> Result<(), Error>,
) -> Result<Vec<EditEntry>, Error> {
    let len = header.payload_len();
    if len > limits.max_box_bytes {
        return Err(Error::LimitExceeded {
            limit: LimitKind::BoxBytes,
            offset: header.offset,
        });
    }
    let size = usize::try_from(len).map_err(|_| overflow())?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(size)
        .map_err(|_| retained(header.offset))?;
    payload.resize(size, 0);
    source.read_exact_at(header.payload_offset(), &mut payload)?;
    let version = payload.first().copied().unwrap_or(0);
    let count = be_u32(&payload, 4).ok_or_else(|| invalid_elst(header))?;
    let width = if version == 1 { 20 } else { 12 };
    let room = payload.len().saturating_sub(8) / width;
    let count = usize::try_from(count)
        .ok()
        .filter(|c| *c <= room)
        .ok_or_else(|| invalid_elst(header))?;
    charge(EDIT_BYTES.saturating_mul(u64::try_from(count).unwrap_or(u64::MAX)))?;
    let mut edits = Vec::new();
    edits
        .try_reserve_exact(count)
        .map_err(|_| retained(header.offset))?;
    for i in 0..count {
        let at = 8usize.saturating_add(i.saturating_mul(width));
        let entry = if version == 1 {
            (|| {
                Some(EditEntry {
                    duration: be_u64(&payload, at)?,
                    media_time: i64::from_be_bytes(be_u64(&payload, at + 8)?.to_be_bytes()),
                    rate: i32::from_be_bytes(be_u32(&payload, at + 16)?.to_be_bytes()),
                })
            })()
        } else {
            (|| {
                Some(EditEntry {
                    duration: u64::from(be_u32(&payload, at)?),
                    media_time: i64::from(i32::from_be_bytes(
                        be_u32(&payload, at + 4)?.to_be_bytes(),
                    )),
                    rate: i32::from_be_bytes(be_u32(&payload, at + 8)?.to_be_bytes()),
                })
            })()
        };
        edits.push(entry.ok_or_else(|| invalid_elst(header))?);
    }
    Ok(edits)
}

fn invalid_elst(header: &BoxHeader) -> Error {
    Error::InvalidBox {
        offset: header.offset,
        kind: header.kind,
        reason: "entry count exceeds the box",
    }
}

fn retained(offset: u64) -> Error {
    Error::LimitExceeded {
        limit: LimitKind::RetainedBytes,
        offset,
    }
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let b: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(b))
}

fn be_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let b: [u8; 8] = bytes.get(at..at.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_be_bytes(b))
}

fn be_i16(bytes: &[u8], at: usize) -> Option<i16> {
    let b: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(i16::from_be_bytes(b))
}
