//! Output sample tables (ISO/IEC 14496-12 8.6 and 8.7), built from the
//! source packets before any sample is copied, so the movie box can come
//! first.
//!
//! Memory: the tables are held in their serialized
//! form and borrowed by the boxes, in fixed blocks so that growing a table
//! never leaves more than one partly used block per table, and that block is
//! trimmed once the first pass ends. Chunks are not stored: their sample
//! counts and byte sizes are recomputed from the durations and sizes when
//! `stsc` and the chunk offsets are written. Sample sizes are stored only
//! once two of them differ, and composition offsets only once
//! one of them is not zero.

use crate::boxes::{Body, Node};
use coreav_isobmff::{Error, LimitKind};
use std::borrow::Cow;

/// Bytes of table memory per sample size, decode-time run, chunk, sync
/// entry and composition-offset run; all count against `max_table_bytes`,
/// the decoded sample-table limit. A size and a run are kept in
/// their `stsz` and `stts` form (4 and 8 bytes); a chunk gives at most one
/// `stsc` entry (12) and one `co64` entry (8) while the boxes are written; a
/// sync sample is kept in its `stss` form (4), a composition-offset run in
/// its `ctts` form (8).
const SIZE_ENTRY: u64 = 4;
const STTS_ENTRY: u64 = 8;
const CHUNK_ENTRY: u64 = 20;
const SYNC_ENTRY: u64 = 4;
const CTTS_ENTRY: u64 = 8;

/// Bytes per storage block; a multiple of every entry size (4 and 8).
const BLOCK: usize = 16 << 10;

/// A byte table kept in fixed blocks: appending never moves stored bytes,
/// and only the last block is partly used.
#[derive(Default)]
pub(crate) struct Blocks {
    blocks: Vec<Vec<u8>>,
    len: usize,
}

impl Blocks {
    /// Bytes stored.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Appends `bytes` (an entry of 4 or 8 bytes, never split by a block
    /// boundary because the block size is a multiple of it).
    fn push(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let room = self
            .blocks
            .last()
            .map_or(0, |b| b.capacity().saturating_sub(b.len()));
        if room < bytes.len() {
            let mut block = Vec::new();
            block.try_reserve_exact(BLOCK).map_err(|_| table_limit())?;
            self.blocks.try_reserve(1).map_err(|_| table_limit())?;
            self.blocks.push(block);
        }
        let block = self.blocks.last_mut().ok_or_else(table_limit)?;
        block.extend_from_slice(bytes);
        self.len = self.len.checked_add(bytes.len()).ok_or_else(table_limit)?;
        Ok(())
    }

    /// The `n` bytes at `at`, when they lie in one block.
    fn get(&self, at: usize, n: usize) -> Option<&[u8]> {
        let block = self.blocks.get(at / BLOCK)?;
        let start = at % BLOCK;
        block.get(start..start.checked_add(n)?)
    }

    fn get_mut(&mut self, at: usize, n: usize) -> Option<&mut [u8]> {
        let block = self.blocks.get_mut(at / BLOCK)?;
        let start = at % BLOCK;
        block.get_mut(start..start.checked_add(n)?)
    }

    /// The stored bytes, block by block.
    pub(crate) fn slices(&self) -> impl Iterator<Item = &[u8]> {
        self.blocks.iter().map(Vec::as_slice)
    }

    /// Releases the unused part of the last block and of the block list.
    fn trim(&mut self) {
        if let Some(last) = self.blocks.last_mut() {
            last.shrink_to_fit();
        }
        self.blocks.shrink_to_fit();
    }
}

/// One output chunk: consecutive samples stored back to back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chunk {
    pub(crate) samples: u32,
    pub(crate) bytes: u64,
    /// Decode time of the chunk's first sample, in media ticks from the
    /// track's first sample.
    pub(crate) start: u64,
}

