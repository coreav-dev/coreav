//! The shared full-file MP4 writer (`docs/design/remux.md`): any number of
//! tracks whose samples are copied unchanged from a source, written front
//! to back with `moov` before `mdat`, from ISO/IEC 14496-12 (boxes, sample
//! tables, edit lists) and ISO/IEC 14496-14 (MP4).
//!
//! [`Muxer`] does not read anything itself, so any demuxer can drive it in
//! two passes (every setter fails once the head is written):
//!
//! 1. For every track, [`Muxer::add_track`] with its header, then
//!    [`Muxer::push_sample`] with every sample's size, duration, sync flag
//!    and composition offset, in decode order. Only the output sample
//!    tables are kept, in their serialized form.
//! 2. [`Muxer::write_head`] writes `ftyp`, `moov` and the `mdat` header.
//!    [`Muxer::for_each_chunk`] then names, chunk by chunk, which track's
//!    next samples to copy, in the order they are stored in `mdat`; the
//!    caller lists that track's samples again and copies their bytes, for
//!    example through one [`CopyWindow`]. [`Muxer::sample_size`] checks
//!    that the second listing matches the first.
//!
//! Memory is the output sample tables, charged against
//! [`Limits::max_table_bytes`] across all tracks, plus what the caller
//! copies with; never the size of the media.

use crate::ExtractError;
use crate::boxes::Node;
use crate::copy::{COPY_BUFFER, Window};
use crate::moov::{
    CopiedBox, Media, MovieBox, Ties, TrackBox, TrackFlags, ftyp_minor, interleave, overflow,
    rescale, write_head,
};
use crate::tables::Tables;
use coreav_isobmff::{Error, FourCc, LimitKind, Limits, Source};
use std::borrow::Cow;
use std::io::Write;

/// Most bytes in one chunk. Chunks hold about one second of their track or
/// this much, whichever is less, so that the streams of a high-bit-rate
/// file alternate within the first megabytes: a reader that probes the
/// start of a file (`ffprobe` reads 5 MB by default) finds every stream.
pub const CHUNK_BYTES: u64 = 1 << 20;

/// One edit list entry (ISO/IEC 14496-12 8.6.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Duration in movie ticks.
    pub duration: u64,
    /// First media time presented, in media ticks; -1 for an empty edit.
    pub media_time: i64,
    /// Rate in 16.16 fixed point; [`Edit::UNIT_RATE`] for normal speed.
    pub rate: i32,
}

impl Edit {
    /// Rate 1.0.
    pub const UNIT_RATE: i32 = 0x0001_0000;
}

/// What a track carries, with the track header fields that differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackMedia {
    /// A visual track (handler `vide`, `vmhd`).
    Video {
        /// Transformation matrix as stored (ISO/IEC 14496-12 8.3.2).
        matrix: [i32; 9],
        /// Presentation width, 16.16 fixed point.
        width: u32,
        /// Presentation height, 16.16 fixed point.
        height: u32,
    },
    /// An audio track (handler `soun`, `smhd`).
    Audio {
        /// Volume, 8.8 fixed point (`0x0100` is full volume).
        volume: i16,
    },
}

/// A box written into the output as given: its type and payload. The
/// payload of a `uuid` box starts with its 16-byte extended type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawBox {
    /// Box type.
    pub kind: FourCc,
    /// Payload, without the size and type fields.
    pub payload: Vec<u8>,
}

/// The header of one output track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackHeader {
    /// Kind and its header fields.
    pub media: TrackMedia,
    /// Media timescale, at least 1.
    pub timescale: u32,
    /// ISO 639-2/T language code (three lowercase letters); `und` is
    /// written for anything else.
    pub language: [u8; 3],
    /// Track header creation time, seconds since 1904-01-01 UTC.
    pub creation_time: u64,
    /// Media header creation time, seconds since 1904-01-01 UTC.
    pub media_creation_time: u64,
    /// Track header flags (1 enabled, 2 in movie, 4 in preview).
    pub flags: u32,
    /// Track header layer.
    pub layer: i16,
    /// Track header alternate group.
    pub alternate_group: i16,
    /// The edit list; empty for none.
    pub edits: Vec<Edit>,
    /// The track header duration in movie ticks; `None` for the sum of the
    /// edits, or the media duration in movie ticks without an edit list
    /// (ISO/IEC 14496-12 8.3.2).
    pub duration: Option<u64>,
    /// The media header duration in media ticks; `None` for the sum of the
    /// pushed sample durations.
    pub media_duration: Option<u64>,
    /// One complete sample entry box, header included, as built by
    /// [`visual_sample_entry`] or [`audio_sample_entry`].
    pub sample_entry: Vec<u8>,
    /// Boxes written at the end of the sample table box, after the tables
    /// the muxer builds: per-sample information that stays valid when the
    /// samples keep their order, such as `sgpd`, `sbgp` and `sdtp`.
    pub sample_table_boxes: Vec<RawBox>,
    /// Boxes written at the end of the track box, such as `udta`.
    pub boxes: Vec<RawBox>,
}

