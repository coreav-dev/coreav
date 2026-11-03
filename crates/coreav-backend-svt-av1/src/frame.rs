//! Bounded AV1 frame metadata and tile-extent validation from sections 5.9 and 6.8.
//!
//! This is not an entropy decoder. Tile contents, decoded pixels, level limits,
//! and motion-vector validity are not certified. Unsupported header branches
//! fail closed before a picture is returned.

use std::fmt;

use crate::{
    obu::{Obu, ObuKind},
    sequence::Sequence,
};

/// The required coded, upscaled and render dimensions in luma samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Expected {
    /// Required width.
    pub width: u32,
    /// Required height.
    pub height: u32,
}

/// Bounded frame-header features not implemented by this metadata reader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unsupported {
    /// Decoder-model schedules.
    DecoderModel,
    /// Frame-ID syntax and reference marking.
    FrameIds,
    /// Multiple or layer-selective operating points.
    OperatingPoints,
    /// Nonzero spatial or temporal layer IDs.
    Layers,
    /// Short reference signaling and its inferred reference assignment.
    ShortReferences,
    /// Nonuniform tile spacing.
    NonuniformTiles,
    /// Segmentation maps or inherited segment quantizers.
    Segmentation,
    /// Nonidentity global motion.
    GlobalMotion,
    /// Super-resolution that changes the coded width.
    SuperResolution,
    /// Redundant headers, tile lists, and other non-frame OBU types.
    ObuKind,
    /// A hidden picture that is not showable.
    UnshowableHiddenFrame,
}

/// A frame syntax, extent, state or resource-bound failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// A required field or tile is missing.
    Truncated,
    /// A syntax value, alignment bit or tile extent is invalid.
    Malformed,
    /// A reference is absent, unshowable or already displayed.
    Reference,
    /// Replacing reference state would lose an undisplayed hidden picture.
    HiddenFrameLost,
    /// The actual dimensions differ from the requested dimensions.
    Geometry,
    /// A header or tile group occurs in the wrong frame state.
    Ordering,
    /// The tile count exceeds the supplied bound.
    TileLimit,
    /// The coded-picture identity counter overflows.
    CountLimit,
    /// A feature is explicitly outside this reader's subset.
    Unsupported(Unsupported),
    /// A prior failure permanently closed this reader.
    Failed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AV1 frame validation failed: {self:?}")
    }
}

impl std::error::Error for Error {}

/// Metadata for a complete coded picture or a validated show-existing header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Picture {
    /// Stable segment-local identity of the underlying coded picture.
    pub identity: u64,
    /// Whether this operation displays a picture.
    pub displayed: bool,
    /// Whether this operation is a shown, independently decodable keyframe.
    pub random_access: bool,
    /// Whether this operation displays an existing reference instead of coding tiles.
    pub show_existing: bool,
    /// The actual coded, upscaled and render dimensions, checked to be identical.
    pub dimensions: Expected,
    /// The coded picture's order hint, with zero when disabled.
    pub order_hint: u32,
}

#[derive(Clone, Copy, Debug)]
struct Reference {
    picture: Picture,
    showable: bool,
    displayed: bool,
    key: bool,
}

#[derive(Clone, Copy, Debug)]
struct Header {
    reference: Reference,
    refresh: u8,
    refs: [Option<Reference>; 8],
    tiles: u32,
    tile_bits: u8,
    tile_size_bytes: u8,
    next_tile: u32,
}

/// A terminal-on-error frame reader with eight inline reference slots.
///
/// Reference state is published only after all declared tiles are present.
/// Each hidden picture must eventually be displayed exactly once; overwriting
/// its last reference or ending the segment first fails validation.
#[derive(Debug)]
pub struct Frames {
    sequence: Sequence,
    expected: Expected,
    max_tiles: u32,
    refs: [Option<Reference>; 8],
    pending: Option<Header>,
    next_identity: u64,
    failed: bool,
}

impl Frames {
    /// Starts fresh reference state for a segment using a checked sequence.
    pub fn new(sequence: Sequence, expected: Expected, max_tiles: u32) -> Result<Self, Error> {
        let syntax = sequence.frame_syntax();
        if syntax.decoder_model {
            return Err(Error::Unsupported(Unsupported::DecoderModel));
        }
        if syntax.frame_ids {
            return Err(Error::Unsupported(Unsupported::FrameIds));
        }
        if !syntax.single_operating_point {
            return Err(Error::Unsupported(Unsupported::OperatingPoints));
        }
        if expected.width == 0
            || expected.height == 0
            || sequence.max_width() != expected.width
            || sequence.max_height() != expected.height
        {
            return Err(Error::Geometry);
        }
        Ok(Self {
            sequence,
            expected,
            max_tiles,
            refs: [None; 8],
            pending: None,
            next_identity: 0,
            failed: false,
        })
    }