/// The sample tables of the output track.
pub(crate) struct Tables {
    limit: u64,
    used: u64,
    /// Media timescale; also the ticks after which a chunk is closed.
    timescale: u32,
    chunk_ticks: u64,
    /// Bytes after which a chunk is closed.
    chunk_bytes: u64,
    /// `stts` entries (sample count, sample delta), big endian.
    stts: Blocks,
    runs: u32,
    /// Delta and sample count of the last run.
    last_run: Option<(u32, u32)>,
    /// Every sample size, big endian, in order; empty while `constant` holds.
    sizes: Blocks,
    /// The size every sample has so far, `None` once two differ.
    constant: Option<u32>,
    chunks: u32,
    open_ticks: u64,
    open_samples: u32,
    open_bytes: u64,
    /// Sync sample numbers (1-based, big endian); only kept once a non-sync
    /// sample is seen.
    sync: Blocks,
    all_sync: bool,
    /// `ctts` entries (sample count, offset), big endian; empty while every
    /// offset is 0. An offset is kept as its 32 stored bits.
    ctts: Blocks,
    ctts_runs: u32,
    /// Offset and sample count of the last composition-offset run.
    last_offset: Option<(u32, u32)>,
    /// Some offset is not 0.
    offsets: bool,
    /// Some offset is negative (`ctts` version 1).
    negative: bool,
    /// Some offset is above `i32::MAX`, which only an unsigned version 0
    /// table stores.
    unsigned: bool,
    samples: u32,
    pub(crate) duration: u64,
    pub(crate) data_bytes: u64,
}

impl Tables {
    /// Empty tables for a track of `timescale` ticks per second, holding at
    /// most `limit` bytes. Chunks hold about one second of audio.
    pub(crate) fn new(timescale: u32, limit: u64) -> Self {
        Self::with_chunk_bytes(timescale, limit, u64::MAX)
    }

    /// The same, with chunks also closed once they hold `chunk_bytes`.
    pub(crate) fn with_chunk_bytes(timescale: u32, limit: u64, chunk_bytes: u64) -> Self {
        Self {
            limit,
            used: 0,
            timescale,
            chunk_ticks: u64::from(timescale.max(1)),
            chunk_bytes,
            stts: Blocks::default(),
            runs: 0,
            last_run: None,
            sizes: Blocks::default(),
            constant: None,
            chunks: 0,
            open_ticks: 0,
            open_samples: 0,
            open_bytes: 0,
            sync: Blocks::default(),
            all_sync: true,
            ctts: Blocks::default(),
            ctts_runs: 0,
            last_offset: None,
            offsets: false,
            negative: false,
            unsigned: false,
            samples: 0,
            duration: 0,
            data_bytes: 0,
        }
    }

    pub(crate) fn samples(&self) -> u32 {
        self.samples
    }

    /// Bytes charged against the table limit so far.
    pub(crate) fn used(&self) -> u64 {
        self.used
    }

    /// Media timescale.
    pub(crate) fn timescale(&self) -> u32 {
        self.timescale
    }

    /// Number of chunks.
    pub(crate) fn chunk_count(&self) -> u32 {
        self.chunks
    }

    fn charge(&mut self, bytes: u64) -> Result<(), Error> {
        self.used = self.used.saturating_add(bytes);
        if self.used > self.limit {
            return Err(table_limit());
        }
        Ok(())
    }

    /// Appends one sample without a composition offset.
    pub(crate) fn push(&mut self, size: u32, duration: u32, sync: bool) -> Result<(), Error> {
        self.push_sample(size, duration, sync, 0)
    }