/// The movie header values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovieHeader {
    /// Movie timescale, at least 1: the unit of the edit durations.
    pub timescale: u32,
    /// Creation time, seconds since 1904-01-01 UTC.
    pub creation_time: u64,
    /// Boxes written at the end of the movie box, such as `udta` and `meta`.
    pub boxes: Vec<RawBox>,
}

/// The file type box (ISO/IEC 14496-12 4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileType {
    /// Major brand.
    pub major: FourCc,
    /// Minor version.
    pub minor: u32,
    /// Compatible brands.
    pub compatible: Vec<FourCc>,
}

/// One sample, as the first pass sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Size in bytes.
    pub size: u32,
    /// Decode duration in media ticks.
    pub duration: u32,
    /// Sync sample.
    pub sync: bool,
    /// Composition time minus decode time, in media ticks; copied as
    /// stored (a version 1 `ctts` when one is negative).
    pub composition_offset: i64,
}

/// Where the sample data lies in the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    /// Offset of the first sample byte; also the bytes written so far.
    pub data_start: u64,
    /// Bytes of sample data that follow.
    pub data_bytes: u64,
}

impl Head {
    /// Size of the whole output once every sample is copied.
    pub fn total(&self) -> u64 {
        self.data_start.saturating_add(self.data_bytes)
    }
}

struct MuxTrack {
    header: TrackHeader,
    tables: Tables,
}

/// Writes an MP4 file whose samples are copied from elsewhere; see the
/// [module documentation](self).
pub struct Muxer {
    file_type: FileType,
    movie: MovieHeader,
    tracks: Vec<MuxTrack>,
    max_tracks: usize,
    table_limit: u64,
    table_used: u64,
    /// The head is written: tracks, samples and headers are fixed.
    written: bool,
}

impl Muxer {
    /// An empty movie. Fails when the movie timescale is 0.
    pub fn new(file_type: FileType, movie: MovieHeader, limits: &Limits) -> Result<Self, Error> {
        if movie.timescale == 0 {
            return Err(invalid("movie timescale 0"));
        }
        Ok(Self {
            file_type,
            movie,
            tracks: Vec::new(),
            max_tracks: usize::try_from(limits.max_tracks).unwrap_or(usize::MAX),
            table_limit: limits.max_table_bytes,
            table_used: 0,
            written: false,
        })
    }

    fn open(&self) -> Result<(), Error> {
        if self.written {
            return Err(invalid("the head is already written"));
        }
        Ok(())
    }

    /// Adds a track; returns its index, from 0 in the order added. Output
    /// `track_ID`s are the index plus one.
    pub fn add_track(&mut self, header: TrackHeader) -> Result<usize, Error> {
        self.open()?;
        if header.timescale == 0 {
            return Err(invalid("media timescale 0"));
        }
        if !is_one_box(&header.sample_entry) {
            return Err(invalid("the sample entry is not one box"));
        }
        if header.edits.iter().any(|e| e.media_time < -1) {
            return Err(invalid("an edit media time below -1"));
        }
        if self.tracks.len() >= self.max_tracks {
            return Err(Error::LimitExceeded {
                limit: LimitKind::Tracks,
                offset: 0,
            });
        }
        self.tracks.try_reserve(1).map_err(|_| table_limit())?;
        let tables = Tables::with_chunk_bytes(header.timescale, self.table_limit, CHUNK_BYTES);
        self.tracks.push(MuxTrack { header, tables });
        Ok(self.tracks.len().saturating_sub(1))
    }

