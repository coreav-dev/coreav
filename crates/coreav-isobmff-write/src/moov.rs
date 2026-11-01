//! The movie box and file head shared by the writers: `ftyp`, `moov` with
//! one `trak` per output track, and the `mdat` header (ISO/IEC 14496-12
//! 8.1 to 8.7), written front to back so that `moov` comes first
//! (`docs/design/remux.md`).

use crate::ExtractError;
use crate::boxes::{Node, header_len, write_header};
use crate::tables::{Chunk, ChunkIter, Tables, push_offset};
use core::cmp::{Ordering, Reverse};
use coreav_isobmff::Error;
use std::borrow::Cow;
use std::collections::BinaryHeap;
use std::io::Write;

/// Unity transformation matrix (14496-12 8.2.2.3).
pub(crate) const UNITY: [i32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

/// Edit rate 1.0 in 16.16 fixed point (14496-12 8.6.6).
pub(crate) const UNIT_RATE: i32 = 0x0001_0000;

/// Track header flags: `track_enabled | track_in_movie` (14496-12 8.3.2).
pub(crate) const ENABLED_IN_MOVIE: u32 = 3;

/// What kind of media a track holds, with the fields that differ.
pub(crate) enum Media {
    /// A visual track: the track header's matrix, width and height (16.16)
    /// as stored in the source.
    Video { matrix: [i32; 9], size: (u32, u32) },
    /// An audio track.
    Audio,
}

/// One edit list entry: a duration in movie ticks, a media time (-1 for an
/// empty edit) and a rate in 16.16 fixed point (14496-12 8.6.6).
pub(crate) type EditEntry = crate::mux::Edit;

/// The track header fields other than times, size and matrix
/// (14496-12 8.3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrackFlags {
    pub(crate) flags: u32,
    pub(crate) layer: i16,
    pub(crate) alternate_group: i16,
    /// 8.8 fixed point; written only for audio tracks.
    pub(crate) volume: i16,
}

impl TrackFlags {
    /// Enabled, in the movie, layer 0, no alternate group, full volume.
    pub(crate) const DEFAULT: Self = Self {
        flags: ENABLED_IN_MOVIE,
        layer: 0,
        alternate_group: 0,
        volume: 0x0100,
    };
}

/// A box copied into the output as it was read: its type and its payload,
/// which for a `uuid` box starts with the 16-byte extended type.
pub(crate) struct CopiedBox<'a> {
    pub(crate) kind: [u8; 4],
    pub(crate) payload: Cow<'a, [u8]>,
}

/// One output track.
pub(crate) struct TrackBox<'a> {
    pub(crate) track_id: u32,
    pub(crate) media: Media,
    /// The handler name, up to its first NUL; the generic one
    /// (`SoundHandler`, `VideoHandler`) for `None`.
    pub(crate) handler_name: Option<&'a str>,
    pub(crate) header: TrackFlags,
    pub(crate) created: u64,
    pub(crate) modified: Option<u64>,
    pub(crate) media_created: u64,
    pub(crate) media_modified: Option<u64>,
    pub(crate) language: [u8; 3],
    pub(crate) media_timescale: u32,
    pub(crate) media_duration: u64,
    /// Track duration in movie ticks: the sum of the edits, or the media
    /// duration in movie ticks without an edit list.
    pub(crate) duration: u64,
    /// The edit list, empty for none.
    pub(crate) edits: Cow<'a, [EditEntry]>,
    /// The `stsd` payload after version and flags: entry count and entries.
    pub(crate) stsd: Vec<u8>,
    pub(crate) tables: &'a Tables,
    /// Boxes written at the end of `trak`, such as `udta`.
    pub(crate) extra: Vec<CopiedBox<'a>>,
    /// Boxes written at the end of `stbl`, such as `sgpd` and `sbgp`.
    pub(crate) stbl_extra: Vec<CopiedBox<'a>>,
}