    /// Appends one sample with its composition offset (composition time
    /// minus decode time, ISO/IEC 14496-12 8.6.1.3). Offsets are copied as
    /// stored: signed ones in a version 1 table, and unsigned ones up to
    /// `u32::MAX` in a version 0 table, as a source's version 0 table holds
    /// them.
    pub(crate) fn push_sample(
        &mut self,
        size: u32,
        duration: u32,
        sync: bool,
        offset: i64,
    ) -> Result<(), Error> {
        let negative = offset < 0;
        let bits = if negative {
            i32::try_from(offset).map(|v| u32::from_be_bytes(v.to_be_bytes()))
        } else {
            u32::try_from(offset)
        }
        .map_err(|_| Error::Unsupported {
            reason: "composition offset outside 32 bits",
        })?;
        let unsigned = offset > i64::from(i32::MAX);
        if (negative && self.unsigned) || (unsigned && self.negative) {
            return Err(Error::Unsupported {
                reason: "signed and unsigned composition offsets in one track",
            });
        }
        self.negative |= negative;
        self.unsigned |= unsigned;
        self.push_offset(bits)?;
        let number = self.samples.checked_add(1).ok_or(Error::Unsupported {
            reason: "more than 2^32 - 1 samples in one track",
        })?;
        self.charge(SIZE_ENTRY)?;
        match self.constant {
            _ if self.samples == 0 => self.constant = Some(size),
            Some(c) if c == size => {}
            Some(c) => {
                // The first differing size: list every size from now on.
                for _ in 0..self.samples {
                    self.sizes.push(&c.to_be_bytes())?;
                }
                self.sizes.push(&size.to_be_bytes())?;
                self.constant = None;
            }
            None => self.sizes.push(&size.to_be_bytes())?,
        }
        match self.last_run {
            Some((delta, count)) if delta == duration && count < u32::MAX => {
                let count = count.saturating_add(1);
                let at = self.stts.len().saturating_sub(8);
                self.stts
                    .get_mut(at, 4)
                    .ok_or_else(table_limit)?
                    .copy_from_slice(&count.to_be_bytes());
                self.last_run = Some((delta, count));
            }
            _ => {
                self.charge(STTS_ENTRY)?;
                self.stts
                    .push(&[1u32.to_be_bytes(), duration.to_be_bytes()].concat())?;
                self.runs = self.runs.saturating_add(1);
                self.last_run = Some((duration, 1));
            }
        }
        if !sync && self.all_sync {
            // Back-fill the sync samples seen so far: every one of them.
            self.all_sync = false;
            self.charge(SYNC_ENTRY.saturating_mul(u64::from(self.samples)))?;
            for n in 1..number {
                self.sync.push(&n.to_be_bytes())?;
            }
        }
        if sync && !self.all_sync {
            self.charge(SYNC_ENTRY)?;
            self.sync.push(&number.to_be_bytes())?;
        }
        let joins = opens_no_chunk(
            (self.open_ticks, self.chunk_ticks),
            (self.open_bytes, self.chunk_bytes),
            self.open_samples,
        );
        if self.chunks > 0 && joins {
            self.open_samples += 1;
        } else {
            self.charge(CHUNK_ENTRY)?;
            self.chunks = self.chunks.saturating_add(1);
            self.open_samples = 1;
            self.open_ticks = 0;
            self.open_bytes = 0;
        }
        self.open_ticks = self.open_ticks.saturating_add(u64::from(duration));
        self.open_bytes = self.open_bytes.saturating_add(u64::from(size));
        self.samples = number;
        self.duration = self
            .duration
            .checked_add(u64::from(duration))
            .ok_or(Error::Overflow { offset: 0 })?;
        self.data_bytes = self
            .data_bytes
            .checked_add(u64::from(size))
            .ok_or(Error::Overflow { offset: 0 })?;
        Ok(())
    }

    /// Records the composition offset of the next sample, before the
    /// sample itself is counted.
    fn push_offset(&mut self, offset: u32) -> Result<(), Error> {
        if offset != 0 && !self.offsets {
            // The first offset that is not 0: one run of zeros for every
            // sample so far.
            self.offsets = true;
            if self.samples > 0 {
                self.charge(CTTS_ENTRY)?;
                self.ctts
                    .push(&[self.samples.to_be_bytes(), 0u32.to_be_bytes()].concat())?;
                self.ctts_runs = 1;
                self.last_offset = Some((0, self.samples));
            }
        }
        if !self.offsets {
            return Ok(());
        }
        match self.last_offset {
            Some((last, count)) if last == offset && count < u32::MAX => {
                let count = count.saturating_add(1);
                let at = self.ctts.len().saturating_sub(8);
                self.ctts
                    .get_mut(at, 4)
                    .ok_or_else(table_limit)?
                    .copy_from_slice(&count.to_be_bytes());
                self.last_offset = Some((offset, count));
            }
            _ => {
                self.charge(CTTS_ENTRY)?;
                self.ctts
                    .push(&[1u32.to_be_bytes(), offset.to_be_bytes()].concat())?;
                self.ctts_runs = self.ctts_runs.saturating_add(1);
                self.last_offset = Some((offset, 1));
            }
        }
        Ok(())
    }

