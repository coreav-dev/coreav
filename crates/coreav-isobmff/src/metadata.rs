//! The descriptive metadata of a movie, as stored: file type brands,
//! creation times, track flags and references, the first sample entry's
//! vendor, compressor name and child boxes, user data boxes, metadata item
//! lists and Nero chapter lists.
//!
//! Probe ([`crate::probe`]) reads what timing and codecs need and skips
//! these boxes; [`read`] walks the movie box again for them, on request.
//! Nothing is interpreted beyond the box layouts: ISO/IEC 14496-12 (`ftyp`
//! 4.3, `mvhd` 8.2.2, `tkhd` 8.3.2, `tref` 8.3.3, `mdhd` 8.4.2, `hdlr`
//! 8.4.3, `stsd` 8.5.2, `stco`/`co64` 8.7.5, `udta` 8.10.1, `kind` 8.10.4,
//! `meta` 8.11.1), the Apple QuickTime File Format (user data atoms,
//! metadata `keys` and item lists, timecode sample descriptions, the `meta`
//! box without version and flags) and two layouts no cited specification
//! defines, read because tools that list metadata read them: the iTunes
//! item list (`ilst` items holding `data`, `mean` and `name` boxes) and the
//! Nero chapter list (`chpl`). A caller maps the values to names; nothing
//! here decides what a tool prints.
//!
//! Reading is bounded and never fails on malformed data: a box that runs
//! past its parent ends the list it is in, more than
//! [`MAX_METADATA_BOXES`] box headers end the walk, and a payload over the
//! limits is skipped; what was read is kept, and [`Metadata::complete`]
//! is cleared. Payloads are read only for the boxes listed above, each at
//! most [`Limits::max_box_bytes`], all together at most
//! [`Limits::max_retained_bytes`]. A source that fails to read
//! ([`Error::Io`]) is an error, so that a transient read error cannot change
//! what a caller prints. A compressed movie box (`cmov`) is inflated under
//! probe's bounds and read from memory.

use crate::header::{BoxHeader, read_header};
use crate::{Error, FourCc, Limits, SliceSource, Source};

/// Box headers read at most, across the whole walk.
pub const MAX_METADATA_BOXES: u32 = 100_000;

/// Bytes of a `tref` box read at most; a larger one is skipped.
const MAX_TREF_BYTES: u64 = 4096;

/// Bytes of the head of a header box (`mvhd`, `tkhd`, `mdhd`) read: every
/// field this module reports lies within them.
const HEADER_HEAD: u64 = 36;

/// What [`read`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Every top-level file type box (`ftyp`) before the movie box, in file
    /// order.
    pub file_types: Vec<FileType>,
    /// The movie header's creation time (`mvhd`).
    pub movie_created: Option<Timestamp>,
    /// One entry per track box (`trak`) of the movie box, in file order: the
    /// position is the stream index probe gives the track
    /// ([`crate::Track::stream_index`]).
    pub tracks: Vec<TrackMetadata>,
    /// User data atoms and list items, in the order the walk meets them:
    /// those of each track box (its `udta` and `meta` boxes), and those of
    /// the movie box (its `udta` and `meta` boxes), as they come in the file.
    pub items: Vec<Item>,
    /// The first Nero chapter list (`chpl`) of the movie's user data.
    pub nero_chapters: Option<Vec<NeroChapter>>,
    /// How many protection system specific header boxes (`pssh`, ISO/IEC
    /// 23001-7 8.1) the movie box holds.
    pub protection_headers: usize,
    /// The movie box was compressed (`cmov`) and read once inflated.
    pub compressed: bool,
    /// Every box of the walk was read; `false` when malformed data or a
    /// bound stopped it.
    pub complete: bool,
}

/// A file type box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileType {
    /// `major_brand`.
    pub major_brand: FourCc,
    /// `minor_version`.
    pub minor_version: u32,
    /// The `compatible_brands` as stored: every byte after the minor
    /// version, a partial last brand included.
    pub compatible_brands: Vec<u8>,
}

/// A creation time as stored: seconds since 1904-01-01 UTC, 32 bits in a
/// version 0 box, 64 in version 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestamp {
    /// The version of the box that holds it.
    pub version: u8,
    /// The value as stored.
    pub seconds: u64,
}

/// What a track box holds beyond timing and codec.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackMetadata {
    /// `track_ID` of the track header.
    pub track_id: Option<u32>,
    /// The track header's flags (bit 0: enabled).
    pub flags: Option<u32>,
    /// The media header's creation time (`mdhd`).
    pub media_created: Option<Timestamp>,
    /// The media header's language as stored: a packed ISO 639-2/T code
    /// (ISO/IEC 14496-12) or a Macintosh language code below 0x400
    /// (QuickTime).
    pub language: Option<u16>,
    /// The media handler (`mdia/hdlr`): its type.
    pub handler: Option<FourCc>,
    /// The handler box's `pre_defined` field: the QuickTime component type
    /// (`mhlr`), 0 in ISO base media.
    pub handler_component: Option<FourCc>,
    /// The handler box's name field as stored (at most 256 bytes).
    pub handler_name: Vec<u8>,
    /// The track references (`tref`), in order.
    pub references: Vec<TrackReference>,
    /// The `kind` boxes of the track's user data: scheme URI and value,
    /// without their terminating NUL.
    pub kinds: Vec<(Vec<u8>, Vec<u8>)>,
    /// The first sample entry of the sample description box.
    pub sample_entry: Option<SampleEntry>,
    /// The first entry of the chunk offset box (`stco` or `co64`): where
    /// the first sample is.
    pub first_chunk_offset: Option<u64>,
}

/// One type of track reference and the `track_ID`s it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackReference {
    /// Reference type: `chap`, `tmcd`, `hint` and so on.
    pub kind: FourCc,
    /// The referenced `track_ID`s, in order.
    pub track_ids: Vec<u32>,
}