/// The movie header values.
pub(crate) struct MovieBox<'a> {
    pub(crate) timescale: u32,
    pub(crate) created: u64,
    pub(crate) modified: Option<u64>,
    pub(crate) duration: u64,
    /// Boxes written at the end of `moov`, such as `udta` and `meta`.
    pub(crate) extra: Vec<CopiedBox<'a>>,
}

/// `ftyp` with `major`, minor version 0 and the `compatible` brands.
pub(crate) fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Node<'static> {
    ftyp_minor(major, 0, compatible)
}

/// `ftyp` with `major`, `minor` and the `compatible` brands.
pub(crate) fn ftyp_minor(major: &[u8; 4], minor: u32, compatible: &[&[u8; 4]]) -> Node<'static> {
    let mut payload = major.to_vec();
    payload.extend_from_slice(&minor.to_be_bytes());
    for brand in compatible {
        payload.extend_from_slice(*brand);
    }
    Node::leaf(b"ftyp", payload)
}

/// Writes `ftyp`, `moov` and the `mdat` header to `out`. The chunks of the
/// tracks follow in `mdat` in the order of [`interleave`]; returns the
/// offset of the first sample byte, and the number of sample bytes.
///
/// The chunk offsets depend on the movie box size, whose only variable part
/// is the offset width: 8 bytes per chunk with `co64`, 4 with `stco`. The
/// movie box is first sized without building `stsc` or the offsets.
pub(crate) fn write_head<W: Write>(
    out: &mut W,
    ftyp: Node<'_>,
    movie: &MovieBox<'_>,
    tracks: Vec<TrackBox<'_>>,
    ties: Ties,
) -> Result<(u64, u64), ExtractError> {
    let data_bytes = tracks
        .iter()
        .try_fold(0u64, |sum, t| sum.checked_add(t.tables.data_bytes))
        .ok_or_else(overflow)?;
    let chunks = tracks.iter().fold(0u64, |sum, t| {
        sum.saturating_add(u64::from(t.tables.chunk_count()))
    });
    let mdat_header = header_len(data_bytes);
    let sized = tracks
        .iter()
        .map(|t| t.tables.sized_boxes(true))
        .collect::<Result<Vec<_>, _>>()?;
    let wide = moov(movie, &tracks, sized).len();
    let narrow = wide
        .checked_sub(chunks.saturating_mul(4))
        .ok_or_else(overflow)?;
    let end = |moov_len: u64| {
        ftyp.len()
            .checked_add(moov_len)
            .and_then(|n| n.checked_add(mdat_header))
            .and_then(|n| n.checked_add(data_bytes))
            .ok_or_else(overflow)
    };
    let co64 = Tables::needs_co64(end(narrow)?);
    let moov_len = if co64 { wide } else { narrow };
    let data_start = end(moov_len)?
        .checked_sub(data_bytes)
        .ok_or_else(overflow)?;
    let tables: Vec<&Tables> = tracks.iter().map(|t| t.tables).collect();
    let mut offsets = tables
        .iter()
        .map(|t| t.offset_table(co64))
        .collect::<Result<Vec<_>, _>>()?;
    let mut at = data_start;
    interleave(&tables, ties, |track, chunk| -> Result<(), Error> {
        let table = offsets.get_mut(track).ok_or_else(overflow)?;
        push_offset(table, at, co64)?;
        at = at.checked_add(chunk.bytes).ok_or_else(overflow)?;
        Ok(())
    })?;
    let built = tracks
        .iter()
        .zip(offsets)
        .map(|(t, offsets)| t.tables.boxes(offsets, co64))
        .collect::<Result<Vec<_>, _>>()?;
    let moov = moov(movie, &tracks, built);
    if moov.len() != moov_len {
        return Err(overflow().into());
    }
    ftyp.write(out)?;
    moov.write(out)?;
    // Only the tables and the copy buffer stay allocated while samples are
    // copied.
    drop(moov);
    drop((ftyp, tracks));
    write_header(out, b"mdat", data_bytes)?;
    Ok((data_start, data_bytes))
}