    /// Removes the last added track, with its samples; for a track the
    /// caller finds it cannot copy after all.
    pub fn pop_track(&mut self) -> Option<TrackHeader> {
        if self.written {
            return None;
        }
        let track = self.tracks.pop()?;
        self.table_used = self.table_used.saturating_sub(track.tables.used());
        Some(track.header)
    }

    /// Sets the file type box, before [`Muxer::write_head`] (its brands
    /// once the tracks are known, for example).
    pub fn set_file_type(&mut self, file_type: FileType) -> Result<(), Error> {
        self.open()?;
        self.file_type = file_type;
        Ok(())
    }

    /// Number of tracks.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// A track's header.
    pub fn header(&self, track: usize) -> Option<&TrackHeader> {
        self.tracks.get(track).map(|t| &t.header)
    }

    /// Sets a track's edit list, before [`Muxer::write_head`]: once its
    /// samples are known, for example. Fails for a media time below -1.
    pub fn set_edits(&mut self, track: usize, edits: Vec<Edit>) -> Result<(), Error> {
        self.open()?;
        if edits.iter().any(|e| e.media_time < -1) {
            return Err(invalid("an edit media time below -1"));
        }
        let t = self.tracks.get_mut(track).ok_or(Error::NoSuchTrack)?;
        t.header.edits = edits;
        Ok(())
    }

    /// Sets a track's media header duration in media ticks; `None` for the
    /// sum of its sample durations.
    pub fn set_media_duration(&mut self, track: usize, duration: Option<u64>) -> Result<(), Error> {
        self.open()?;
        let t = self.tracks.get_mut(track).ok_or(Error::NoSuchTrack)?;
        t.header.media_duration = duration;
        Ok(())
    }

    /// Sets the boxes written at the end of a track box, such as `udta`.
    pub fn set_boxes(&mut self, track: usize, boxes: Vec<RawBox>) -> Result<(), Error> {
        self.open()?;
        let t = self.tracks.get_mut(track).ok_or(Error::NoSuchTrack)?;
        t.header.boxes = boxes;
        Ok(())
    }

    /// Sets the boxes written at the end of a track's sample table box.
    pub fn set_sample_table_boxes(
        &mut self,
        track: usize,
        boxes: Vec<RawBox>,
    ) -> Result<(), Error> {
        self.open()?;
        let t = self.tracks.get_mut(track).ok_or(Error::NoSuchTrack)?;
        t.header.sample_table_boxes = boxes;
        Ok(())
    }

    /// Sets the boxes written at the end of the movie box.
    pub fn set_movie_boxes(&mut self, boxes: Vec<RawBox>) -> Result<(), Error> {
        self.open()?;
        self.movie.boxes = boxes;
        Ok(())
    }

    /// Sets the movie timescale, the unit of the edits and track durations
    /// (which the caller sets in that unit). Fails for 0.
    pub fn set_movie_timescale(&mut self, timescale: u32) -> Result<(), Error> {
        self.open()?;
        if timescale == 0 {
            return Err(invalid("movie timescale 0"));
        }
        self.movie.timescale = timescale;
        Ok(())
    }

    /// Sets a track's header duration in movie ticks; `None` for the sum of
    /// its edits, or its media duration without an edit list.
    pub fn set_duration(&mut self, track: usize, duration: Option<u64>) -> Result<(), Error> {
        self.open()?;
        let t = self.tracks.get_mut(track).ok_or(Error::NoSuchTrack)?;
        t.header.duration = duration;
        Ok(())
    }

    /// Appends one sample to a track, in decode order. Fails when the
    /// output tables of all tracks together exceed
    /// [`Limits::max_table_bytes`], or a value does not fit the tables.
    pub fn push_sample(&mut self, track: usize, sample: Sample) -> Result<(), Error> {
        self.open()?;
        let t = self.tracks.get_mut(track).ok_or(Error::NoSuchTrack)?;
        let before = t.tables.used();
        t.tables.push_sample(
            sample.size,
            sample.duration,
            sample.sync,
            sample.composition_offset,
        )?;
        let grown = t.tables.used().saturating_sub(before);
        self.table_used = self.table_used.saturating_add(grown);
        if self.table_used > self.table_limit {
            return Err(table_limit());
        }
        Ok(())
    }

