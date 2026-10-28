//! The one fixed copy buffer through which samples are copied one at a time,
//! which also serves as a read window.

use crate::ExtractError;
use crate::moov::overflow;
use core::cell::RefCell;
use coreav_isobmff::{Error, Source};
use std::io::Write;

/// Bytes copied per read.
pub(crate) const COPY_BUFFER: usize = 64 << 10;

/// The copy buffer, holding the source bytes from `start`. Its memory is
/// reserved, not zeroed: each refill appends the source's bytes
/// ([`Source::read_append`]), so the buffer is written once per refill.
/// Zeroing 64 KiB that the first refill overwrites at once cost as many
/// instructions again as a small clip, depending on whether the allocator
/// had fresh memory to hand (`docs/design/benchmarks.md`).
pub(crate) struct Window {
    buffer: Vec<u8>,
    capacity: usize,
    start: u64,
}

impl Window {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::new(),
            capacity,
            start: 0,
        }
    }

    /// Forgets the bytes held, before copying from another source
    /// (`crate::join`).
    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Writes the `size` bytes at `offset` of `source` to `out`, from the
    /// window when they lie in it, else after refilling it from `offset`
    /// with as much of the file as fits, ending the window at the end of
    /// the file rather than starting it at `offset` when fewer bytes than
    /// the window are left. Every refill then reads a whole window, or the
    /// file up to its end, which a [`BufferedSource`] hands straight to the
    /// window instead of keeping a second copy. A
    /// sample larger than the window is copied through it piece by piece.
    ///
    /// [`BufferedSource`]: coreav_isobmff::BufferedSource
    pub(crate) fn copy<S: Source, W: Write>(
        &mut self,
        source: &mut S,
        offset: u64,
        size: u32,
        out: &mut W,
    ) -> Result<(), ExtractError> {
        let len = usize::try_from(size).map_err(|_| overflow())?;
        if len > self.capacity {
            let result = copy(source, offset, size, self.capacity, &mut self.buffer, out);
            self.buffer.clear();
            return result;
        }
        let at = match holds(self.start, self.buffer.len(), offset, len) {
            Some(at) => at,
            None => {
                let (start, fill) = refill_range(self.capacity, offset, size, source.len()?)?;
                if let Err(error) = refill(source, start, fill, &mut self.buffer) {
                    self.buffer.clear();
                    return Err(error);
                }
                self.start = start;
                usize::try_from(offset.saturating_sub(start)).map_err(|_| overflow())?
            }
        };
        let bytes = self
            .buffer
            .get(at..at.checked_add(len).ok_or_else(overflow)?)
            .ok_or_else(overflow)?;
        out.write_all(bytes)?;
        Ok(())
    }
}

/// Where a window holding `held` bytes from `start` holds the `len` bytes
/// at `offset`, if it holds them all.
fn holds(start: u64, held: usize, offset: u64, len: usize) -> Option<usize> {
    offset
        .checked_sub(start)
        .and_then(|at| usize::try_from(at).ok())
        .filter(|at| at.checked_add(len).is_some_and(|end| end <= held))
}

/// The refill of a window of `capacity` bytes for the `size` bytes at
/// `offset` of a file of `total` bytes, as a start and a length: the window
/// [start, start + fill) holds the sample, and it ends at the end of the
/// file or holds a whole window from the sample on.
fn refill_range(
    capacity: usize,
    offset: u64,
    size: u32,
    total: u64,
) -> Result<(u64, usize), ExtractError> {
    offset
        .checked_add(u64::from(size))
        .filter(|end| *end <= total)
        .ok_or(Error::Truncated { offset })?;
    let capacity = u64::try_from(capacity).map_err(|_| overflow())?;
    let start = offset.min(total.saturating_sub(capacity));
    let fill =
        usize::try_from(total.saturating_sub(start).min(capacity)).map_err(|_| overflow())?;
    Ok((start, fill))
}

/// The bytes a copy pass through a [`Window`] reads, listed before the pass
/// from the same samples in the same order, by the same decisions and
/// without reading (#31): each refill, and each sample larger than the
/// window, as one region, regions that touch or overlap joined. A remote
/// source given them as advice ([`Source::will_read_regions`]) can fetch
/// them and skip the bytes between them, which in a file that interleaves
/// tracks are the other tracks' samples. The list is given up (`None`) when
/// it would hold more than `max` regions, when a region starts before the
/// one before it (the pass would read backwards), when memory for it cannot
/// be reserved, or when a sample lies past the end of the file (the pass
/// fails there anyway).
pub(crate) struct ReadPlan {
    capacity: usize,
    total: u64,
    start: u64,
    held: usize,
    max: usize,
    regions: Option<Vec<(u64, u64)>>,
}

impl ReadPlan {
    pub(crate) fn new(capacity: usize, total: u64, max: usize) -> Self {
        Self {
            capacity,
            total,
            start: 0,
            held: 0,
            max,
            regions: Some(Vec::new()),
        }
    }

    /// Adds the reads of copying the `size` bytes at `offset`.
    pub(crate) fn push(&mut self, offset: u64, size: u32) {
        if self.regions.is_none() {
            return;
        }
        let Ok(len) = usize::try_from(size) else {
            self.regions = None;
            return;
        };
        if len > self.capacity {
            // Copied piece by piece through the window, which is then empty.
            self.held = 0;
            self.add(offset, u64::from(size));
            return;
        }
        if holds(self.start, self.held, offset, len).is_some() {
            return;
        }
        let Ok((start, fill)) = refill_range(self.capacity, offset, size, self.total) else {
            self.regions = None;
            return;
        };
        self.start = start;
        self.held = fill;
        self.add(start, fill as u64);
    }