/// How chunks that start at the same time are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ties {
    /// The earlier track first.
    TrackOrder,
    /// The smaller chunk first, then the earlier track: a stream's small
    /// first chunk (audio, say) is not stored after a very large one.
    SmallerFirst,
}

/// Calls `f` with every chunk of every track, as `(track, chunk)`, in the
/// order they are stored: by decode time in seconds, ties ordered by
/// `ties`, so that the tracks alternate about once a second. One track's
/// chunks come in their own order.
///
/// The per-track cursors and the queue of their next chunks are allocated
/// once, before the first call, so the copy pass that runs inside allocates
/// nothing beyond the copy buffer; each chunk costs
/// a logarithm of the number of tracks.
pub(crate) fn interleave<E: From<Error>>(
    tracks: &[&Tables],
    ties: Ties,
    mut f: impl FnMut(usize, Chunk) -> Result<(), E>,
) -> Result<(), E> {
    let mut cursors: Vec<ChunkIter<'_>> = Vec::new();
    cursors
        .try_reserve_exact(tracks.len())
        .map_err(|_| E::from(overflow()))?;
    let mut queue = BinaryHeap::new();
    queue
        .try_reserve_exact(tracks.len())
        .map_err(|_| E::from(overflow()))?;
    for (index, tables) in tracks.iter().enumerate() {
        let mut chunks = tables.chunks();
        if let Some(chunk) = chunks.next().transpose()? {
            queue.push(Reverse(Next::new(index, tables, chunk, ties)));
        }
        cursors.push(chunks);
    }
    while let Some(Reverse(next)) = queue.pop() {
        f(next.index, next.chunk)?;
        let (Some(chunks), Some(tables)) = (cursors.get_mut(next.index), tracks.get(next.index))
        else {
            return Err(overflow().into());
        };
        if let Some(chunk) = chunks.next().transpose().map_err(E::from)? {
            queue.push(Reverse(Next::new(next.index, tables, chunk, ties)));
        }
    }
    Ok(())
}

/// A track's next chunk, ordered by decode time in seconds, then as
/// `ties` says.
struct Next {
    index: usize,
    timescale: u32,
    chunk: Chunk,
    ties: Ties,
}

impl Next {
    fn new(index: usize, tables: &Tables, chunk: Chunk, ties: Ties) -> Self {
        Self {
            index,
            timescale: tables.timescale(),
            chunk,
            ties,
        }
    }
}

impl Ord for Next {
    fn cmp(&self, other: &Self) -> Ordering {
        // start / timescale compared exactly: a / b < c / d iff a d < c b.
        let mine = u128::from(self.chunk.start).saturating_mul(u128::from(other.timescale));
        let theirs = u128::from(other.chunk.start).saturating_mul(u128::from(self.timescale));
        let size = match self.ties {
            Ties::TrackOrder => Ordering::Equal,
            Ties::SmallerFirst => self.chunk.bytes.cmp(&other.chunk.bytes),
        };
        mine.cmp(&theirs)
            .then(size)
            .then(self.index.cmp(&other.index))
    }
}

impl PartialOrd for Next {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Next {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Next {}

fn moov<'a>(
    movie: &'a MovieBox<'_>,
    tracks: &'a [TrackBox<'_>],
    tables: Vec<Vec<Node<'a>>>,
) -> Node<'a> {
    let next_id = tracks
        .iter()
        .map(|t| t.track_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut children = vec![mvhd(movie, next_id)];
    for (track, tables) in tracks.iter().zip(tables) {
        children.push(trak(track, tables));
    }
    children.extend(movie.extra.iter().map(copied));
    Node::parent(b"moov", children)
}