    /// Releases the unused capacity of a track's tables once its samples
    /// are pushed; [`Muxer::write_head`] does it for every track. Tables
    /// grow in 16 KiB blocks, so a caller that lists one track after the
    /// other keeps at most one track's partly used blocks this way.
    pub fn finish_track(&mut self, track: usize) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.tables.finish();
        }
    }

    /// Samples pushed to a track.
    pub fn samples(&self, track: usize) -> Option<u32> {
        self.tracks.get(track).map(|t| t.tables.samples())
    }

    /// Sum of a track's pushed sample durations, in media ticks.
    pub fn duration(&self, track: usize) -> Option<u64> {
        self.tracks.get(track).map(|t| t.tables.duration)
    }

    /// Size of a track's sample `index` (0-based), to check a second
    /// listing against the first.
    pub fn sample_size(&self, track: usize, index: u32) -> Option<u32> {
        self.tracks.get(track)?.tables.size(index)
    }

    /// Bytes of output sample tables held, across all tracks.
    pub fn table_bytes(&self) -> u64 {
        self.table_used
    }

    /// Writes `ftyp`, `moov` and the `mdat` header to `out`. The sample
    /// data follows in the order of [`Muxer::for_each_chunk`].
    ///
    /// Track header durations are the given ones, or the sums of their
    /// edits, or the media duration in movie ticks without an edit list;
    /// the movie duration is the longest track (ISO/IEC 14496-12 8.2.2,
    /// 8.3.2). Offsets are 32-bit (`stco`) unless the file ends beyond
    /// 4 GiB (`co64`).
    pub fn write_head<W: Write>(&mut self, out: &mut W) -> Result<Head, ExtractError> {
        self.open()?;
        self.written = true;
        for track in &mut self.tracks {
            track.tables.finish();
        }
        let movie_timescale = self.movie.timescale;
        let mut boxes = Vec::new();
        boxes
            .try_reserve_exact(self.tracks.len())
            .map_err(|_| table_limit())?;
        let mut movie_duration = 0u64;
        for (index, track) in self.tracks.iter().enumerate() {
            let h = &track.header;
            let media_duration = h.media_duration.unwrap_or(track.tables.duration);
            let duration = if let Some(duration) = h.duration {
                duration
            } else if h.edits.is_empty() {
                rescale(media_duration, h.timescale, movie_timescale)?
            } else {
                h.edits
                    .iter()
                    .try_fold(0u64, |sum, e| sum.checked_add(e.duration))
                    .ok_or_else(overflow)?
            };
            movie_duration = movie_duration.max(duration);
            let mut stsd = 1u32.to_be_bytes().to_vec();
            stsd.extend_from_slice(&h.sample_entry);
            let (media, header) = match h.media {
                TrackMedia::Video {
                    matrix,
                    width,
                    height,
                } => (
                    Media::Video {
                        matrix,
                        size: (width, height),
                    },
                    TrackFlags {
                        flags: h.flags,
                        layer: h.layer,
                        alternate_group: h.alternate_group,
                        volume: 0,
                    },
                ),
                TrackMedia::Audio { volume } => (
                    Media::Audio,
                    TrackFlags {
                        flags: h.flags,
                        layer: h.layer,
                        alternate_group: h.alternate_group,
                        volume,
                    },
                ),
            };
            boxes.push(TrackBox {
                track_id: u32::try_from(index)
                    .ok()
                    .and_then(|i| i.checked_add(1))
                    .ok_or_else(overflow)?,
                media,
                header,
                created: h.creation_time,
                media_created: h.media_creation_time,
                language: h.language,
                media_timescale: h.timescale,
                media_duration,
                duration,
                edits: Cow::Borrowed(&h.edits),
                stsd,
                tables: &track.tables,
                extra: copied(&h.boxes),
                stbl_extra: copied(&h.sample_table_boxes),
            });
        }
        let brands: Vec<&[u8; 4]> = self.file_type.compatible.iter().map(|b| &b.0).collect();
        let head = write_head(
            out,
            ftyp_minor(&self.file_type.major.0, self.file_type.minor, &brands),
            &MovieBox {
                timescale: movie_timescale,
                created: self.movie.creation_time,
                duration: movie_duration,
                extra: copied(&self.movie.boxes),
            },
            boxes,
            Ties::SmallerFirst,
        )?;
        Ok(Head {
            data_start: head.0,
            data_bytes: head.1,
        })
    }

    /// Calls `f(track, samples)` for every chunk of the output in the order
    /// the chunks are stored in `mdat`: the caller copies the next
    /// `samples` samples of `track`. Chunks hold about one second of their
    /// track or [`CHUNK_BYTES`], and the tracks alternate by decode time;
    /// of chunks that start at the same time the smaller comes first, so
    /// that a stream's small first chunk is not stored after a large one
    /// (one key frame of high-bit-rate video can exceed the 5 MB `ffprobe`
    /// reads to identify streams).
    pub fn for_each_chunk<E: From<Error>>(
        &self,
        mut f: impl FnMut(usize, u32) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut tables = Vec::new();
        tables
            .try_reserve_exact(self.tracks.len())
            .map_err(|_| E::from(table_limit()))?;
        tables.extend(self.tracks.iter().map(|t| &t.tables));
        interleave(&tables, Ties::SmallerFirst, |track, chunk| {
            f(track, chunk.samples)
        })
    }
}

