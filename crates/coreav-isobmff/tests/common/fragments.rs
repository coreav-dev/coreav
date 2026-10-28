//! A virtual fragmented movie (ISO/IEC 14496-12 8.8), served without ever
//! holding the file in memory: a `ftyp`, a `moov` whose sample tables are
//! empty and whose `mvex` declares the tracks, then fragments of identical
//! length, each a `moof` with one `traf` per track and one `mdat`.
//!
//! Fragment bytes are regenerated into one reused buffer when a read moves
//! to another fragment, so the source allocates nothing after the first
//! fragment it serves and a 2.2 GB file costs the test its header only.
//! Both crates' tests use it: the pinned limit tests of
//! `docs/design/limits.md` need large and hostile fragmented inputs, and
//! the writer's need one with a track it can copy.
#![allow(dead_code, clippy::indexing_slicing)]

use super::{aac_entry, bx, cat, full, u32s, visual_entry};
use coreav_isobmff::{Error, Source};

/// What a track's samples are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// ProRes 422 HQ (`apch`): every sample intra, no codec configuration,
    /// so a stream copy keeps the container's sync flags
    /// (`docs/design/cut-points.md`).
    ProRes,
    /// H.264 (`avc1`): each sample one slice NAL unit, an IDR picture when
    /// the sample is a sync sample and a non-IDR one otherwise, so that the
    /// coded data agrees with the fragment's sample flags.
    H264,
    /// AAC LC (`mp4a` with an `esds`).
    Aac,
}

/// One track of a virtual fragmented movie.
#[derive(Clone, Copy)]
pub struct Track {
    /// What its samples are.
    pub kind: Kind,
    /// Media timescale.
    pub timescale: u32,
    /// Samples per fragment.
    pub samples: u32,
    /// Bytes per sample.
    pub size: u32,
    /// Decode duration of a sample, in media ticks.
    pub duration: u32,
    /// A sync sample every `gop` samples of the track, counted from its
    /// first; 1 marks every sample a sync sample.
    pub gop: u32,
}

impl Track {
    /// The video track of the 2.2 GB pinned-limit file: 10
    /// intra samples of 11,000 bytes a fragment, 25 frames a second.
    pub const PRORES: Self = Self {
        kind: Kind::ProRes,
        timescale: 12_800,
        samples: 10,
        size: 11_000,
        duration: 512,
        gop: 1,
    };

    /// The same shape in H.264, with one sync sample a second.
    pub const H264: Self = Self {
        kind: Kind::H264,
        gop: 25,
        ..Self::PRORES
    };

    /// AAC at 48 kHz: 19 frames of 1,024 ticks a fragment, which is about
    /// as long as the video fragment above.
    pub const AAC: Self = Self {
        kind: Kind::Aac,
        timescale: 48_000,
        samples: 19,
        size: 300,
        duration: 1024,
        gop: 1,
    };

    fn video(&self) -> bool {
        self.kind != Kind::Aac
    }

    fn entry(&self) -> Vec<u8> {
        match self.kind {
            Kind::ProRes => visual_entry(b"apch", &[]),
            Kind::H264 => visual_entry(b"avc1", &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0])),
            Kind::Aac => aac_entry(),
        }
    }

    /// The `sample_flags` (ISO/IEC 14496-12 8.8.3.1) of sample `index` of
    /// the track: `sample_depends_on` 2 and the sync bit clear for a sync
    /// sample, `sample_depends_on` 1 and `sample_is_non_sync_sample` set
    /// otherwise.
    fn flags(&self, index: u64) -> u32 {
        if index % u64::from(self.gop.max(1)) == 0 {
            0x0200_0000
        } else {
            0x0101_0000
        }
    }

    /// The sample's bytes, written into `out`.
    fn sample(&self, index: u64, out: &mut Vec<u8>) {
        let start = out.len();
        if self.kind == Kind::H264 && self.size >= 6 {
            out.extend_from_slice(&(self.size - 4).to_be_bytes());
            // nal_ref_idc 3 with nal_unit_type 5 (IDR) or 1 (non-IDR),
            // then first_mb_in_slice 0 and a slice type.
            out.push(if self.flags(index) == 0x0200_0000 {
                0x65
            } else {
                0x41
            });
            out.push(0x88);
        }
        let written = out.len() - start;
        let filler = usize::try_from(self.size)
            .unwrap_or(0)
            .saturating_sub(written);
        out.resize(out.len() + filler, 0xab);
    }
}