fn copied<'a>(b: &'a CopiedBox<'_>) -> Node<'a> {
    Node::Leaf {
        kind: b.kind,
        head: Vec::new(),
        body: crate::boxes::Body::Bytes(Cow::Borrowed(&b.payload)),
    }
}

pub(crate) fn trak<'a>(track: &'a TrackBox<'_>, tables: Vec<Node<'a>>) -> Node<'a> {
    let mut stbl = vec![Node::full(b"stsd", 0, 0, &track.stsd)];
    stbl.extend(tables);
    stbl.extend(track.stbl_extra.iter().map(copied));
    let dref = {
        let mut payload = 1u32.to_be_bytes().to_vec();
        // A self-contained `url ` entry (flags 1).
        payload.extend_from_slice(&[0, 0, 0, 12, b'u', b'r', b'l', b' ', 0, 0, 0, 1]);
        Node::full(b"dref", 0, 0, &payload)
    };
    let header = match track.media {
        // graphicsmode 0 (copy), opcolor 0; flags 1 (14496-12 12.1.2).
        Media::Video { .. } => Node::full(b"vmhd", 0, 1, &[0; 8]),
        Media::Audio => Node::full(b"smhd", 0, 0, &[0; 4]),
    };
    let minf = Node::parent(
        b"minf",
        vec![
            header,
            Node::parent(b"dinf", vec![dref]),
            Node::parent(b"stbl", stbl),
        ],
    );
    let mdia = Node::parent(
        b"mdia",
        vec![mdhd(track), hdlr(&track.media, track.handler_name), minf],
    );
    let mut trak = vec![tkhd(track)];
    if !track.edits.is_empty() {
        trak.push(Node::parent(b"edts", vec![elst(&track.edits)]));
    }
    trak.push(mdia);
    trak.extend(track.extra.iter().map(copied));
    Node::parent(b"trak", trak)
}

/// Times and a duration, with version 1 when any value needs 64 bits
/// (14496-12 8.2.2, 8.3.2, 8.4.2).
fn times(
    created: u64,
    modified: Option<u64>,
    timescale: Option<u32>,
    track_id: Option<u32>,
    duration: u64,
) -> (u8, Vec<u8>) {
    let modified = modified.unwrap_or(created);
    let wide = u32::try_from(created).is_err()
        || u32::try_from(modified).is_err()
        || u32::try_from(duration).is_err();
    let put = |out: &mut Vec<u8>, value: u64| match u32::try_from(value) {
        Ok(compact) if !wide => out.extend_from_slice(&compact.to_be_bytes()),
        _ => out.extend_from_slice(&value.to_be_bytes()),
    };
    let mut out = Vec::with_capacity(32);
    put(&mut out, created);
    put(&mut out, modified);
    if let Some(timescale) = timescale {
        out.extend_from_slice(&timescale.to_be_bytes());
    }
    if let Some(id) = track_id {
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&[0; 4]);
    }
    put(&mut out, duration);
    (u8::from(wide), out)
}