    /// Ends the first pass: releases the unused part of every table.
    pub(crate) fn finish(&mut self) {
        self.stts.trim();
        self.sizes.trim();
        self.sync.trim();
        self.ctts.trim();
    }

    /// Size of sample `index` (0-based).
    pub(crate) fn size(&self, index: u32) -> Option<u32> {
        if index >= self.samples {
            return None;
        }
        if let Some(c) = self.constant {
            return Some(c);
        }
        let at = usize::try_from(index).ok()?.checked_mul(4)?;
        let bytes = self.sizes.get(at, 4)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    }

    /// Every chunk in order, recomputed from the durations and sizes with the
    /// rule of [`Tables::push`].
    pub(crate) fn chunks(&self) -> ChunkIter<'_> {
        ChunkIter {
            tables: self,
            run: 0,
            left_in_run: 0,
            delta: 0,
            index: 0,
            ticks: 0,
            pending: None,
        }
    }

    /// Calls `f` with every chunk in order.
    fn each_chunk(&self, mut f: impl FnMut(Chunk) -> Result<(), Error>) -> Result<(), Error> {
        for chunk in self.chunks() {
            f(chunk?)?;
        }
        Ok(())
    }

    /// Whether chunk offsets need 64 bits for data that ends at `end`.
    pub(crate) fn needs_co64(end: u64) -> bool {
        end > u64::from(u32::MAX)
    }

    /// Number of `stsc` entries: one per change of samples per chunk.
    fn stsc_entries(&self) -> Result<usize, Error> {
        let mut entries = 0usize;
        let mut previous = None;
        self.each_chunk(|chunk| {
            if previous != Some(chunk.samples) {
                previous = Some(chunk.samples);
                entries = entries.saturating_add(1);
            }
            Ok(())
        })?;
        Ok(entries)
    }

    /// `stsc` at its exact size.
    fn stsc(&self, entries: usize) -> Result<Vec<u8>, Error> {
        let mut stsc = Vec::new();
        stsc.try_reserve_exact(entries.saturating_mul(12))
            .map_err(|_| table_limit())?;
        let mut number = 0u32;
        let mut previous = None;
        self.each_chunk(|chunk| {
            number = number.checked_add(1).ok_or(Error::Overflow { offset: 0 })?;
            if previous != Some(chunk.samples) {
                previous = Some(chunk.samples);
                stsc.extend_from_slice(&number.to_be_bytes());
                stsc.extend_from_slice(&chunk.samples.to_be_bytes());
                stsc.extend_from_slice(&1u32.to_be_bytes());
            }
            Ok(())
        })?;
        Ok(stsc)
    }

    /// An empty chunk-offset table with room for every chunk.
    pub(crate) fn offset_table(&self, co64: bool) -> Result<Vec<u8>, Error> {
        let chunks = usize::try_from(self.chunks).map_err(|_| table_limit())?;
        let mut offsets = Vec::new();
        offsets
            .try_reserve_exact(chunks.saturating_mul(if co64 { 8 } else { 4 }))
            .map_err(|_| table_limit())?;
        Ok(offsets)
    }

    /// The `stbl` children after `stsd`: `stts`, `ctts` when some
    /// composition offset is not 0, `stsc`, `stsz`, the chunk offsets
    /// `offsets` (built with [`Tables::offset_table`] and [`push_offset`]),
    /// and `stss` when some sample is not a sync sample. The decode times,
    /// composition offsets, sample sizes and sync samples are borrowed, not
    /// copied; `stsc` is built at its exact size.
    pub(crate) fn boxes(&self, offsets: Vec<u8>, co64: bool) -> Result<Vec<Node<'_>>, Error> {
        let entries = self.stsc_entries()?;
        let stsc = self.stsc(entries)?;
        self.assemble(
            entries,
            Body::Bytes(Cow::Owned(stsc)),
            Body::Bytes(Cow::Owned(offsets)),
            co64,
        )
    }

    /// The same boxes with the right sizes but `stsc` and the chunk offsets
    /// left unbuilt (zeros): for laying out the file without the memory of
    /// a second set of tables.
    pub(crate) fn sized_boxes(&self, co64: bool) -> Result<Vec<Node<'_>>, Error> {
        let entries = self.stsc_entries()?;
        let chunks = usize::try_from(self.chunks).map_err(|_| table_limit())?;
        let len = |n: usize, size: usize| {
            u64::try_from(n.saturating_mul(size)).map_err(|_| table_limit())
        };
        self.assemble(
            entries,
            Body::Zeros(len(entries, 12)?),
            Body::Zeros(len(chunks, if co64 { 8 } else { 4 })?),
            co64,
        )
    }

    fn assemble<'a>(
        &'a self,
        stsc_entries: usize,
        stsc: Body<'a>,
        offsets: Body<'a>,
        co64: bool,
    ) -> Result<Vec<Node<'a>>, Error> {
        let runs = u32::try_from(stsc_entries).map_err(|_| Error::Overflow { offset: 0 })?;
        // A `sample_size` of 0 means the sizes are listed, so samples that
        // are all empty are listed too (14496-12 8.7.3.2).
        let (sample_size, sizes) = match self.constant {
            Some(size) if size != 0 => (size, Body::Bytes(Cow::Borrowed(&[][..]))),
            Some(_) => {
                let len = usize::try_from(self.samples)
                    .ok()
                    .and_then(|n| n.checked_mul(4))
                    .ok_or_else(table_limit)?;
                let mut zeros = Vec::new();
                zeros.try_reserve_exact(len).map_err(|_| table_limit())?;
                zeros.resize(len, 0);
                (0, Body::Bytes(Cow::Owned(zeros)))
            }
            None => (0, Body::Blocks(&self.sizes)),
        };
        let mut stsz_head = sample_size.to_be_bytes().to_vec();
        stsz_head.extend_from_slice(&self.samples.to_be_bytes());
        let mut boxes = vec![Node::full_split(
            b"stts",
            0,
            0,
            &self.runs.to_be_bytes(),
            Body::Blocks(&self.stts),
        )];
        if self.offsets {
            // Version 1 stores signed offsets (14496-12 8.6.1.3).
            boxes.push(Node::full_split(
                b"ctts",
                u8::from(self.negative),
                0,
                &self.ctts_runs.to_be_bytes(),
                Body::Blocks(&self.ctts),
            ));
        }
        boxes.extend([
            Node::full_split(b"stsc", 0, 0, &runs.to_be_bytes(), stsc),
            Node::full_split(b"stsz", 0, 0, &stsz_head, sizes),
            Node::full_split(
                if co64 { b"co64" } else { b"stco" },
                0,
                0,
                &self.chunks.to_be_bytes(),
                offsets,
            ),
        ]);
        if !self.all_sync {
            let sync_count = u32::try_from(self.sync.len() / 4).map_err(|_| table_limit())?;
            boxes.push(Node::full_split(
                b"stss",
                0,
                0,
                &sync_count.to_be_bytes(),
                Body::Blocks(&self.sync),
            ));
        }
        Ok(boxes)
    }
}