/// The first sample entry of a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleEntry {
    /// Sample entry type.
    pub kind: FourCc,
    /// Payload length the entry declares (its size less its 8-byte header).
    pub declared_len: u64,
    /// `vendor` of a visual or sound entry (QuickTime layout; 0 in ISO base
    /// media).
    pub vendor: Option<[u8; 4]>,
    /// The 32-byte `compressorname` field of a visual entry.
    pub compressor_name: Option<[u8; 32]>,
    /// The fields of a timecode entry (`tmcd`).
    pub timecode: Option<TimecodeEntry>,
    /// The child boxes after the entry's fixed fields (visual, sound and
    /// timecode entries), in order, each with its payload.
    pub children: Vec<EntryBox>,
    /// The child boxes of a QuickTime `wave` child (sound entries).
    pub wave: Vec<EntryBox>,
}

/// A child box of a sample entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryBox {
    /// Box type.
    pub kind: FourCc,
    /// Box payload.
    pub payload: Vec<u8>,
}

/// The fields of a QuickTime timecode sample description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimecodeEntry {
    /// Flags: 1 drop frame, 2 wraps at 24 hours, 4 negative times allowed,
    /// 8 counter.
    pub flags: u32,
    /// Time scale.
    pub timescale: u32,
    /// Frame duration in time scale units.
    pub frame_duration: u32,
    /// Number of frames per second, as stored.
    pub frames: u8,
}

/// One metadata item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Position of the track box the item is in, `None` for the movie's.
    pub track: Option<usize>,
    /// The item.
    pub value: ItemValue,
}

/// A user data atom or an item list entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemValue {
    /// A child of a user data box that is not a list (`meta`, `ilst`), a
    /// chapter list or a `kind` box: its type and payload.
    UserData {
        /// Box type.
        kind: FourCc,
        /// Payload as stored.
        payload: Vec<u8>,
    },
    /// An entry of a metadata item list (`ilst`).
    ListItem {
        /// Where the list is.
        list: ListPlace,
        /// The entry's key.
        key: ListKey,
        /// Its `data` boxes, in order: one for most entries; a cover
        /// picture entry (`covr`) holds one per picture.
        data: Vec<ItemData>,
    },
}

/// Where an item list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListPlace {
    /// Directly in a user data box.
    UserData,
    /// In a `meta` box, with the type of the `meta` box's handler (`mdir`,
    /// `mdta`), `None` when it has none.
    Meta {
        /// The handler type.
        handler: Option<FourCc>,
    },
}

/// The key of an item list entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListKey {
    /// The entry's box type (`©nam`, `trkn` and so on).
    Atom(FourCc),
    /// The entry of the `meta` box's `keys` list its box type numbers
    /// (1-based): the key's namespace and name.
    Key {
        /// Key namespace (`mdta`).
        namespace: FourCc,
        /// Key name as stored.
        name: Vec<u8>,
    },
    /// A free-form entry (`----`): the payloads of its `mean` and `name`
    /// boxes, after their version and flags.
    Free {
        /// The `mean` box's text.
        mean: Option<Vec<u8>>,
        /// The `name` box's text.
        name: Option<Vec<u8>>,
    },
}

/// A `data` box of an item list entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemData {
    /// The type indicator (its first byte, the type set, included).
    pub type_indicator: u32,
    /// The locale indicator.
    pub locale: u32,
    /// The value as stored.
    pub value: Vec<u8>,
    /// Where the value starts in the file; for a compressed movie box
    /// ([`Metadata::compressed`]), in the inflated box.
    pub offset: u64,
}

/// One entry of a Nero chapter list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeroChapter {
    /// Start in units of 100 nanoseconds.
    pub start: u64,
    /// Title as stored.
    pub title: Vec<u8>,
}

/// Reads the metadata of the movie in `source`, bounded by `limits`
/// ([`Limits::max_top_level_boxes`], [`Limits::max_box_bytes`],
/// [`Limits::max_retained_bytes`], and probe's bounds for a compressed
/// movie box).
///
/// # Errors
///
/// [`Error::Io`] when `source` fails to read. Malformed data is never an
/// error: see [`Metadata::complete`].
pub fn read<S: Source>(source: S, limits: &Limits) -> Result<Metadata, Error> {
    let mut source = source;
    let len = source.len()?;
    let mut reader = Reader {
        source,
        limits,
        boxes: 0,
        retained: 0,
        complete: true,
        inflated: false,
    };
    let mut out = Metadata::default();
    let mut at = 0u64;
    let mut top = 0u32;
    while len.saturating_sub(at) >= 8 {
        top = top.saturating_add(1);
        if top > limits.max_top_level_boxes {
            reader.complete = false;
            break;
        }
        let Some(header) = reader.header(at, len)? else {
            break;
        };
        match &header.kind.0 {
            b"ftyp" => {
                if let Some(payload) = reader.payload(&header)? {
                    if let Some(file_type) = file_type(&payload) {
                        out.file_types.push(file_type);
                    }
                }
            }
            b"moov" => {
                reader.movie(&header, &mut out)?;
                break;
            }
            _ => {}
        }
        at = header.end();
    }
    out.complete = reader.complete;
    Ok(out)
}

fn file_type(payload: &[u8]) -> Option<FileType> {
    let major_brand = FourCc(payload.get(..4)?.try_into().ok()?);
    let minor_version = be_u32(payload, 4)?;
    Some(FileType {
        major_brand,
        minor_version,
        compatible_brands: payload.get(8..).unwrap_or_default().to_vec(),
    })
}

struct Reader<'a, S> {
    source: S,
    limits: &'a Limits,
    boxes: u32,
    retained: u64,
    complete: bool,
    /// The walk is in the movie box a compressed movie box held, which may
    /// not hold another.
    inflated: bool,
}