fn matrix(values: &[i32; 9]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

pub(crate) fn mvhd(movie: &MovieBox<'_>, next_id: u32) -> Node<'static> {
    let (version, mut payload) = times(
        movie.created,
        movie.modified,
        Some(movie.timescale),
        None,
        movie.duration,
    );
    payload.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    payload.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    payload.extend_from_slice(&[0; 10]);
    payload.extend_from_slice(&matrix(&UNITY));
    payload.extend_from_slice(&[0; 24]);
    payload.extend_from_slice(&next_id.to_be_bytes());
    Node::full(b"mvhd", version, 0, &payload)
}

fn tkhd(track: &TrackBox<'_>) -> Node<'static> {
    let (version, mut payload) = times(
        track.created,
        track.modified,
        None,
        Some(track.track_id),
        track.duration,
    );
    payload.extend_from_slice(&[0; 8]);
    let header = track.header;
    payload.extend_from_slice(&header.layer.to_be_bytes());
    payload.extend_from_slice(&header.alternate_group.to_be_bytes());
    let (volume, values, (width, height)) = match &track.media {
        Media::Video { matrix, size } => (0i16, matrix, *size),
        Media::Audio => (header.volume, &UNITY, (0, 0)),
    };
    payload.extend_from_slice(&volume.to_be_bytes());
    payload.extend_from_slice(&[0; 2]);
    payload.extend_from_slice(&matrix(values));
    payload.extend_from_slice(&width.to_be_bytes());
    payload.extend_from_slice(&height.to_be_bytes());
    Node::full(b"tkhd", version, header.flags, &payload)
}

fn elst(edits: &[EditEntry]) -> Node<'static> {
    let wide = edits
        .iter()
        .any(|e| u32::try_from(e.duration).is_err() || i32::try_from(e.media_time).is_err());
    let mut payload = u32::try_from(edits.len())
        .unwrap_or(u32::MAX)
        .to_be_bytes()
        .to_vec();
    for edit in edits {
        if wide {
            payload.extend_from_slice(&edit.duration.to_be_bytes());
            payload.extend_from_slice(&edit.media_time.to_be_bytes());
        } else {
            payload.extend_from_slice(
                &u32::try_from(edit.duration)
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            );
            payload.extend_from_slice(&i32::try_from(edit.media_time).unwrap_or(-1).to_be_bytes());
        }
        payload.extend_from_slice(&edit.rate.to_be_bytes());
    }
    Node::full(b"elst", u8::from(wide), 0, &payload)
}

fn mdhd(track: &TrackBox<'_>) -> Node<'static> {
    let (version, mut payload) = times(
        track.media_created,
        track.media_modified,
        Some(track.media_timescale),
        None,
        track.media_duration,
    );
    // ISO 639-2/T code, three 5-bit letters offset by 0x60.
    let [a, b, c] = track
        .language
        .map(|l| u16::from(l.saturating_sub(0x60) & 0x1f));
    payload.extend_from_slice(&((a << 10) | (b << 5) | c).to_be_bytes());
    payload.extend_from_slice(&[0; 2]);
    Node::full(b"mdhd", version, 0, &payload)
}

/// The handler box: the name is a null-terminated UTF-8 string (ISO/IEC
/// 14496-12 8.4.3.3).
fn hdlr(media: &Media, name: Option<&str>) -> Node<'static> {
    let (kind, generic) = match media {
        Media::Video { .. } => (b"vide", "VideoHandler"),
        Media::Audio => (b"soun", "SoundHandler"),
    };
    let name = name
        .and_then(|n| n.split('\0').next())
        .filter(|n| !n.is_empty())
        .unwrap_or(generic);
    let mut payload = vec![0; 4];
    payload.extend_from_slice(kind);
    payload.extend_from_slice(&[0; 12]);
    payload.extend_from_slice(name.as_bytes());
    payload.push(0);
    Node::full(b"hdlr", 0, 0, &payload)
}

/// `value` ticks of `from` per second, in ticks of `to` per second, rounded
/// to the nearest tick. `from` is never 0 (checked by the callers).
pub(crate) fn rescale(value: u64, from: u32, to: u32) -> Result<u64, Error> {
    let scaled = u128::from(value)
        .checked_mul(u128::from(to))
        .and_then(|n| n.checked_add(u128::from(from / 2)))
        .and_then(|n| n.checked_div(u128::from(from)))
        .ok_or_else(overflow)?;
    u64::try_from(scaled).map_err(|_| overflow())
}

/// The language as an ISO 639-2/T code, `und` when the source has none.
pub(crate) fn language(code: Option<[u8; 3]>) -> [u8; 3] {
    code.filter(|l| l.iter().all(u8::is_ascii_lowercase))
        .unwrap_or(*b"und")
}

pub(crate) fn overflow() -> Error {
    Error::Overflow { offset: 0 }
}