fn copied(boxes: &[RawBox]) -> Vec<CopiedBox<'_>> {
    boxes
        .iter()
        .map(|b| CopiedBox {
            kind: b.kind.0,
            payload: Cow::Borrowed(&b.payload),
        })
        .collect()
}

/// The one copy buffer of the writers, which also
/// serves as a read window: a sample outside it refills it from the
/// sample's start, so samples stored close together need one read.
pub struct CopyWindow(Window);

impl CopyWindow {
    /// A 64 KiB window.
    pub fn new() -> Self {
        Self(Window::new(COPY_BUFFER))
    }

    /// Writes the `size` bytes at `offset` of `source` to `out`.
    pub fn copy<S: Source, W: Write>(
        &mut self,
        source: &mut S,
        offset: u64,
        size: u32,
        out: &mut W,
    ) -> Result<(), ExtractError> {
        self.0.copy(source, offset, size, out)
    }
}

impl Default for CopyWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// An ISO visual sample entry (ISO/IEC 14496-12 12.1.3): the coded width
/// and height, 72 dpi, one frame per sample, the compressor name (a length
/// byte and up to 31 bytes; all zeros for none) and the depth (`0x0018`
/// for colour without alpha), followed by `children` (the codec
/// configuration box first) in the given order. Returns the whole entry
/// box.
pub fn visual_sample_entry(
    fourcc: FourCc,
    width: u16,
    height: u16,
    compressor_name: &[u8; 32],
    depth: u16,
    children: &[RawBox],
) -> Result<Vec<u8>, Error> {
    let mut entry = Vec::with_capacity(78);
    entry.extend_from_slice(&[0; 6]);
    entry.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    entry.extend_from_slice(&[0; 16]); // pre_defined, reserved
    entry.extend_from_slice(&width.to_be_bytes());
    entry.extend_from_slice(&height.to_be_bytes());
    entry.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi
    entry.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    entry.extend_from_slice(&[0; 4]); // reserved
    entry.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    entry.extend_from_slice(compressor_name);
    entry.extend_from_slice(&depth.to_be_bytes());
    entry.extend_from_slice(&(-1i16).to_be_bytes()); // pre_defined
    entry_box(fourcc, entry, children)
}

/// A colour description as ITU-T H.273 code points, written as a `colr`
/// box of type `nclx` (ISO/IEC 14496-12 12.1.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nclx {
    /// `colour_primaries`.
    pub primaries: u16,
    /// `transfer_characteristics`.
    pub transfer: u16,
    /// `matrix_coefficients`.
    pub matrix: u16,
    /// `full_range_flag`.
    pub full_range: bool,
}