/// A virtual fragmented movie. Reads are served from the header and from
/// one regenerated fragment, so a read that stays inside a fragment copies
/// bytes and allocates nothing.
pub struct Fragmented {
    head: Vec<u8>,
    tracks: Vec<Track>,
    fragment_len: u64,
    fragments: u64,
    buffer: Vec<u8>,
    /// Scratch: where each `trun`'s data offset sits in `buffer`.
    sites: Vec<usize>,
    cached: Option<u64>,
    read: u64,
}

impl Fragmented {
    /// A movie of `fragments` fragments of these tracks.
    pub fn new(tracks: &[Track], fragments: u64) -> Self {
        let head = head(tracks);
        let mut source = Self {
            head,
            tracks: tracks.to_vec(),
            fragment_len: 0,
            fragments,
            buffer: Vec::new(),
            sites: Vec::new(),
            cached: None,
            read: 0,
        };
        source.fill(0);
        source.fragment_len = source.buffer.len() as u64;
        source
    }

    /// The 2.2 GB file of the pinned limit tests:
    /// 20,000 fragments of 10 ProRes samples of 11,000 bytes.
    pub fn prores(fragments: u64) -> Self {
        Self::new(&[Track::PRORES], fragments)
    }

    /// Bytes handed to callers so far.
    pub fn read_bytes(&self) -> u64 {
        self.read
    }

    fn total(&self) -> u64 {
        self.head.len() as u64 + self.fragment_len * self.fragments
    }

    /// Regenerates fragment `index` into the reused buffer, which keeps
    /// its capacity, so that only the first fragment a source serves
    /// allocates.
    fn fill(&mut self, index: u64) {
        if self.cached == Some(index) {
            return;
        }
        let mut buffer = std::mem::take(&mut self.buffer);
        let mut sites = std::mem::take(&mut self.sites);
        buffer.clear();
        sites.clear();
        fragment(&self.tracks, index, &mut buffer, &mut sites);
        self.buffer = buffer;
        self.sites = sites;
        self.cached = Some(index);
    }

    /// Walks the bytes of `[offset, offset + len)`, a run at a time, each
    /// a slice of the header or of the fragment buffer. Fails before
    /// yielding anything when they run past the end. It allocates nothing:
    /// what the pinned tests measure must be the library's memory, not the
    /// fixture's.
    fn runs(&mut self, offset: u64, len: usize, mut each: impl FnMut(&[u8])) -> Result<(), Error> {
        let end = offset
            .checked_add(len as u64)
            .ok_or(Error::Truncated { offset })?;
        if end > self.total() {
            return Err(Error::Truncated { offset });
        }
        self.read += len as u64;
        let head = self.head.len() as u64;
        let mut at = offset;
        while at < end {
            if at < head {
                let take = (head - at).min(end - at) as usize;
                let from = at as usize;
                each(&self.head[from..from + take]);
                at += take as u64;
                continue;
            }
            let relative = at - head;
            self.fill(relative / self.fragment_len);
            let within = (relative % self.fragment_len) as usize;
            let take = (self.fragment_len - within as u64).min(end - at) as usize;
            each(&self.buffer[within..within + take]);
            at += take as u64;
        }
        Ok(())
    }
}

impl Source for Fragmented {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.total())
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let mut done = 0;
        let len = buf.len();
        self.runs(offset, len, |bytes| {
            buf[done..done + bytes.len()].copy_from_slice(bytes);
            done += bytes.len();
        })?;
        Ok(())
    }

    fn read_append(&mut self, offset: u64, len: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        self.runs(offset, len, |bytes| buf.extend_from_slice(bytes))
    }
}

/// `ftyp` and the `moov` of a fragmented movie: empty sample tables, and a
/// `mvex` with one `trex` per track.
fn head(tracks: &[Track]) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"iso6\0\0\x02\0");
    let mut mvhd = u32s(&[0, 0, 1000, 0]);
    mvhd.extend_from_slice(&[0u8; 80]);
    let mut body = full(b"mvhd", 0, 0, &mvhd);
    for (i, track) in tracks.iter().enumerate() {
        body.extend_from_slice(&trak(track, i as u32 + 1));
    }
    let mut mvex = Vec::new();
    for (i, track) in tracks.iter().enumerate() {
        mvex.extend_from_slice(&full(
            b"trex",
            0,
            0,
            &u32s(&[i as u32 + 1, 1, track.duration, track.size, 0]),
        ));
    }
    body.extend_from_slice(&bx(b"mvex", &mvex));
    cat(&[ftyp, bx(b"moov", &body)])
}