    /// Checks a frame, frame-header or tile-group OBU, returning only complete pictures.
    pub fn push(&mut self, obu: Obu<'_>) -> Result<Option<Picture>, Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.read(obu);
        if result.is_err() {
            self.failed = true;
            self.pending = None;
        }
        result
    }

    /// Requires no incomplete frame or undisplayed hidden picture at segment end.
    pub fn finish(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = if self.pending.is_some() {
            Err(Error::Truncated)
        } else if self.refs.iter().flatten().any(|r| !r.displayed) {
            Err(Error::HiddenFrameLost)
        } else {
            Ok(())
        };
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// Returns whether a header still requires tile groups.
    pub fn incomplete(&self) -> bool {
        self.pending.is_some()
    }

    fn read(&mut self, obu: Obu<'_>) -> Result<Option<Picture>, Error> {
        if obu.temporal_id() != 0 || obu.spatial_id() != 0 {
            return Err(Error::Unsupported(Unsupported::Layers));
        }
        if obu.kind() == ObuKind::TileGroup {
            let mut header = self.pending.take().ok_or(Error::Ordering)?;
            tiles(&mut header, obu.payload(), false)?;
            if header.next_tile == header.tiles {
                return self.complete(header).map(Some);
            }
            self.pending = Some(header);
            return Ok(None);
        }
        if !matches!(obu.kind(), ObuKind::Frame | ObuKind::FrameHeader) {
            return Err(Error::Unsupported(Unsupported::ObuKind));
        }
        if self.pending.is_some() {
            return Err(Error::Ordering);
        }
        let mut bits = Bits::new(obu.payload());
        let syntax = self.sequence.frame_syntax();
        if !syntax.reduced && bits.flag()? {
            if obu.kind() == ObuKind::Frame {
                return Err(Error::Malformed);
            }
            let index = bits.read(3)? as usize;
            let mut reference = self
                .refs
                .get(index)
                .copied()
                .flatten()
                .ok_or(Error::Reference)?;
            if !reference.showable || reference.displayed {
                return Err(Error::Reference);
            }
            bits.trailing()?;
            reference.displayed = true;
            if reference.key {
                reference.showable = false;
                self.ensure_retained(&[Some(reference); 8], Some(reference.picture.identity))?;
                self.refs = [Some(reference); 8];
            } else {
                for slot in self.refs.iter_mut().flatten() {
                    if slot.picture.identity == reference.picture.identity {
                        slot.displayed = true;
                    }
                }
            }
            let mut picture = reference.picture;
            picture.displayed = true;
            picture.random_access = false;
            picture.show_existing = true;
            return Ok(Some(picture));
        }
        if self.sequence.still_picture() && self.next_identity != 0 {
            return Err(Error::Malformed);
        }
        let mut header = self.header(&mut bits)?;
        if obu.kind() == ObuKind::FrameHeader {
            bits.trailing()?;
            self.pending = Some(header);
            return Ok(None);
        }
        bits.align()?;
        tiles(&mut header, bits.remaining(), true)?;
        if header.next_tile != header.tiles {
            return Err(Error::Malformed);
        }
        self.complete(header).map(Some)
    }

    fn header(&self, bits: &mut Bits<'_>) -> Result<Header, Error> {
        let s = self.sequence.frame_syntax();
        let kind = if s.reduced { 0 } else { bits.read(2)? };
        let intra = kind == 0 || kind == 2;
        let show = s.reduced || bits.flag()?;
        if self.sequence.still_picture() && (kind != 0 || !show) {
            return Err(Error::Malformed);
        }
        let showable = if show { kind != 0 } else { bits.flag()? };
        if !show && !showable {
            return Err(Error::Unsupported(Unsupported::UnshowableHiddenFrame));
        }
        let resilient = s.reduced || kind == 3 || (kind == 0 && show) || bits.flag()?;
        let disable_cdf = bits.flag()?;
        let screen = if s.screen_tools == 2 {
            bits.flag()?
        } else {
            s.screen_tools != 0
        };
        let integer = if screen {
            if s.integer_mv == 2 {
                bits.flag()?
            } else {
                s.integer_mv != 0
            }
        } else {
            false
        };
        let size_override = if kind == 3 {
            true
        } else {
            !s.reduced && bits.flag()?
        };
        let hint = bits.read(s.order_hint_bits)?;
        if !intra && !resilient {
            bits.read(3)?;
        }
        let refresh = if kind == 3 || (kind == 0 && show) {
            255
        } else {
            bits.read(8)? as u8
        };
        if kind == 2 && refresh == 255 {
            return Err(Error::Malformed);
        }
        let mut refs = self.refs;
        if kind == 0 && show {
            refs = [None; 8];
        }
        if (!intra || refresh != 255) && resilient && s.order_hint_bits != 0 {
            for slot in &mut refs {
                let reference_hint = bits.read(s.order_hint_bits)?;
                if slot.is_some_and(|r| r.picture.order_hint != reference_hint) {
                    *slot = None;
                }
            }
        }
        let mut selected = [None; 7];
        let (width, height, render) = if intra {
            let (w, h) = self.size(bits, size_override)?;
            (w, h, render_size(bits, w, h)?)
        } else {
            if s.order_hint_bits != 0 && bits.flag()? {
                return Err(Error::Unsupported(Unsupported::ShortReferences));
            }
            for slot in &mut selected {
                let index = bits.read(3)? as usize;
                *slot = Some(refs.get(index).copied().flatten().ok_or(Error::Reference)?);
            }
            let mut inherited = None;
            if size_override && !resilient {
                for reference in selected.iter().flatten() {
                    if bits.flag()? {
                        inherited = Some(reference.picture.dimensions);
                        break;
                    }
                }
            }
            if let Some(dimensions) = inherited {
                self.superres(bits)?;
                (dimensions.width, dimensions.height, dimensions)
            } else {
                let (w, h) = self.size(bits, size_override)?;
                (w, h, render_size(bits, w, h)?)
            }
        };
        if (Expected { width, height }) != self.expected || render != self.expected {
            return Err(Error::Geometry);
        }
        let intrabc = intra && screen && bits.flag()?;
        if !intra {
            if !integer {
                bits.flag()?;
            }
            if !bits.flag()? {
                bits.read(2)?;
            }
            bits.flag()?;
            if !resilient && s.reference_mvs {
                bits.flag()?;
            }
        }
        if !s.reduced && !disable_cdf {
            bits.flag()?;
        }
        let (tile_count, tile_bits, tile_size_bytes) =
            tile_info(bits, width, height, s.superblock_128, self.max_tiles)?;
        let base_q = bits.read(8)?;
        let mut zero_deltas = delta_q(bits)? == 0;
        let separate = s.separate_uv_delta_q && bits.flag()?;
        zero_deltas &= delta_q(bits)? == 0;
        zero_deltas &= delta_q(bits)? == 0;
        if separate {
            zero_deltas &= delta_q(bits)? == 0;
            zero_deltas &= delta_q(bits)? == 0;
        }
        if bits.flag()? {
            bits.read(4)?;
            bits.read(4)?;
            if s.separate_uv_delta_q {
                bits.read(4)?;
            }
        }
        if bits.flag()? {
            return Err(Error::Unsupported(Unsupported::Segmentation));
        }
        let delta_present = base_q > 0 && bits.flag()?;
        if delta_present {
            bits.read(2)?;
            if !intrabc && bits.flag()? {
                bits.read(3)?;
            }
        }
        let lossless = base_q == 0 && zero_deltas;
        if !lossless && !intrabc {
            let y0 = bits.read(6)?;
            let y1 = bits.read(6)?;
            if y0 != 0 || y1 != 0 {
                bits.read(12)?;
            }
            bits.read(3)?;
            if bits.flag()? && bits.flag()? {
                for _ in 0..10 {
                    if bits.flag()? {
                        bits.read(7)?;
                    }
                }
            }
        }
        if !lossless && !intrabc && s.cdef {
            bits.read(2)?;
            let count = 1_u32 << bits.read(2)?;
            for _ in 0..count {
                bits.read(12)?;
            }
        }
        if !lossless && !intrabc && s.restoration {
            let y = bits.read(2)? != 0;
            let u = bits.read(2)? != 0;
            let v = bits.read(2)? != 0;
            if y || u || v {
                let shift = bits.flag()?;
                if !s.superblock_128 && shift {
                    bits.flag()?;
                }
                if u || v {
                    bits.flag()?;
                }
            }
        }
        if !lossless {
            bits.flag()?;
        }
        let compound = !intra && bits.flag()?;
        if compound && s.order_hint_bits != 0 && skip_allowed(&selected, hint, s.order_hint_bits) {
            bits.flag()?;
        }
        if !intra && !resilient && s.warped_motion {
            bits.flag()?;
        }
        bits.flag()?;
        if !intra {
            for _ in 0..7 {
                if bits.flag()? {
                    return Err(Error::Unsupported(Unsupported::GlobalMotion));
                }
            }
        }
        Ok(Header {
            reference: Reference {
                picture: Picture {
                    identity: self.next_identity,
                    displayed: show,
                    random_access: kind == 0 && show,
                    show_existing: false,
                    dimensions: self.expected,
                    order_hint: hint,
                },
                showable,
                displayed: show,
                key: kind == 0,
            },
            refresh,
            refs,
            tiles: tile_count,
            tile_bits,
            tile_size_bytes,
            next_tile: 0,
        })
    }

    fn superres(&self, bits: &mut Bits<'_>) -> Result<(), Error> {
        if self.sequence.frame_syntax().superres && bits.flag()? {
            return Err(Error::Unsupported(Unsupported::SuperResolution));
        }
        Ok(())
    }

    fn size(&self, bits: &mut Bits<'_>, override_size: bool) -> Result<(u32, u32), Error> {
        let s = self.sequence.frame_syntax();
        let size = if override_size {
            (bits.read(s.width_bits)? + 1, bits.read(s.height_bits)? + 1)
        } else {
            (self.sequence.max_width(), self.sequence.max_height())
        };
        self.superres(bits)?;
        Ok(size)
    }

    fn ensure_retained(
        &self,
        refs: &[Option<Reference>; 8],
        displayed: Option<u64>,
    ) -> Result<(), Error> {
        for old in self.refs.iter().flatten().filter(|r| !r.displayed) {
            if displayed != Some(old.picture.identity)
                && !refs
                    .iter()
                    .flatten()
                    .any(|r| r.picture.identity == old.picture.identity)
            {
                return Err(Error::HiddenFrameLost);
            }
        }
        Ok(())
    }

    fn complete(&mut self, mut header: Header) -> Result<Picture, Error> {
        let next = self.next_identity.checked_add(1).ok_or(Error::CountLimit)?;
        for (i, slot) in header.refs.iter_mut().enumerate() {
            if header.refresh & (1 << i) != 0 {
                *slot = Some(header.reference);
            }
        }
        self.ensure_retained(&header.refs, None)?;
        if !header.reference.displayed
            && !header
                .refs
                .iter()
                .flatten()
                .any(|r| r.picture.identity == header.reference.picture.identity)
        {
            return Err(Error::HiddenFrameLost);
        }
        self.refs = header.refs;
        self.next_identity = next;
        Ok(header.reference.picture)
    }
}

fn render_size(bits: &mut Bits<'_>, width: u32, height: u32) -> Result<Expected, Error> {
    Ok(if bits.flag()? {
        Expected {
            width: bits.read(16)? + 1,
            height: bits.read(16)? + 1,
        }
    } else {
        Expected { width, height }
    })
}

fn relative(a: u32, b: u32, bits: u8) -> i32 {
    let diff = a as i32 - b as i32;
    let sign = 1_i32 << (bits - 1);
    (diff & (sign - 1)) - (diff & sign)
}

fn skip_allowed(refs: &[Option<Reference>; 7], hint: u32, bits: u8) -> bool {
    let forward = refs
        .iter()
        .flatten()
        .map(|r| r.picture.order_hint)
        .filter(|h| relative(*h, hint, bits) < 0)
        .max_by_key(|h| relative(*h, hint, bits));
    let Some(forward) = forward else {
        return false;
    };
    refs.iter().flatten().any(|r| {
        relative(r.picture.order_hint, hint, bits) > 0
            || relative(r.picture.order_hint, forward, bits) < 0
    })
}

fn delta_q(bits: &mut Bits<'_>) -> Result<u32, Error> {
    if bits.flag()? { bits.read(7) } else { Ok(0) }
}

fn tile_log2(size: u32, target: u32) -> u8 {
    let mut log = 0;
    while (size << log) < target {
        log += 1;
    }
    log
}

fn tile_info(
    bits: &mut Bits<'_>,
    width: u32,
    height: u32,
    large: bool,
    limit: u32,
) -> Result<(u32, u8, u8), Error> {
    let shift = if large { 7 } else { 6 };
    let cols = width.div_ceil(1 << shift);
    let rows = height.div_ceil(1 << shift);
    let min_cols = tile_log2(4096 >> shift, cols);
    let min_tiles = min_cols.max(tile_log2((4096 * 2304) >> (2 * shift), rows * cols));
    let max_cols = tile_log2(1, cols.min(64));
    let max_rows = tile_log2(1, rows.min(64));
    if !bits.flag()? {
        return Err(Error::Unsupported(Unsupported::NonuniformTiles));
    }
    let mut col_log = min_cols;
    while col_log < max_cols && bits.flag()? {
        col_log += 1;
    }
    let mut row_log = min_tiles.saturating_sub(col_log);
    while row_log < max_rows && bits.flag()? {
        row_log += 1;
    }
    let tile_width = cols.div_ceil(1 << col_log);
    let tile_height = rows.div_ceil(1 << row_log);
    if tile_width > 4096 >> shift || tile_width * tile_height > (4096 * 2304) >> (2 * shift) {
        return Err(Error::Malformed);
    }
    let count = cols.div_ceil(tile_width) * rows.div_ceil(tile_height);
    if count > limit {
        return Err(Error::TileLimit);
    }
    let tile_bits = col_log + row_log;
    let size_bytes = if tile_bits != 0 {
        if bits.read(tile_bits)? >= count {
            return Err(Error::Malformed);
        }
        bits.read(2)? as u8 + 1
    } else {
        0
    };
    Ok((count, tile_bits, size_bytes))
}

fn tiles(header: &mut Header, payload: &[u8], frame_obu: bool) -> Result<(), Error> {
    let mut bits = Bits::new(payload);
    let explicit = header.tiles > 1 && bits.flag()?;
    if frame_obu && explicit {
        return Err(Error::Malformed);
    }
    let (start, end) = if explicit {
        (bits.read(header.tile_bits)?, bits.read(header.tile_bits)?)
    } else {
        (0, header.tiles - 1)
    };
    if start != header.next_tile || end < start || end >= header.tiles {
        return Err(Error::Ordering);
    }
    bits.align()?;
    let mut rest = bits.remaining();
    for tile in start..=end {
        if tile == end {
            if rest.is_empty() {
                return Err(Error::Truncated);
            }
        } else {
            let mut size = 0_u64;
            for shift in 0..header.tile_size_bytes {
                let (&byte, tail) = rest.split_first().ok_or(Error::Truncated)?;
                rest = tail;
                size |= u64::from(byte) << (8 * shift);
            }
            let size = usize::try_from(size + 1).map_err(|_| Error::Malformed)?;
            let (_, tail) = rest.split_at_checked(size).ok_or(Error::Truncated)?;
            rest = tail;
        }
    }
    header.next_tile = end + 1;
    Ok(())
}

struct Bits<'a> {
    bytes: &'a [u8],
    current: u8,
    left: u8,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            current: 0,
            left: 0,
        }
    }
    fn read(&mut self, count: u8) -> Result<u32, Error> {
        let mut value = 0;
        for _ in 0..count {
            if self.left == 0 {
                let (&byte, tail) = self.bytes.split_first().ok_or(Error::Truncated)?;
                self.bytes = tail;
                self.current = byte;
                self.left = 8;
            }
            self.left -= 1;
            value = (value << 1) | u32::from((self.current >> self.left) & 1);
        }
        Ok(value)
    }
    fn flag(&mut self) -> Result<bool, Error> {
        Ok(self.read(1)? != 0)
    }
    fn align(&mut self) -> Result<(), Error> {
        while self.left != 0 {
            if self.flag()? {
                return Err(Error::Malformed);
            }
        }
        Ok(())
    }
    fn trailing(&mut self) -> Result<(), Error> {
        if !self.flag()? {
            return Err(Error::Malformed);
        }
        self.align()?;
        if self.bytes.iter().any(|b| *b != 0) {
            return Err(Error::Malformed);
        }
        Ok(())
    }
    fn remaining(&self) -> &'a [u8] {
        self.bytes
    }
}