/// An H.264 `avc1` sample entry (ISO/IEC 14496-15 5.4.2.1) built with
/// [`visual_sample_entry`]: the coded `width` and `height`, no compressor
/// name, depth `0x0018`, and its children in this order: `avcC` with
/// `avcc` (an `AVCDecoderConfigurationRecord`, 5.3.3.1, written as given),
/// then `colr` (`nclx`) when `colour` is given, then `pasp` (ISO/IEC
/// 14496-12 12.1.4, horizontal and vertical spacing) when `pixel_aspect`
/// is given. Returns the whole entry box. Fails when `avcc` is not a
/// configuration record of version 1, or a spacing is 0.
pub fn avc1_sample_entry(
    width: u16,
    height: u16,
    avcc: &[u8],
    pixel_aspect: Option<(u32, u32)>,
    colour: Option<Nclx>,
) -> Result<Vec<u8>, Error> {
    if avcc.len() < 7 || avcc.first() != Some(&1) {
        return Err(invalid(
            "the avcC payload is not an AVCDecoderConfigurationRecord",
        ));
    }
    let mut children = vec![RawBox {
        kind: FourCc(*b"avcC"),
        payload: avcc.to_vec(),
    }];
    if let Some(c) = colour {
        let mut nclx = b"nclx".to_vec();
        for code in [c.primaries, c.transfer, c.matrix] {
            nclx.extend_from_slice(&code.to_be_bytes());
        }
        nclx.push(if c.full_range { 0x80 } else { 0 });
        children.push(RawBox {
            kind: FourCc(*b"colr"),
            payload: nclx,
        });
    }
    if let Some((h, v)) = pixel_aspect {
        if h == 0 || v == 0 {
            return Err(invalid("a pixel aspect spacing of 0"));
        }
        let mut pasp = h.to_be_bytes().to_vec();
        pasp.extend_from_slice(&v.to_be_bytes());
        children.push(RawBox {
            kind: FourCc(*b"pasp"),
            payload: pasp,
        });
    }
    visual_sample_entry(FourCc(*b"avc1"), width, height, &[0; 32], 0x0018, &children)
}

/// An ISO version 0 audio sample entry (ISO/IEC 14496-12 12.2.3): the
/// channel count, 16-bit sample size and the sample rate in Hz (at most
/// 65,535), followed by `children` (the codec configuration box first).
/// Returns the whole entry box.
pub fn audio_sample_entry(
    fourcc: FourCc,
    channels: u16,
    sample_rate: u32,
    children: &[RawBox],
) -> Result<Vec<u8>, Error> {
    let rate = u16::try_from(sample_rate).map_err(|_| Error::Unsupported {
        reason: "sample rate above 65535 Hz",
    })?;
    let mut entry = Vec::with_capacity(28);
    entry.extend_from_slice(&[0; 6]);
    entry.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    entry.extend_from_slice(&[0; 8]); // version 0, reserved
    entry.extend_from_slice(&channels.to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes()); // samplesize
    entry.extend_from_slice(&[0; 4]); // pre_defined, reserved
    entry.extend_from_slice(&(u32::from(rate) << 16).to_be_bytes());
    entry_box(fourcc, entry, children)
}

/// An `esds` payload (ISO/IEC 14496-1 7.2.6.5): an ES descriptor with a
/// decoder configuration for AAC (object type 0x40, audio stream) holding
/// `config`, and the predefined SL configuration 2.
pub fn esds(config: &[u8]) -> Vec<u8> {
    fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let mut len = body.len();
        let mut bytes = [0u8; 4];
        for slot in bytes.iter_mut().rev() {
            *slot = u8::try_from(len & 0x7F).unwrap_or(0);
            len >>= 7;
        }
        for (i, b) in bytes.iter().enumerate() {
            out.push(if i < 3 { b | 0x80 } else { *b });
        }
        out.extend_from_slice(body);
        out
    }
    let mut decoder = vec![0x40, 0x15, 0, 0, 0];
    decoder.extend_from_slice(&[0; 8]);
    decoder.extend(descriptor(0x05, config));
    let mut es = vec![0, 0, 0];
    es.extend(descriptor(0x04, &decoder));
    es.extend(descriptor(0x06, &[0x02]));
    let mut out = vec![0, 0, 0, 0];
    out.extend(descriptor(0x03, &es));
    out
}

fn entry_box(fourcc: FourCc, mut entry: Vec<u8>, children: &[RawBox]) -> Result<Vec<u8>, Error> {
    for child in children {
        Node::leaf(&child.kind.0, child.payload.clone())
            .write(&mut entry)
            .map_err(|_| overflow())?;
    }
    let mut out = Vec::new();
    Node::leaf(&fourcc.0, entry)
        .write(&mut out)
        .map_err(|_| overflow())?;
    Ok(out)
}