fn trak(track: &Track, id: u32) -> Vec<u8> {
    let mut tkhd = u32s(&[0, 0, id, 0, 0]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]));
    tkhd.extend_from_slice(&u32s(if track.video() {
        &[320 << 16, 240 << 16]
    } else {
        &[0, 0]
    }));
    let mut mdhd = u32s(&[0, 0, track.timescale, 0]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(if track.video() { b"vide" } else { b"soun" });
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&track.entry());
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &u32s(&[0])),
            full(b"stsc", 0, 0, &u32s(&[0])),
            full(b"stsz", 0, 0, &u32s(&[0, 0])),
            full(b"stco", 0, 0, &u32s(&[0])),
        ]),
    );
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let media_header = if track.video() {
        full(b"vmhd", 0, 1, &[0; 8])
    } else {
        full(b"smhd", 0, 0, &[0; 4])
    };
    let minf = bx(b"minf", &cat(&[media_header, bx(b"dinf", &dref), stbl]));
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia]))
}

/// Starts a box of this type, returning where its size word sits.
fn open(out: &mut Vec<u8>, kind: &[u8; 4]) -> usize {
    let at = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(kind);
    at
}

/// Writes the size of the box that started at `at`.
fn close(out: &mut [u8], at: usize) {
    let size = (out.len() - at) as u32;
    out[at..at + 4].copy_from_slice(&size.to_be_bytes());
}

/// One fragment: a `moof` whose `traf`s point into the `mdat` that follows.
///
/// Written straight into `out`, with no temporary of its own, so that
/// regenerating a fragment inside a measured region allocates nothing and
/// what the region reports is the library's memory. `sites` is scratch: it
/// collects where each `trun`'s data offset sits, which is only known once
/// the `moof` is complete.
fn fragment(tracks: &[Track], index: u64, out: &mut Vec<u8>, sites: &mut Vec<usize>) {
    let moof = open(out, b"moof");
    let mfhd = open(out, b"mfhd");
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(index as u32 + 1).to_be_bytes());
    close(out, mfhd);
    for (i, track) in tracks.iter().enumerate() {
        let traf = open(out, b"traf");
        // tfhd: default-base-is-moof, and defaults for duration, size and
        // flags (ISO/IEC 14496-12 8.8.7).
        let tfhd = open(out, b"tfhd");
        out.extend_from_slice(&[0x00, 0x02, 0x00, 0x38]);
        out.extend_from_slice(&(i as u32 + 1).to_be_bytes());
        out.extend_from_slice(&track.duration.to_be_bytes());
        out.extend_from_slice(&track.size.to_be_bytes());
        out.extend_from_slice(&track.flags(0).to_be_bytes());
        close(out, tfhd);
        let tfdt = open(out, b"tfdt");
        out.extend_from_slice(&[0x01, 0, 0, 0]);
        let base = index * u64::from(track.samples) * u64::from(track.duration);
        out.extend_from_slice(&base.to_be_bytes());
        close(out, tfdt);
        // trun: a data offset, and sample flags only where they differ
        // from the track fragment's default.
        let per_sample = track.gop > 1;
        let trun = open(out, b"trun");
        out.extend_from_slice(&[0x00, 0x00, if per_sample { 0x04 } else { 0x00 }, 0x01]);
        out.extend_from_slice(&track.samples.to_be_bytes());
        sites.push(out.len());
        out.extend_from_slice(&[0; 4]);
        if per_sample {
            let first = index * u64::from(track.samples);
            for sample in 0..u64::from(track.samples) {
                out.extend_from_slice(&track.flags(first + sample).to_be_bytes());
            }
        }
        close(out, trun);
        close(out, traf);
    }
    close(out, moof);
    let moof_len = (out.len() - moof) as u32;
    let payload: u32 = tracks.iter().map(|t| t.samples * t.size).sum();
    out.extend_from_slice(&(payload + 8).to_be_bytes());
    out.extend_from_slice(b"mdat");
    // Each track's samples follow the ones before them; the data offsets
    // are measured from the start of the `moof` (`default-base-is-moof`).
    let mut at = moof_len + 8;
    for (track, site) in tracks.iter().zip(sites.iter()) {
        out[*site..*site + 4].copy_from_slice(&at.to_be_bytes());
        at += track.samples * track.size;
    }
    for track in tracks {
        let first = index * u64::from(track.samples);
        for sample in 0..u64::from(track.samples) {
            track.sample(first + sample, out);
        }
    }
}