    fn add(&mut self, offset: u64, len: u64) {
        let Some(regions) = self.regions.as_mut() else {
            return;
        };
        let end = offset.saturating_add(len);
        if let Some(last) = regions.last_mut() {
            if offset < last.0 {
                self.regions = None;
                return;
            }
            let last_end = last.0.saturating_add(last.1);
            if offset <= last_end {
                last.1 = end.max(last_end).saturating_sub(last.0);
                return;
            }
        }
        if regions.len() >= self.max || regions.try_reserve(1).is_err() {
            self.regions = None;
            return;
        }
        regions.push((offset, len));
    }

    /// The regions, or `None` if the list was given up.
    pub(crate) fn finish(self) -> Option<Vec<(u64, u64)>> {
        self.regions
    }
}

/// Copies `size` bytes at `offset` from `source` to `out` through `buffer`.
fn copy<S: Source, W: Write>(
    source: &mut S,
    offset: u64,
    size: u32,
    capacity: usize,
    buffer: &mut Vec<u8>,
    out: &mut W,
) -> Result<(), ExtractError> {
    let mut at = offset;
    let mut left = u64::from(size);
    while left > 0 {
        let step = usize::try_from(left).map_or(capacity, |l| l.min(capacity));
        refill(source, at, step, buffer)?;
        out.write_all(buffer)?;
        let step = u64::try_from(step).map_err(|_| overflow())?;
        at = at.checked_add(step).ok_or_else(overflow)?;
        left = left.saturating_sub(step);
    }
    Ok(())
}

/// Fills `buffer` with the `len` bytes at `offset` of `source`. The bytes
/// it already holds are read over in place (one read, as before the window
/// stopped being zeroed); only what it lacks is appended, into memory
/// reserved, not zeroed. On failure `buffer` may hold any of the bytes.
fn refill<S: Source>(
    source: &mut S,
    offset: u64,
    len: usize,
    buffer: &mut Vec<u8>,
) -> Result<(), ExtractError> {
    buffer.truncate(len);
    let have = buffer.len();
    if have > 0 {
        source.read_exact_at(offset, buffer)?;
    }
    if len > have {
        buffer
            .try_reserve_exact(len - have)
            .map_err(|_| overflow())?;
        let rest = u64::try_from(have).map_err(|_| overflow())?;
        source.read_append(
            offset.checked_add(rest).ok_or_else(overflow)?,
            len - have,
            buffer,
        )?;
    }
    Ok(())
}

/// A source shared by the listings of a copy pass, one per track. Each read
/// borrows it only for its own duration, so the borrow never fails.
pub(crate) struct Shared<'a, S>(pub(crate) &'a RefCell<S>);

impl<S: Source> Source for Shared<'_, S> {
    fn len(&mut self) -> Result<u64, Error> {
        self.0.try_borrow_mut().map_err(|_| busy())?.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.0
            .try_borrow_mut()
            .map_err(|_| busy())?
            .read_exact_at(offset, buf)
    }

    fn will_read(&mut self, offset: u64, len: u64) {
        if let Ok(mut source) = self.0.try_borrow_mut() {
            source.will_read(offset, len);
        }
    }

    fn release(&mut self) {
        if let Ok(mut source) = self.0.try_borrow_mut() {
            source.release();
        }
    }

    fn read_append(&mut self, offset: u64, len: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        self.0
            .try_borrow_mut()
            .map_err(|_| busy())?
            .read_append(offset, len, buf)
    }
}

fn busy() -> Error {
    Error::Unsupported {
        reason: "the source is already being read",
    }
}

#[cfg(test)]
mod tests {
    use super::ReadPlan;

    #[test]
    fn a_read_plan_lists_refills_and_gives_up_rather_than_mislead() {
        // A refill near the end of the file starts before its sample and
        // joins the region before it.
        let mut plan = ReadPlan::new(64, 100, 8);
        plan.push(0, 10);
        plan.push(20, 10);
        plan.push(70, 10);
        assert_eq!(plan.finish(), Some(vec![(0, 100)]));
        // Separate refills stay apart; a sample larger than the window is
        // one region and empties the window, and regions that touch join.
        let mut plan = ReadPlan::new(64, 10_000, 8);
        plan.push(1000, 10);
        plan.push(2000, 100);
        plan.push(2100, 10);
        plan.push(2110, 10);
        assert_eq!(plan.finish(), Some(vec![(1000, 64), (2000, 164)]));
        // A read backwards, more regions than allowed, a sample past the
        // end: no list.
        let mut plan = ReadPlan::new(64, 10_000, 8);
        plan.push(1000, 10);
        plan.push(100, 10);
        assert_eq!(plan.finish(), None);
        let mut plan = ReadPlan::new(64, 10_000, 2);
        for at in [0, 1000, 2000] {
            plan.push(at, 10);
        }
        assert_eq!(plan.finish(), None);
        let mut plan = ReadPlan::new(64, 100, 8);
        plan.push(95, 10);
        assert_eq!(plan.finish(), None);
    }
}