/// Appends the offset of the chunk that starts at `at` to `offsets`.
pub(crate) fn push_offset(offsets: &mut Vec<u8>, at: u64, co64: bool) -> Result<(), Error> {
    if co64 {
        offsets.extend_from_slice(&at.to_be_bytes());
    } else {
        let compact = u32::try_from(at).map_err(|_| Error::Overflow { offset: 0 })?;
        offsets.extend_from_slice(&compact.to_be_bytes());
    }
    Ok(())
}

/// The chunks of a [`Tables`], in order.
pub(crate) struct ChunkIter<'a> {
    tables: &'a Tables,
    run: u32,
    left_in_run: u32,
    delta: u32,
    /// Next sample.
    index: u32,
    /// Decode time of the next sample.
    ticks: u64,
    /// The open chunk and the ticks it holds.
    pending: Option<(Chunk, u64)>,
}

impl ChunkIter<'_> {
    /// The next sample's size and duration, `None` after the last.
    fn next_sample(&mut self) -> Result<Option<(u64, u32)>, Error> {
        while self.left_in_run == 0 {
            if self.run >= self.tables.runs {
                return Ok(None);
            }
            let at = usize::try_from(self.run)
                .ok()
                .and_then(|r| r.checked_mul(8))
                .ok_or_else(table_limit)?;
            let entry = self.tables.stts.get(at, 8).ok_or_else(table_limit)?;
            let (count, delta) = entry.split_at(4);
            self.left_in_run = u32::from_be_bytes(count.try_into().map_err(|_| table_limit())?);
            self.delta = u32::from_be_bytes(delta.try_into().map_err(|_| table_limit())?);
            self.run = self.run.saturating_add(1);
        }
        let size = u64::from(self.tables.size(self.index).ok_or_else(table_limit)?);
        self.left_in_run = self.left_in_run.saturating_sub(1);
        self.index = self.index.saturating_add(1);
        Ok(Some((size, self.delta)))
    }

    fn step(&mut self) -> Result<Option<Chunk>, Error> {
        loop {
            let Some((size, delta)) = self.next_sample()? else {
                return Ok(self.pending.take().map(|(chunk, _)| chunk));
            };
            let start = self.ticks;
            self.ticks = self.ticks.saturating_add(u64::from(delta));
            match self.pending.as_mut() {
                Some((chunk, ticks))
                    if opens_no_chunk(
                        (*ticks, self.tables.chunk_ticks),
                        (chunk.bytes, self.tables.chunk_bytes),
                        chunk.samples,
                    ) =>
                {
                    chunk.samples += 1;
                    chunk.bytes = chunk.bytes.saturating_add(size);
                    *ticks = ticks.saturating_add(u64::from(delta));
                }
                _ => {
                    let next = Chunk {
                        samples: 1,
                        bytes: size,
                        start,
                    };
                    let closed = self.pending.replace((next, u64::from(delta)));
                    if let Some((chunk, _)) = closed {
                        return Ok(Some(chunk));
                    }
                }
            }
        }
    }
}