/// Whether `bytes` is exactly one box with a 32-bit size.
fn is_one_box(bytes: &[u8]) -> bool {
    bytes
        .get(..4)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .is_some_and(|size| {
            let size = u32::from_be_bytes(size);
            size >= 8 && usize::try_from(size).is_ok_and(|s| s == bytes.len())
        })
}

fn invalid(reason: &'static str) -> Error {
    Error::Unsupported { reason }
}

fn table_limit() -> Error {
    Error::LimitExceeded {
        limit: LimitKind::TableBytes,
        offset: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_isobmff::{Allowlist, Budget, SliceSource, probe};

    fn header(media: TrackMedia, entry: Vec<u8>) -> TrackHeader {
        TrackHeader {
            media,
            timescale: 1000,
            language: *b"eng",
            creation_time: 0,
            media_creation_time: 0,
            flags: 3,
            layer: 0,
            alternate_group: 0,
            edits: Vec::new(),
            duration: None,
            media_duration: None,
            sample_entry: entry,
            sample_table_boxes: Vec::new(),
            boxes: Vec::new(),
        }
    }

    #[test]
    fn writes_tracks_that_the_parser_reads_back() {
        let limits = Limits::default();
        let mut muxer = Muxer::new(
            FileType {
                major: FourCc(*b"isom"),
                minor: 0,
                compatible: vec![FourCc(*b"isom")],
            },
            MovieHeader {
                timescale: 600,
                creation_time: 0,
                boxes: vec![RawBox {
                    kind: FourCc(*b"udta"),
                    payload: b"\0\0\0\x0cname\0\0\0\0".to_vec(),
                }],
            },
            &limits,
        )
        .unwrap();
        let avcc = RawBox {
            kind: FourCc(*b"avcC"),
            payload: vec![1, 0x64, 0, 0x1f, 0xff, 0xe0, 0],
        };
        let video = muxer
            .add_track(header(
                TrackMedia::Video {
                    matrix: crate::moov::UNITY,
                    width: 320 << 16,
                    height: 240 << 16,
                },
                visual_sample_entry(FourCc(*b"avc1"), 320, 240, &[0; 32], 0x18, &[avcc]).unwrap(),
            ))
            .unwrap();
        let mut esds = vec![0u8; 4];
        esds.extend_from_slice(&[3, 25, 0, 1, 0, 4, 17, 0x40, 0x15]);
        esds.extend_from_slice(&[0; 11]);
        esds.extend_from_slice(&[5, 2, 0x12, 0x10, 6, 1, 2]);
        let audio = muxer
            .add_track(header(
                TrackMedia::Audio { volume: 0x0100 },
                audio_sample_entry(
                    FourCc(*b"mp4a"),
                    2,
                    48_000,
                    &[RawBox {
                        kind: FourCc(*b"esds"),
                        payload: esds,
                    }],
                )
                .unwrap(),
            ))
            .unwrap();
        // Samples: video byte `i`, audio byte `100 + i`.
        let mut data = Vec::new();
        for i in 0..5u8 {
            muxer
                .push_sample(
                    video,
                    Sample {
                        size: 1,
                        duration: 500,
                        sync: i == 0,
                        composition_offset: if i == 0 { 0 } else { -100 },
                    },
                )
                .unwrap();
            data.push(i);
        }
        for i in 0..3u8 {
            muxer
                .push_sample(
                    audio,
                    Sample {
                        size: 1,
                        duration: 1024,
                        sync: true,
                        composition_offset: 0,
                    },
                )
                .unwrap();
            data.push(100 + i);
        }
        muxer
            .set_edits(
                video,
                vec![Edit {
                    duration: 1200,
                    media_time: 100,
                    rate: Edit::UNIT_RATE,
                }],
            )
            .unwrap();
        assert!(
            muxer
                .set_edits(
                    video,
                    vec![Edit {
                        duration: 1,
                        media_time: -2,
                        rate: Edit::UNIT_RATE,
                    }],
                )
                .is_err()
        );
        let mut out = Vec::new();
        let head = muxer.write_head(&mut out).unwrap();
        assert_eq!(head.data_start, out.len() as u64);
        let (mut next_video, mut next_audio) = (0usize, 5usize);
        muxer
            .for_each_chunk(|track, samples| -> Result<(), Error> {
                for _ in 0..samples {
                    let at = if track == video {
                        next_video += 1;
                        next_video - 1
                    } else {
                        next_audio += 1;
                        next_audio - 1
                    };
                    out.push(data[at]);
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(head.total(), out.len() as u64);

        let movie = probe(SliceSource::new(&out), &limits, &Allowlist::default()).unwrap();
        assert_eq!(movie.timescale, 600);
        assert_eq!(movie.tracks.len(), 2);
        let mut budget = Budget::new(&limits);
        for (t, first, durations) in [(0usize, 0u8, 500u32), (1, 100, 1024)] {
            let track = &movie.tracks[t];
            let packets: Vec<_> = movie
                .packets(track.index, SliceSource::new(&out), &limits, &mut budget)
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for (i, p) in packets.iter().enumerate() {
                assert_eq!(out[p.offset as usize], first + i as u8);
                assert_eq!(p.duration, durations);
            }
        }
        assert_eq!(movie.tracks[0].language, Some(*b"eng"));
        assert_eq!(movie.tracks[0].ctts_version(), Some(1));
        assert_eq!(movie.tracks[1].description.sample_rate, Some(48_000));
        // The copied box ends the movie box.
        let udta = out.windows(4).position(|w| w == b"udta").unwrap();
        assert_eq!(&out[udta + 4..udta + 8], b"\0\0\0\x0c");
    }

    #[test]
    fn tables_share_one_limit() {
        let limits = Limits {
            max_table_bytes: 100,
            ..Limits::default()
        };
        let mut muxer = Muxer::new(
            FileType {
                major: FourCc(*b"isom"),
                minor: 0,
                compatible: Vec::new(),
            },
            MovieHeader {
                timescale: 1000,
                creation_time: 0,
                boxes: Vec::new(),
            },
            &limits,
        )
        .unwrap();
        let entry = audio_sample_entry(FourCc(*b"mp4a"), 2, 48_000, &[]).unwrap();
        let a = muxer
            .add_track(header(TrackMedia::Audio { volume: 256 }, entry.clone()))
            .unwrap();
        let b = muxer
            .add_track(header(TrackMedia::Audio { volume: 256 }, entry))
            .unwrap();
        let sample = |size| Sample {
            size,
            duration: 1,
            sync: true,
            composition_offset: 0,
        };
        // 32 bytes for the first sample of each track, 4 per size after.
        muxer.push_sample(a, sample(1)).unwrap();
        muxer.push_sample(b, sample(1)).unwrap();
        for size in 2..40 {
            if muxer.push_sample(a, sample(size)).is_err() {
                assert!(muxer.table_bytes() > 100);
                let used = muxer.table_bytes();
                muxer.pop_track();
                assert!(muxer.table_bytes() < used);
                return;
            }
        }
        panic!("the shared limit was never reached");
    }

    #[test]
    fn bad_headers_are_refused() {
        let limits = Limits::default();
        let movie = |timescale| MovieHeader {
            timescale,
            creation_time: 0,
            boxes: Vec::new(),
        };
        let file_type = FileType {
            major: FourCc(*b"isom"),
            minor: 0,
            compatible: Vec::new(),
        };
        assert!(Muxer::new(file_type.clone(), movie(0), &limits).is_err());
        let mut muxer = Muxer::new(file_type, movie(1000), &limits).unwrap();
        let mut h = header(
            TrackMedia::Audio { volume: 256 },
            vec![0, 0, 0, 9, 1, 2, 3, 4],
        );
        assert!(muxer.add_track(h.clone()).is_err());
        h.sample_entry = audio_sample_entry(FourCc(*b"mp4a"), 2, 8000, &[]).unwrap();
        h.timescale = 0;
        assert!(muxer.add_track(h.clone()).is_err());
        h.timescale = 8000;
        h.edits = vec![Edit {
            duration: 1,
            media_time: -2,
            rate: Edit::UNIT_RATE,
        }];
        assert!(muxer.add_track(h).is_err());
        assert!(audio_sample_entry(FourCc(*b"mp4a"), 2, 65_535, &[]).is_ok());
        assert!(audio_sample_entry(FourCc(*b"mp4a"), 2, 96_000, &[]).is_err());
    }
}