impl<S: Source> Reader<'_, S> {
    /// The box header at `at` within `end`; `None` (and the walk marked
    /// incomplete) when it is malformed or the box budget is spent.
    fn header(&mut self, at: u64, end: u64) -> Result<Option<BoxHeader>, Error> {
        self.boxes = self.boxes.saturating_add(1);
        if self.boxes > MAX_METADATA_BOXES.min(self.limits.max_total_boxes) {
            self.complete = false;
            return Ok(None);
        }
        match read_header(&mut self.source, at, end) {
            Ok(header) => Ok(Some(header)),
            Err(error @ Error::Io(_)) => Err(error),
            Err(_) => {
                self.complete = false;
                Ok(None)
            }
        }
    }

    /// The child box headers of the payload between `start` and `end`, up
    /// to the first malformed one. A 32-bit zero terminator, which QuickTime
    /// allows at the end of a list, is not a box.
    fn children(&mut self, start: u64, end: u64) -> Result<Vec<BoxHeader>, Error> {
        let mut out = Vec::new();
        let mut at = start;
        while end.saturating_sub(at) >= 8 {
            let Some(header) = self.header(at, end)? else {
                break;
            };
            at = header.end();
            out.push(header);
        }
        Ok(out)
    }

    /// Charges `len` kept bytes to the retained bytes; `false` (and the walk
    /// marked incomplete) when that would pass the bound.
    fn charge(&mut self, len: usize) -> bool {
        let retained = self.retained.saturating_add(len as u64);
        if retained > self.limits.max_retained_bytes {
            self.complete = false;
            return false;
        }
        self.retained = retained;
        true
    }

    /// `len` bytes at `at`, charged to the retained bytes; `None` (and the
    /// walk marked incomplete) when over a bound or past the source's end.
    fn bytes(&mut self, at: u64, len: u64) -> Result<Option<Vec<u8>>, Error> {
        let retained = self.retained.saturating_add(len);
        if len > self.limits.max_box_bytes || retained > self.limits.max_retained_bytes {
            self.complete = false;
            return Ok(None);
        }
        let Ok(size) = usize::try_from(len) else {
            self.complete = false;
            return Ok(None);
        };
        let mut data = Vec::new();
        if data.try_reserve_exact(size).is_err() {
            self.complete = false;
            return Ok(None);
        }
        match self.source.read_append(at, size, &mut data) {
            Ok(()) => {
                self.retained = retained;
                Ok(Some(data))
            }
            Err(error @ Error::Io(_)) => Err(error),
            Err(_) => {
                self.complete = false;
                Ok(None)
            }
        }
    }

    fn payload(&mut self, header: &BoxHeader) -> Result<Option<Vec<u8>>, Error> {
        self.bytes(header.payload_offset(), header.payload_len())
    }

    /// At most `max` bytes of the payload of `header`.
    fn head(&mut self, header: &BoxHeader, max: u64) -> Result<Option<Vec<u8>>, Error> {
        self.bytes(header.payload_offset(), header.payload_len().min(max))
    }

    fn movie(&mut self, moov: &BoxHeader, out: &mut Metadata) -> Result<(), Error> {
        for child in self.children(moov.payload_offset(), moov.end())? {
            match &child.kind.0 {
                b"mvhd" => {
                    if let Some(head) = self.head(&child, HEADER_HEAD)? {
                        out.movie_created = created(&head);
                    }
                }
                b"trak" => {
                    let index = out.tracks.len();
                    let track = self.track(&child, index, &mut out.items)?;
                    out.tracks.push(track);
                }
                b"udta" => {
                    let mut kinds = Vec::new();
                    let chapters = self.user_data(&child, None, &mut out.items, &mut kinds)?;
                    if out.nero_chapters.is_none() {
                        out.nero_chapters = chapters;
                    }
                }
                b"meta" => self.meta(&child, None, &mut out.items)?,
                b"pssh" => out.protection_headers = out.protection_headers.saturating_add(1),
                b"cmov" if !self.inflated => self.compressed(&child, out)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Reads the movie box a compressed movie box holds, in memory, with
    /// this walk's counters.
    fn compressed(&mut self, cmov: &BoxHeader, out: &mut Metadata) -> Result<(), Error> {
        let inflated = match crate::movie::inflate_movie_box(&mut self.source, cmov, self.limits) {
            Ok(inflated) => inflated,
            Err(error @ Error::Io(_)) => return Err(error),
            Err(_) => {
                self.complete = false;
                return Ok(());
            }
        };
        let mut inner = Reader {
            source: SliceSource::new(&inflated),
            limits: self.limits,
            boxes: self.boxes,
            retained: self.retained,
            complete: self.complete,
            inflated: true,
        };
        out.compressed = true;
        let end = inflated.len() as u64;
        if let Some(header) = inner.header(0, end)? {
            if header.kind == FourCc(*b"moov") {
                inner.movie(&header, out)?;
            } else {
                inner.complete = false;
            }
        }
        self.boxes = inner.boxes;
        self.retained = inner.retained;
        self.complete = inner.complete;
        Ok(())
    }

    fn track(
        &mut self,
        trak: &BoxHeader,
        index: usize,
        items: &mut Vec<Item>,
    ) -> Result<TrackMetadata, Error> {
        let mut track = TrackMetadata::default();
        for child in self.children(trak.payload_offset(), trak.end())? {
            match &child.kind.0 {
                b"tkhd" => {
                    if let Some(head) = self.head(&child, HEADER_HEAD)? {
                        let id_at = if head.first() == Some(&1) { 20 } else { 12 };
                        track.flags = be_u32(&head, 0).map(|v| v & 0x00ff_ffff);
                        track.track_id = be_u32(&head, id_at);
                    }
                }
                b"tref" if child.payload_len() <= MAX_TREF_BYTES => {
                    for reference in self.children(child.payload_offset(), child.end())? {
                        if let Some(payload) = self.payload(&reference)? {
                            track.references.push(TrackReference {
                                kind: reference.kind,
                                track_ids: payload
                                    .chunks_exact(4)
                                    .filter_map(|c| be_u32(c, 0))
                                    .collect(),
                            });
                        }
                    }
                }
                b"mdia" => self.media(&child, &mut track)?,
                b"udta" => {
                    self.user_data(&child, Some(index), items, &mut track.kinds)?;
                }
                b"meta" => self.meta(&child, Some(index), items)?,
                _ => {}
            }
        }
        Ok(track)
    }

    fn media(&mut self, mdia: &BoxHeader, track: &mut TrackMetadata) -> Result<(), Error> {
        let mut minf = None;
        for child in self.children(mdia.payload_offset(), mdia.end())? {
            match &child.kind.0 {
                b"mdhd" => {
                    if let Some(head) = self.head(&child, HEADER_HEAD)? {
                        track.media_created = created(&head);
                        let at = if head.first() == Some(&1) { 32 } else { 20 };
                        track.language = be_u16(&head, at);
                    }
                }
                b"hdlr" if track.handler.is_none() => {
                    if let Some(head) = self.head(&child, 24 + 256)? {
                        track.handler_component = fourcc(&head, 4);
                        track.handler = fourcc(&head, 8);
                        track.handler_name = head.get(24..).unwrap_or_default().to_vec();
                    }
                }
                b"minf" if minf.is_none() => minf = Some(child),
                _ => {}
            }
        }
        let Some(minf) = minf else {
            return Ok(());
        };
        let stbl = self
            .children(minf.payload_offset(), minf.end())?
            .into_iter()
            .find(|c| c.kind == FourCc(*b"stbl"));
        let Some(stbl) = stbl else {
            return Ok(());
        };
        for child in self.children(stbl.payload_offset(), stbl.end())? {
            match &child.kind.0 {
                b"stsd" if track.sample_entry.is_none() => {
                    if let Some(payload) = self.payload(&child)? {
                        let entry = sample_entry(&payload, track.handler);
                        // The entry's child boxes are copies of the payload,
                        // which is dropped: charge what is kept twice over
                        // (in the entry and in `wave`).
                        let copies = entry.as_ref().map_or(0, |e| {
                            e.children
                                .iter()
                                .chain(e.wave.iter())
                                .map(|c| c.payload.len())
                                .sum::<usize>()
                        });
                        if self.charge(copies.saturating_sub(payload.len())) {
                            track.sample_entry = entry;
                        }
                    }
                }
                b"stco" | b"co64" if track.first_chunk_offset.is_none() => {
                    if let Some(head) = self.head(&child, 16)? {
                        track.first_chunk_offset = match (be_u32(&head, 4), &child.kind.0) {
                            (Some(0), _) => None,
                            (Some(_), b"stco") => be_u32(&head, 8).map(u64::from),
                            (Some(_), _) => be_u64(&head, 8),
                            (None, _) => None,
                        };
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Reads a user data box: its lists and atoms into `items`, its `kind`
    /// boxes into `kinds`; returns its first Nero chapter list.
    fn user_data(
        &mut self,
        udta: &BoxHeader,
        track: Option<usize>,
        items: &mut Vec<Item>,
        kinds: &mut Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<Option<Vec<NeroChapter>>, Error> {
        let mut chapters = None;
        for child in self.children(udta.payload_offset(), udta.end())? {
            match &child.kind.0 {
                b"meta" => self.meta(&child, track, items)?,
                b"ilst" => self.list(&child, track, ListPlace::UserData, &[], items)?,
                b"chpl" => {
                    if let Some(payload) = self.payload(&child)? {
                        if chapters.is_none() {
                            chapters = nero_chapters(&payload);
                        }
                    }
                }
                b"kind" => {
                    if let Some(payload) = self.payload(&child)? {
                        let mut strings = payload.get(4..).unwrap_or_default().split(|b| *b == 0);
                        let scheme = strings.next().unwrap_or_default().to_vec();
                        let value = strings.next().unwrap_or_default().to_vec();
                        kinds.push((scheme, value));
                    }
                }
                _ => {
                    if let Some(payload) = self.payload(&child)? {
                        items.push(Item {
                            track,
                            value: ItemValue::UserData {
                                kind: child.kind,
                                payload,
                            },
                        });
                    }
                }
            }
        }
        Ok(chapters)
    }

    /// Reads a `meta` box: an ISO full box, or QuickTime's without version
    /// and flags (its handler comes first).
    fn meta(
        &mut self,
        meta: &BoxHeader,
        track: Option<usize>,
        items: &mut Vec<Item>,
    ) -> Result<(), Error> {
        let Some(probe) = self.bytes(meta.payload_offset(), meta.payload_len().min(8))? else {
            return Ok(());
        };
        let start = if probe.get(4..8) == Some(b"hdlr".as_slice()) {
            meta.payload_offset()
        } else {
            meta.payload_offset().saturating_add(4)
        };
        let mut handler = None;
        let mut keys: Vec<(FourCc, Vec<u8>)> = Vec::new();
        let mut lists = Vec::new();
        for child in self.children(start, meta.end())? {
            match &child.kind.0 {
                b"hdlr" if handler.is_none() => {
                    if let Some(head) = self.head(&child, 12)? {
                        handler = fourcc(&head, 8);
                    }
                }
                b"keys" if keys.is_empty() => {
                    if let Some(payload) = self.payload(&child)? {
                        keys = key_list(&payload);
                    }
                }
                b"ilst" => lists.push(child),
                _ => {}
            }
        }
        for list in lists {
            self.list(&list, track, ListPlace::Meta { handler }, &keys, items)?;
        }
        Ok(())
    }

    /// Reads the entries of an item list.
    fn list(
        &mut self,
        ilst: &BoxHeader,
        track: Option<usize>,
        place: ListPlace,
        keys: &[(FourCc, Vec<u8>)],
        items: &mut Vec<Item>,
    ) -> Result<(), Error> {
        for entry in self.children(ilst.payload_offset(), ilst.end())? {
            let index = u32::from_be_bytes(entry.kind.0);
            let keyed = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| keys.get(i));
            let (mut mean, mut name, mut data) = (None, None, Vec::new());
            for part in self.children(entry.payload_offset(), entry.end())? {
                match &part.kind.0 {
                    b"data" => {
                        if let Some(payload) = self.payload(&part)? {
                            data.extend(item_data(
                                &payload,
                                part.payload_offset().saturating_add(8),
                            ));
                        }
                    }
                    b"mean" if mean.is_none() => {
                        if let Some(payload) = self.payload(&part)? {
                            mean = Some(payload.get(4..).unwrap_or_default().to_vec());
                        }
                    }
                    b"name" if name.is_none() => {
                        if let Some(payload) = self.payload(&part)? {
                            name = Some(payload.get(4..).unwrap_or_default().to_vec());
                        }
                    }
                    _ => {}
                }
            }
            let key = match keyed {
                Some((namespace, key)) => {
                    // Each entry keeps its own copy of the key's name.
                    if !self.charge(key.len()) {
                        continue;
                    }
                    ListKey::Key {
                        namespace: *namespace,
                        name: key.clone(),
                    }
                }
                None if entry.kind == FourCc(*b"----") => ListKey::Free { mean, name },
                None => ListKey::Atom(entry.kind),
            };
            items.push(Item {
                track,
                value: ItemValue::ListItem {
                    list: place,
                    key,
                    data,
                },
            });
        }
        Ok(())
    }
}

/// The creation time of a `mvhd` or `mdhd` head: version, flags, then the
/// time in 32 or 64 bits.
fn created(head: &[u8]) -> Option<Timestamp> {
    let version = *head.first()?;
    let seconds = if version == 1 {
        be_u64(head, 4)?
    } else {
        u64::from(be_u32(head, 4)?)
    };
    Some(Timestamp { version, seconds })
}

/// The entries of a `keys` box: version and flags, an entry count, then per
/// entry its size, namespace and name.
fn key_list(payload: &[u8]) -> Vec<(FourCc, Vec<u8>)> {
    let mut out = Vec::new();
    let count = be_u32(payload, 4).unwrap_or(0);
    let mut at = 8usize;
    for _ in 0..count {
        let Some(size) = be_u32(payload, at).and_then(|s| usize::try_from(s).ok()) else {
            break;
        };
        let (Some(namespace), Some(end)) =
            (fourcc(payload, at.saturating_add(4)), at.checked_add(size))
        else {
            break;
        };
        let Some(name) = payload.get(at.saturating_add(8)..end) else {
            break;
        };
        if size < 8 {
            break;
        }
        out.push((namespace, name.to_vec()));
        at = end;
    }
    out
}

fn item_data(payload: &[u8], offset: u64) -> Option<ItemData> {
    Some(ItemData {
        type_indicator: be_u32(payload, 0)?,
        locale: be_u32(payload, 4)?,
        value: payload.get(8..).unwrap_or_default().to_vec(),
        offset,
    })
}

/// A Nero chapter list: version and flags, 4 more bytes in version 1, an
/// 8-bit count, then per chapter a 64-bit start and a title of 8-bit
/// length. A list cut short keeps the chapters read.
fn nero_chapters(payload: &[u8]) -> Option<Vec<NeroChapter>> {
    let version = *payload.first()?;
    let mut at = if version == 1 { 8 } else { 4 };
    let count = *payload.get(at)?;
    at += 1;
    let mut out = Vec::new();
    for _ in 0..count {
        let Some(start) = be_u64(payload, at) else {
            break;
        };
        let Some(len) = payload.get(at.saturating_add(8)) else {
            break;
        };
        let from = at.saturating_add(9);
        let Some(title) = payload.get(from..from.saturating_add(usize::from(*len))) else {
            break;
        };
        out.push(NeroChapter {
            start,
            title: title.to_vec(),
        });
        at = from.saturating_add(usize::from(*len));
    }
    Some(out)
}

/// Fixed bytes of a visual sample entry (ISO/IEC 14496-12 12.1.3).
const VISUAL_FIXED: usize = 78;
/// Fixed bytes of a sound sample entry, version 0 (ISO/IEC 14496-12
/// 12.2.3; QuickTime adds 16 bytes in version 1 and 36 in version 2).
const SOUND_FIXED: usize = 28;
/// Fixed bytes of a QuickTime timecode sample description.
const TIMECODE_FIXED: usize = 26;

/// The first entry of a sample description box payload, its layout chosen
/// by the track's handler.
fn sample_entry(stsd: &[u8], handler: Option<FourCc>) -> Option<SampleEntry> {
    let size = be_u32(stsd, 8)?;
    let kind = fourcc(stsd, 12)?;
    let declared_len = u64::from(size).saturating_sub(8);
    let body_end = usize::try_from(size)
        .ok()
        .and_then(|s| s.checked_add(8))
        .map_or(stsd.len(), |e| e.min(stsd.len()));
    let body = stsd.get(16..body_end).unwrap_or_default();
    let handler = handler.map(|h| h.0);
    let mut entry = SampleEntry {
        kind,
        declared_len,
        vendor: None,
        compressor_name: None,
        timecode: None,
        children: Vec::new(),
        wave: Vec::new(),
    };
    let fixed = match (handler.as_ref(), &kind.0) {
        (Some(b"vide"), _) => {
            entry.vendor = body.get(12..16).and_then(|v| v.try_into().ok());
            entry.compressor_name = body.get(42..74).and_then(|v| v.try_into().ok());
            Some(VISUAL_FIXED)
        }
        (Some(b"soun"), _) => {
            entry.vendor = body.get(12..16).and_then(|v| v.try_into().ok());
            Some(match be_u16(body, 8) {
                Some(1) => SOUND_FIXED + 16,
                Some(2) => SOUND_FIXED + 36,
                _ => SOUND_FIXED,
            })
        }
        (_, b"tmcd") => {
            entry.timecode = match (
                be_u32(body, 12),
                be_u32(body, 16),
                be_u32(body, 20),
                body.get(24),
            ) {
                (Some(flags), Some(timescale), Some(frame_duration), Some(frames)) => {
                    Some(TimecodeEntry {
                        flags,
                        timescale,
                        frame_duration,
                        frames: *frames,
                    })
                }
                _ => None,
            };
            Some(TIMECODE_FIXED)
        }
        _ => None,
    };
    if let Some(rest) = fixed.and_then(|f| body.get(f..)) {
        entry.children = boxes_in(rest);
        if let Some(wave) = entry.children.iter().find(|c| c.kind == FourCc(*b"wave")) {
            entry.wave = boxes_in(&wave.payload);
        }
    }
    Some(entry)
}

/// The boxes of a byte run, up to the first malformed one or a terminator.
fn boxes_in(data: &[u8]) -> Vec<EntryBox> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let (Some(size), Some(kind)) = (be_u32(data, at), fourcc(data, at.saturating_add(4))) {
        let Some(end) = usize::try_from(size).ok().and_then(|s| at.checked_add(s)) else {
            break;
        };
        if size < 8 {
            break;
        }
        let Some(payload) = data.get(at.saturating_add(8)..end) else {
            break;
        };
        out.push(EntryBox {
            kind,
            payload: payload.to_vec(),
        });
        at = end;
    }
    out
}

/// The `DecoderSpecificInfo` of an `esds` box payload (ISO/IEC 14496-1
/// 7.2.6.5 and 7.2.6.7): the bytes of the first one in the
/// `DecoderConfigDescriptor` of the `ES_Descriptor`; `None` when the box
/// holds none or its descriptors run past their parents.
pub fn decoder_specific_info(esds: &[u8]) -> Option<&[u8]> {
    let mut at = 4usize;
    let (tag, len) = descriptor(esds, &mut at)?;
    if tag != 3 {
        return None;
    }
    let end = at.checked_add(len)?.min(esds.len());
    let flags = *esds.get(at.checked_add(2)?)?;
    at = at.checked_add(3)?;
    if flags & 0x80 != 0 {
        at = at.checked_add(2)?;
    }
    if flags & 0x40 != 0 {
        let url_len = usize::from(*esds.get(at)?);
        at = at.checked_add(1)?.checked_add(url_len)?;
    }
    if flags & 0x20 != 0 {
        at = at.checked_add(2)?;
    }
    while at < end {
        let (tag, len) = descriptor(esds, &mut at)?;
        let body_end = at.checked_add(len)?;
        if tag == 4 {
            let body = esds.get(at..body_end.min(end))?;
            // object type, stream type, buffer size, two bit rates.
            let mut inner = 13usize;
            while inner < body.len() {
                let (tag, len) = descriptor(body, &mut inner)?;
                let stop = inner.checked_add(len)?;
                if tag == 5 {
                    return body.get(inner..stop);
                }
                inner = stop;
            }
            return None;
        }
        at = body_end;
    }
    None
}

/// A descriptor tag and its expandable size (ISO/IEC 14496-1 8.3.3).
fn descriptor(data: &[u8], at: &mut usize) -> Option<(u8, usize)> {
    let tag = *data.get(*at)?;
    *at = at.checked_add(1)?;
    let mut len = 0usize;
    for _ in 0..4 {
        let byte = *data.get(*at)?;
        *at = at.checked_add(1)?;
        len = (len << 7) | usize::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((tag, len));
        }
    }
    None
}

fn be_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn be_u64(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        data.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn fourcc(data: &[u8], at: usize) -> Option<FourCc> {
    Some(FourCc(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(payload.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn data(kind: u32, value: &[u8]) -> Vec<u8> {
        boxed(b"data", &[&kind.to_be_bytes()[..], &[0; 4], value].concat())
    }

    /// A sample description with one visual entry holding an `avcC`.
    fn stsd() -> Vec<u8> {
        let mut body = vec![0u8; 78];
        body[7] = 1;
        body[12..16].copy_from_slice(b"appl");
        body[42] = 5;
        body[43..48].copy_from_slice(b"H.264");
        body.extend(boxed(b"avcC", &[1, 0x64, 0, 0x1f, 0xff, 0xe0, 0]));
        let entry = boxed(b"avc1", &body);
        boxed(b"stsd", &[&[0, 0, 0, 0, 0, 0, 0, 1][..], &entry].concat())
    }

    fn track() -> Vec<u8> {
        let mut tkhd = vec![0u8; 84];
        tkhd[3] = 3;
        tkhd[12..16].copy_from_slice(&7u32.to_be_bytes());
        let tkhd = boxed(b"tkhd", &tkhd);
        let tref = boxed(b"tref", &boxed(b"tmcd", &[0, 0, 0, 2]));
        let mut mdhd = vec![0u8; 24];
        mdhd[20..22].copy_from_slice(&0x15c7u16.to_be_bytes());
        let mdhd = boxed(b"mdhd", &mdhd);
        let hdlr = boxed(
            b"hdlr",
            &[&[0u8; 4][..], b"mhlrvide", &[0; 12], b"\x05Video"].concat(),
        );
        let stco = boxed(b"stco", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0x10, 0]);
        let stbl = boxed(b"stbl", &[stsd(), stco].concat());
        let minf = boxed(b"minf", &stbl);
        let mdia = boxed(b"mdia", &[mdhd, hdlr, minf].concat());
        let kind = boxed(b"kind", b"\0\0\0\0urn:mpeg:dash:role:2011\0dub\0");
        let udta = boxed(
            b"udta",
            &[kind, boxed(b"\xa9nam", b"\0\x02\0\0Tn")].concat(),
        );
        boxed(b"trak", &[tkhd, tref, mdia, udta].concat())
    }

    fn movie() -> Vec<u8> {
        let ftyp = boxed(b"ftyp", b"qt  \0\0\x02\0qt  ");
        let mut mvhd = vec![0u8; 100];
        mvhd[4..8].copy_from_slice(&3_850_167_845u32.to_be_bytes());
        let mvhd = boxed(b"mvhd", &mvhd);
        let free_item = boxed(
            b"----",
            &[
                boxed(b"mean", b"\0\0\0\0com.apple.iTunes"),
                boxed(b"name", b"\0\0\0\0iTunNORM"),
                data(1, b" 1 2"),
            ]
            .concat(),
        );
        let ilst = boxed(
            b"ilst",
            &[boxed(b"\xa9too", &data(1, b"Lavf")), free_item].concat(),
        );
        let hdlr_mdir = boxed(b"hdlr", &[&[0u8; 8][..], b"mdirappl", &[0; 9]].concat());
        let meta = boxed(b"meta", &[&[0u8; 4][..], &hdlr_mdir, &ilst].concat());
        let chpl = boxed(
            b"chpl",
            &[&[1u8, 0, 0, 0, 0, 0, 0, 0, 1][..], &[0; 8], b"\x02C1"].concat(),
        );
        let udta = boxed(b"udta", &[meta, chpl, boxed(b"FIRM", b"HD5")].concat());
        let keys = boxed(
            b"keys",
            &[&[0u8, 0, 0, 0, 0, 0, 0, 1][..], &boxed(b"mdta", b"com.x.k")].concat(),
        );
        let hdlr_mdta = boxed(b"hdlr", &[&[0u8; 8][..], b"mdta", &[0; 13]].concat());
        let item = boxed(&1u32.to_be_bytes(), &data(23, &30f32.to_be_bytes()));
        let mdta = boxed(
            b"meta",
            &[&[0u8; 4][..], &hdlr_mdta, &keys, &boxed(b"ilst", &item)].concat(),
        );
        let moov = boxed(b"moov", &[mvhd, track(), udta, mdta].concat());
        [ftyp, boxed(b"free", &[]), moov].concat()
    }

    /// Every `data` box of an item list entry is read, in order, with where
    /// its value starts: a cover picture entry holds one per picture.
    #[test]
    fn reads_every_data_box_of_an_entry() {
        let covr = boxed(
            b"covr",
            &[data(13, b"\xff\xd8one"), data(14, b"\x89PNGtwo")].concat(),
        );
        let hdlr = boxed(b"hdlr", &[&[0u8; 8][..], b"mdir", &[0; 13]].concat());
        let meta = boxed(
            b"meta",
            &[&[0u8; 4][..], &hdlr, &boxed(b"ilst", &covr)].concat(),
        );
        let file = boxed(b"moov", &boxed(b"udta", &meta));
        let read = read(SliceSource::new(&file), &Limits::default()).unwrap();
        let [
            Item {
                value: ItemValue::ListItem { data, .. },
                ..
            },
        ] = &read.items[..]
        else {
            panic!("{:?}", read.items);
        };
        let values: Vec<(u32, &[u8], u64)> = data
            .iter()
            .map(|d| (d.type_indicator, d.value.as_slice(), d.offset))
            .collect();
        assert_eq!(
            values,
            [(13, &b"\xff\xd8one"[..], 93), (14, &b"\x89PNGtwo"[..], 114)]
        );
    }

    #[test]
    fn reads_every_box_it_reports() {
        let file = movie();
        let meta = read(SliceSource::new(&file), &Limits::default()).unwrap();
        assert!(meta.complete);
        assert_eq!(
            meta.file_types,
            vec![FileType {
                major_brand: FourCc(*b"qt  "),
                minor_version: 512,
                compatible_brands: b"qt  ".to_vec(),
            }]
        );
        assert_eq!(
            meta.movie_created,
            Some(Timestamp {
                version: 0,
                seconds: 3_850_167_845
            })
        );
        let track = &meta.tracks[0];
        assert_eq!((track.track_id, track.flags), (Some(7), Some(3)));
        assert_eq!(track.language, Some(0x15c7));
        assert_eq!(track.handler, Some(FourCc(*b"vide")));
        assert_eq!(track.handler_component, Some(FourCc(*b"mhlr")));
        assert_eq!(track.handler_name, b"\x05Video");
        assert_eq!(
            track.references,
            vec![TrackReference {
                kind: FourCc(*b"tmcd"),
                track_ids: vec![2]
            }]
        );
        assert_eq!(
            track.kinds,
            vec![(b"urn:mpeg:dash:role:2011".to_vec(), b"dub".to_vec())]
        );
        assert_eq!(track.first_chunk_offset, Some(0x1000));
        let entry = track.sample_entry.as_ref().unwrap();
        assert_eq!(entry.kind, FourCc(*b"avc1"));
        assert_eq!(entry.vendor, Some(*b"appl"));
        assert_eq!(&entry.compressor_name.unwrap()[..6], b"\x05H.264");
        assert_eq!(entry.children.len(), 1);
        assert_eq!(entry.children[0].kind, FourCc(*b"avcC"));
        assert_eq!(entry.children[0].payload.len(), 7);
        let items: Vec<_> = meta
            .items
            .iter()
            .map(|i| match &i.value {
                ItemValue::UserData { kind, .. } => (i.track, format!("{kind}")),
                ItemValue::ListItem { key, .. } => (i.track, format!("{key:?}")),
            })
            .collect();
        assert_eq!(items.len(), 5, "{items:?}");
        assert_eq!(items[0], (Some(0), "\\xa9nam".to_owned()));
        assert!(items[1].1.starts_with("Atom"));
        assert!(items[2].1.starts_with("Free"));
        assert_eq!(items[3], (None, "FIRM".to_owned()));
        let ItemValue::ListItem { list, key, data } = &meta.items[4].value else {
            panic!("a list item");
        };
        assert_eq!(
            *list,
            ListPlace::Meta {
                handler: Some(FourCc(*b"mdta"))
            }
        );
        assert_eq!(
            *key,
            ListKey::Key {
                namespace: FourCc(*b"mdta"),
                name: b"com.x.k".to_vec()
            }
        );
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].type_indicator, 23);
        assert_eq!(
            meta.nero_chapters,
            Some(vec![NeroChapter {
                start: 0,
                title: b"C1".to_vec()
            }])
        );
    }

    #[test]
    fn reads_a_compressed_movie_box() {
        let file = movie();
        let at = file.windows(4).position(|w| w == b"moov").unwrap() - 4;
        let moov = &file[at..];
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(moov, 6);
        let cmvd = boxed(
            b"cmvd",
            &[
                &u32::try_from(moov.len()).unwrap().to_be_bytes()[..],
                &packed,
            ]
            .concat(),
        );
        let cmov = boxed(b"cmov", &[boxed(b"dcom", b"zlib"), cmvd].concat());
        let compressed = [&file[..at], &boxed(b"moov", &cmov)].concat();
        let plain = read(SliceSource::new(&file), &Limits::default()).unwrap();
        let meta = read(SliceSource::new(&compressed), &Limits::default()).unwrap();
        assert!(meta.compressed && meta.complete);
        // Offsets of an inflated box are not the file's.
        let unplaced = |items: &[Item]| {
            let mut items = items.to_vec();
            for item in &mut items {
                if let ItemValue::ListItem { data, .. } = &mut item.value {
                    for d in data {
                        d.offset = 0;
                    }
                }
            }
            items
        };
        assert_eq!(unplaced(&meta.items), unplaced(&plain.items));
        assert_eq!(meta.tracks, plain.tracks);
        assert_eq!(meta.movie_created, plain.movie_created);
    }

    #[test]
    fn every_truncation_and_corruption_is_safe() {
        let file = movie();
        for len in 0..file.len() {
            let meta = read(SliceSource::new(&file[..len]), &Limits::default()).unwrap();
            assert!(meta.tracks.len() <= 1);
        }
        for at in 0..file.len() {
            for value in [0u8, 0xff, 7] {
                let mut bad = file.clone();
                bad[at] = value;
                read(SliceSource::new(&bad), &Limits::default()).unwrap();
            }
        }
    }

    #[test]
    fn bounds_stop_the_walk() {
        let file = movie();
        let limits = Limits {
            max_retained_bytes: 64,
            ..Limits::default()
        };
        assert!(!read(SliceSource::new(&file), &limits).unwrap().complete);
        let limits = Limits {
            max_total_boxes: 5,
            ..Limits::default()
        };
        assert!(!read(SliceSource::new(&file), &limits).unwrap().complete);
    }

    #[test]
    fn key_names_kept_per_entry_are_charged() {
        let name = vec![b'k'; 1000];
        let keys = boxed(
            b"keys",
            &[&[0u8, 0, 0, 0, 0, 0, 0, 1][..], &boxed(b"mdta", &name)].concat(),
        );
        let hdlr = boxed(b"hdlr", &[&[0u8; 8][..], b"mdta", &[0; 13]].concat());
        let entries: Vec<u8> = (0..100)
            .flat_map(|_| boxed(&1u32.to_be_bytes(), &[]))
            .collect();
        let meta_box = boxed(
            b"meta",
            &[&[0u8; 4][..], &hdlr, &keys, &boxed(b"ilst", &entries)].concat(),
        );
        let file = boxed(b"moov", &meta_box);
        let limits = Limits {
            max_retained_bytes: 20_000,
            ..Limits::default()
        };
        let meta = read(SliceSource::new(&file), &limits).unwrap();
        assert!(!meta.complete);
        assert!(meta.items.len() < 20, "{}", meta.items.len());
    }

    #[test]
    fn decoder_specific_info_of_esds() {
        // ES_Descriptor, DecoderConfigDescriptor, DecoderSpecificInfo (2
        // bytes), SLConfigDescriptor.
        let esds = [
            0, 0, 0, 0, 3, 0x19, 0, 1, 0, 4, 0x11, 0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5,
            2, 0x12, 0x10, 6, 1, 2,
        ];
        assert_eq!(decoder_specific_info(&esds), Some(&[0x12, 0x10][..]));
        let mut without = esds;
        without[24] = 6;
        assert_eq!(decoder_specific_info(&without), None);
        for len in 0..esds.len() {
            let _ = decoder_specific_info(&esds[..len]);
        }
    }

    #[test]
    fn timecode_entry_and_its_name() {
        let mut body = vec![0u8; 26];
        body[12..16].copy_from_slice(&3u32.to_be_bytes());
        body[16..20].copy_from_slice(&30_000u32.to_be_bytes());
        body[20..24].copy_from_slice(&1001u32.to_be_bytes());
        body[24] = 30;
        body.extend(boxed(b"name", b"\0\x04\0\0Reel"));
        let stsd = [&[0u8; 8][..], &boxed(b"tmcd", &body)].concat();
        let entry = sample_entry(&stsd, Some(FourCc(*b"tmcd"))).unwrap();
        assert_eq!(
            entry.timecode,
            Some(TimecodeEntry {
                flags: 3,
                timescale: 30_000,
                frame_duration: 1001,
                frames: 30
            })
        );
        assert_eq!(entry.declared_len, 26 + 16);
        assert_eq!(entry.children[0].kind, FourCc(*b"name"));
    }
}