impl Iterator for ChunkIter<'_> {
    type Item = Result<Chunk, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.step().transpose()
    }
}

/// Whether the next sample joins the open chunk: it holds less than one
/// second, fewer bytes than the chunk size and fewer than 2^32 - 1 samples.
/// Each pair is (held, most).
fn opens_no_chunk(ticks: (u64, u64), bytes: (u64, u64), open_samples: u32) -> bool {
    ticks.0 < ticks.1 && bytes.0 < bytes.1 && open_samples < u32::MAX
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

    fn bytes(nodes: &[Node]) -> Vec<Vec<u8>> {
        nodes
            .iter()
            .map(|n| {
                let mut out = Vec::new();
                n.write(&mut out).unwrap();
                out
            })
            .collect()
    }

    fn chunks(t: &Tables) -> Vec<Chunk> {
        t.chunks().map(Result::unwrap).collect()
    }

    /// The boxes with the chunks stored back to back from `start`.
    fn boxes(t: &Tables, start: u64, co64: bool) -> Result<Vec<Node<'_>>, Error> {
        let mut offsets = t.offset_table(co64)?;
        let mut at = start;
        for chunk in t.chunks() {
            push_offset(&mut offsets, at, co64)?;
            at = at
                .checked_add(chunk?.bytes)
                .ok_or(Error::Overflow { offset: 0 })?;
        }
        t.boxes(offsets, co64)
    }

    #[test]
    fn runs_chunks_and_constant_sizes() {
        let mut t = Tables::new(4, 1 << 20);
        for _ in 0..5 {
            t.push(10, 2, true).unwrap();
        }
        t.push(10, 1, true).unwrap();
        assert_eq!(t.samples(), 6);
        assert_eq!(t.duration, 11);
        // Constant sizes are not stored.
        assert_eq!(t.sizes.len(), 0);
        assert_eq!(t.size(5), Some(10));
        assert_eq!(t.size(6), None);
        // Chunks close once they hold 4 ticks: 2+2, 2+2, 2+1.
        let chunk = |start| Chunk {
            samples: 2,
            bytes: 20,
            start,
        };
        assert_eq!(chunks(&t), vec![chunk(0), chunk(4), chunk(8)]);
        assert_eq!(t.chunk_count(), 3);
        let out = bytes(&boxes(&t, 100, false).unwrap());
        assert_eq!(
            &out[0][12..],
            &[0, 0, 0, 2, 0, 0, 0, 5, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 1]
        );
        assert_eq!(
            &out[1][12..],
            &[0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1]
        );
        assert_eq!(&out[2][12..], &[0, 0, 0, 10, 0, 0, 0, 6]);
        assert_eq!(&out[3][4..8], b"stco");
        assert_eq!(
            &out[3][12..],
            &[0, 0, 0, 3, 0, 0, 0, 100, 0, 0, 0, 120, 0, 0, 0, 140]
        );
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn variable_sizes_sync_samples_and_co64() {
        let mut t = Tables::new(1000, 1 << 20);
        t.push(5, 10, true).unwrap();
        t.push(5, 10, true).unwrap();
        t.push(7, 10, false).unwrap();
        t.push(8, 10, true).unwrap();
        assert_eq!(t.size(1), Some(5));
        assert_eq!(t.size(2), Some(7));
        assert_eq!(t.size(4), None);
        let start = u64::from(u32::MAX) + 1;
        let out = bytes(&boxes(&t, start, true).unwrap());
        assert_eq!(
            &out[2][12..],
            &[
                0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 5, 0, 0, 0, 7, 0, 0, 0, 8
            ]
        );
        assert_eq!(&out[3][4..8], b"co64");
        assert_eq!(&out[3][12..], &[0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(&out[4][4..8], b"stss");
        assert_eq!(
            &out[4][12..],
            &[0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 4]
        );
    }

    #[test]
    fn tables_span_blocks() {
        // More sizes, runs and sync samples than one block holds.
        let n = 3 * BLOCK as u32;
        let mut t = Tables::new(48_000, 1 << 30);
        for i in 0..n {
            t.push(100 + i % 7, 1024 + i % 2, i % 3 != 1).unwrap();
        }
        t.finish();
        assert!(t.sizes.blocks.len() > 2 && t.stts.blocks.len() > 2 && t.sync.blocks.len() > 1);
        for i in [0, 1, 4095, 4096, 4097, n - 1] {
            assert_eq!(t.size(i), Some(100 + i % 7));
        }
        let all = chunks(&t);
        assert_eq!(all.len() as u32, t.chunk_count());
        assert_eq!(all.iter().map(|c| c.samples).sum::<u32>(), n);
        assert_eq!(all.iter().map(|c| c.bytes).sum::<u64>(), t.data_bytes);
        let out = bytes(&boxes(&t, 0, false).unwrap());
        // The sized boxes have the same sizes without building the chunk tables.
        for co64 in [false, true] {
            let sized: Vec<usize> = bytes(&t.sized_boxes(co64).unwrap())
                .iter()
                .map(Vec::len)
                .collect();
            let built: Vec<usize> = bytes(&boxes(&t, 0, co64).unwrap())
                .iter()
                .map(Vec::len)
                .collect();
            assert_eq!(sized, built);
        }
        assert_eq!(out[0].len(), 16 + 8 * n as usize);
        assert_eq!(out[2].len(), 20 + 4 * n as usize);
        assert_eq!(
            &out[2][20 + 4 * 4096..24 + 4 * 4096],
            &(100 + 4096 % 7u32).to_be_bytes()
        );
        // Trimmed: no block holds unused capacity.
        for table in [&t.sizes, &t.stts, &t.sync] {
            let capacity: usize = table.blocks.iter().map(Vec::capacity).sum();
            assert_eq!(capacity, table.len());
        }
    }

    #[test]
    fn composition_offsets() {
        let mut t = Tables::new(1000, 1 << 20);
        t.push_sample(5, 10, true, 0).unwrap();
        t.push_sample(5, 10, false, 0).unwrap();
        assert!(!t.offsets);
        t.push_sample(5, 10, false, 20).unwrap();
        t.push_sample(5, 10, false, 20).unwrap();
        t.push_sample(5, 10, false, -10).unwrap();
        let out = bytes(&boxes(&t, 0, false).unwrap());
        assert_eq!(&out[1][4..8], b"ctts");
        // Version 1 for the negative offset; a run of the two leading zeros.
        assert_eq!(out[1][8], 1);
        assert_eq!(
            &out[1][12..],
            &[
                0, 0, 0, 3, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 20, 0, 0, 0, 1, 255, 255,
                255, 246
            ]
        );
        // An unsigned offset after a negative one.
        assert!(
            t.push_sample(5, 10, false, i64::from(i32::MAX) + 1)
                .is_err()
        );
        assert!(
            t.push_sample(5, 10, false, i64::from(i32::MIN) - 1)
                .is_err()
        );
        // A version 0 table's large unsigned values are copied as stored.
        let mut v0 = Tables::new(1000, 1 << 20);
        v0.push_sample(5, 10, true, 0).unwrap();
        v0.push_sample(5, 10, false, 4_294_967_272).unwrap();
        let out = bytes(&boxes(&v0, 0, false).unwrap());
        assert_eq!(out[1][8], 0);
        assert_eq!(
            &out[1][12..],
            &[
                0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 255, 255, 255, 232
            ]
        );
        assert!(v0.push_sample(5, 10, false, -1).is_err());
        assert!(
            v0.push_sample(5, 10, false, i64::from(u32::MAX) + 1)
                .is_err()
        );
        // No offsets, no box.
        let mut plain = Tables::new(1000, 1 << 20);
        plain.push_sample(5, 10, true, 0).unwrap();
        let out = bytes(&boxes(&plain, 0, false).unwrap());
        assert!(out.iter().all(|b| &b[4..8] != b"ctts"));
    }

    #[test]
    fn chunks_close_at_a_byte_size() {
        let mut t = Tables::with_chunk_bytes(1000, 1 << 20, 100);
        for size in [60, 50, 10, 200, 1] {
            t.push(size, 1, true).unwrap();
        }
        let sizes: Vec<(u32, u64)> = chunks(&t).iter().map(|c| (c.samples, c.bytes)).collect();
        assert_eq!(sizes, [(2, 110), (2, 210), (1, 1)]);
        assert_eq!(t.chunk_count(), 3);
        // Without a byte size, one second per chunk.
        let mut t = Tables::new(1000, 1 << 20);
        for size in [60, 50, 10, 200, 1] {
            t.push(size, 1, true).unwrap();
        }
        assert_eq!(t.chunk_count(), 1);
    }

    #[test]
    fn empty_samples_are_listed() {
        let mut t = Tables::new(1000, 1 << 20);
        t.push(0, 10, true).unwrap();
        t.push(0, 10, true).unwrap();
        let out = bytes(&boxes(&t, 0, false).unwrap());
        assert_eq!(
            &out[2][12..],
            &[0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn table_bytes_are_limited() {
        let mut t = Tables::new(1000, 44);
        // 4 + 8 + 20 = 32 bytes, then 4 more per sample of the same run and chunk.
        t.push(1, 1, true).unwrap();
        t.push(2, 1, true).unwrap();
        t.push(3, 1, true).unwrap();
        t.push(4, 1, true).unwrap();
        assert!(matches!(
            t.push(5, 1, true),
            Err(Error::LimitExceeded {
                limit: LimitKind::TableBytes,
                ..
            })
        ));
    }

    #[test]
    fn stco_overflow_is_an_error() {
        let mut t = Tables::new(1, 1 << 20);
        t.push(10, 1, true).unwrap();
        t.push(10, 1, true).unwrap();
        assert!(boxes(&t, u64::from(u32::MAX) - 5, false).is_err());
    }
}
