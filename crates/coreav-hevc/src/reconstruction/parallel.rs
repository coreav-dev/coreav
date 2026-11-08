use super::{
    Cell, Frame, InterStats, Memory, ReclaimMemory, caught,
    filter::Ctu,
    syntax::{Checkpoint, Contexts, Qp},
};
use crate::{DecodeSummary, Error, IntraStats, Result, cabac::Cabac, dpb::Dpb, parser::SliceInput};
use core::ops::Range;
use std::sync::{
    Condvar, Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};

const STACK_BYTES: usize = 2 << 20;

#[derive(Default)]
pub(crate) struct Cache {
    frames: Vec<Frame>,
    peak: usize,
    #[cfg(test)]
    hits: usize,
}

impl Cache {
    #[cfg(test)]
    pub(crate) fn frames_for_test(&mut self) -> &mut Vec<Frame> {
        &mut self.frames
    }

    #[cfg(test)]
    pub(crate) fn hits(&self) -> usize {
        self.hits
    }
    pub(crate) fn clear(&mut self) {
        self.frames = Vec::new();
    }

    pub(crate) fn peak(&self) -> usize {
        self.peak
    }

    fn bytes(&self) -> Result<usize> {
        self.frames
            .iter()
            .try_fold(super::allocation_bytes(&self.frames)?, |total, frame| {
                total
                    .checked_add(frame.heap_bytes()?)
                    .ok_or(Error::Limit("reconstruction_memory"))
            })
    }

    fn take(&mut self, template: &Frame, count: usize) -> Result<Option<Vec<Frame>>> {
        let heap = template.heap_bytes()?;
        let reusable = self.frames.len() == count - 1
            && self.frames.capacity() == count - 1
            && self.frames.iter().all(|frame| {
                frame.info.width == template.info.width
                    && frame.info.height == template.info.height
                    && frame.info.bit_depth == template.info.bit_depth
                    && frame.info.ctb_log2 == template.info.ctb_log2
                    && frame.reference.is_some() == template.reference.is_some()
                    && frame.storage_lengths() == template.storage_lengths()
                    && frame.heap_bytes().is_ok_and(|bytes| bytes == heap)
            });
        if reusable {
            #[cfg(test)]
            {
                self.hits += 1;
            }
            Ok(Some(core::mem::take(&mut self.frames)))
        } else {
            self.clear();
            Ok(None)
        }
    }
}

impl ReclaimMemory for Cache {
    fn retained_bytes(&mut self, mandatory: usize, limit: usize) -> Result<usize> {
        let retained = self.bytes().unwrap_or(usize::MAX);
        let retained = if mandatory
            .checked_add(retained)
            .is_some_and(|bytes| bytes <= limit)
        {
            retained
        } else {
            self.clear();
            0
        };
        let physical = mandatory
            .checked_add(retained)
            .filter(|bytes| *bytes <= limit)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        self.peak = self.peak.max(physical);
        Ok(retained)
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) enum TestFault {
    None,
    Error,
    Panic,
    Deadline,
}

#[cfg(test)]
#[derive(Default)]
struct TestProgress {
    upper_paused: bool,
    lower_waiting: bool,
    lower_published: bool,
    released: bool,
}

#[cfg(test)]
pub(super) struct TestHook {
    fault: TestFault,
    delay: bool,
    state: Mutex<TestProgress>,
    ready: Condvar,
}

#[cfg(test)]
impl TestHook {
    pub(super) fn new(fault: TestFault, delay: bool) -> Self {
        Self {
            fault,
            delay,
            state: Mutex::new(TestProgress::default()),
            ready: Condvar::new(),
        }
    }

    pub(super) fn compute(&self, row: u32, column: u32) -> Result<()> {
        if (row, column) == (1, 1) {
            match self.fault {
                TestFault::None | TestFault::Deadline => {}
                TestFault::Error => return Err(Error::InvalidData("injected_row_error")),
                TestFault::Panic => std::panic::resume_unwind(Box::new(())),
            }
        }
        Ok(())
    }

    fn waiting(&self, row: usize, column: usize) -> Option<std::time::Instant> {
        if self.delay && (row, column) == (1, 1) {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .lower_waiting = true;
            self.ready.notify_all();
            if matches!(self.fault, TestFault::Deadline) {
                return Some(std::time::Instant::now() + std::time::Duration::from_millis(20));
            }
        }
        None
    }

    fn publication(&self, row: usize, column: usize) {
        if !self.delay {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if (row, column) == (0, 2) {
            state.upper_paused = true;
            self.ready.notify_all();
            while !state.released {
                state = self
                    .ready
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        } else if (row, column) == (1, 1) {
            state.lower_published = true;
            self.ready.notify_all();
        }
    }

    pub(super) fn release_after_wait(&self, delay: std::time::Duration) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (state, _) = self
            .ready
            .wait_timeout_while(state, std::time::Duration::from_secs(2), |s| {
                !s.upper_paused || !s.lower_waiting
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let reached = state.upper_paused && state.lower_waiting;
        let (mut state, _) = self
            .ready
            .wait_timeout_while(state, delay, |s| !s.lower_published)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let premature = state.lower_published;
        state.released = true;
        self.ready.notify_all();
        reached && !premature
    }
}

#[derive(Clone, Copy)]
struct Continuation {
    next_ctu: u32,
    closed: bool,
    contexts: Contexts,
    checkpoint: Option<Checkpoint>,
    qp: Qp,
    stats: IntraStats,
    inter: InterStats,
}

struct Outcome {
    result: Result<Continuation>,
    consumed: u64,
}

struct Band {
    planes: [Vec<u16>; 3],
    cells: Vec<Cell>,
    ctus: Vec<Ctu>,
    y: usize,
    height: usize,
    completed: usize,
    checkpoint: Option<Checkpoint>,
    outcome: Option<Outcome>,
}

struct Row {
    band: Mutex<Band>,
    ready: Condvar,
}

impl Row {
    fn lock(&self) -> MutexGuard<'_, Band> {
        self.band
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn reserve<T>(values: &mut Vec<T>, count: usize, memory: &mut Memory) -> Result<()> {
    memory.charge(
        count
            .checked_mul(core::mem::size_of::<T>())
            .ok_or(Error::Limit("reconstruction_memory"))?,
    )?;
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::Limit("picture_allocation"))?;
    memory.charge(
        (values.capacity() - count)
            .checked_mul(core::mem::size_of::<T>())
            .ok_or(Error::Limit("reconstruction_memory"))?,
    )
}

fn copy_rows<T: Copy>(
    source: &[T],
    destination: &mut [T],
    width: usize,
    source_y: usize,
    destination_y: usize,
    height: usize,
    columns: Range<usize>,
) -> Result<()> {
    for y in 0..height {
        let source = source
            .get((source_y + y) * width + columns.start..(source_y + y) * width + columns.end)
            .ok_or(Error::InvalidData("wpp_row_storage"))?;
        destination
            .get_mut(
                (destination_y + y) * width + columns.start
                    ..(destination_y + y) * width + columns.end,
            )
            .ok_or(Error::InvalidData("wpp_row_storage"))?
            .copy_from_slice(source);
    }
    Ok(())
}

impl Band {
    fn new(frame: &Frame, y: usize, ctb: usize, width: usize, memory: &mut Memory) -> Result<Self> {
        let height = ctb.min(frame.info.height as usize - y);
        let pixels = frame.info.width as usize;
        Ok(Self {
            planes: [
                memory.allocate(pixels * height, 0)?,
                memory.allocate(pixels * height / 4, 0)?,
                memory.allocate(pixels * height / 4, 0)?,
            ],
            cells: memory.allocate(
                pixels * height / 16,
                *frame
                    .cells
                    .first()
                    .ok_or(Error::InvalidData("wpp_row_storage"))?,
            )?,
            ctus: memory.allocate(width, Ctu::default())?,
            y,
            height,
            completed: 0,
            checkpoint: None,
            outcome: None,
        })
    }

    fn publish(&mut self, frame: &Frame, columns: Range<usize>) -> Result<()> {
        let ctb = 1usize << frame.info.ctb_log2;
        let width = frame.info.width as usize;
        for (component, (source, destination)) in
            frame.planes.iter().zip(&mut self.planes).enumerate()
        {
            let divisor = if component == 0 { 1 } else { 2 };
            copy_rows(
                source,
                destination,
                width / divisor,
                self.y / divisor,
                0,
                self.height / divisor,
                columns.start * ctb / divisor..(columns.end * ctb).min(width) / divisor,
            )?;
        }
        copy_rows(
            &frame.cells,
            &mut self.cells,
            width / 4,
            self.y / 4,
            0,
            self.height / 4,
            columns.start * ctb / 4..(columns.end * ctb).min(width) / 4,
        )?;
        let row = self.y / ctb;
        let ctus = width.div_ceil(ctb);
        self.ctus
            .get_mut(columns.clone())
            .ok_or(Error::InvalidData("wpp_row_storage"))?
            .copy_from_slice(
                frame
                    .filter_ctus
                    .get(row * ctus + columns.start..row * ctus + columns.end)
                    .ok_or(Error::InvalidData("wpp_row_storage"))?,
            );
        Ok(())
    }

    fn import(&self, frame: &mut Frame, columns: Range<usize>, lines: Range<usize>) -> Result<()> {
        let ctb = 1usize << frame.info.ctb_log2;
        let width = frame.info.width as usize;
        for (component, (source, destination)) in
            self.planes.iter().zip(&mut frame.planes).enumerate()
        {
            let divisor = if component == 0 { 1 } else { 2 };
            copy_rows(
                source,
                destination,
                width / divisor,
                lines.start / divisor,
                (self.y + lines.start) / divisor,
                lines.len() / divisor,
                columns.start * ctb / divisor..(columns.end * ctb).min(width) / divisor,
            )?;
        }
        copy_rows(
            &self.cells,
            &mut frame.cells,
            width / 4,
            lines.start / 4,
            (self.y + lines.start) / 4,
            lines.len() / 4,
            columns.start * ctb / 4..(columns.end * ctb).min(width) / 4,
        )?;
        let row = self.y / ctb;
        let ctus = width.div_ceil(ctb);
        frame
            .filter_ctus
            .get_mut(row * ctus + columns.start..row * ctus + columns.end)
            .ok_or(Error::InvalidData("wpp_row_storage"))?
            .copy_from_slice(
                self.ctus
                    .get(columns)
                    .ok_or(Error::InvalidData("wpp_row_storage"))?,
            );
        Ok(())
    }
}

impl Frame {
    fn storage_lengths(&self) -> [usize; 14] {
        let [y, cb, cr] = self.planes.each_ref().map(Vec::len);
        let [first, second] = self.scratch.predictors.each_ref().map(Vec::len);
        [
            y,
            cb,
            cr,
            self.cells.len(),
            self.filter_ctus.len(),
            self.scratch.coefficients.len(),
            self.scratch.residual.len(),
            self.scratch.prediction.len(),
            self.scratch.output.len(),
            self.scratch.scaling.len(),
            self.scratch.references.len(),
            first,
            second,
            self.scratch.inter_output.len(),
        ]
    }

    fn reset_private(&mut self, source: &Frame) {
        let Self {
            #[cfg(test)]
            test_hook,
            workers,
            deadline,
            index,
            info,
            depth,
            planes: _,
            cells: _,
            filter_ctus: _,
            filtered,
            next_ctu,
            segment_closed,
            contexts,
            wpp,
            qp,
            stats,
            inter_stats,
            reference,
            hash_seen,
            scratch,
        } = self;
        #[cfg(test)]
        {
            *test_hook = source.test_hook.clone();
        }
        *workers = 1;
        *deadline = source.deadline;
        *index = source.index;
        *info = source.info;
        *depth = source.depth;
        *filtered = source.filtered;
        *next_ctu = source.next_ctu;
        *segment_closed = source.segment_closed;
        *contexts = source.contexts;
        *wpp = source.wpp;
        *qp = source.qp;
        *stats = source.stats;
        *inter_stats = source.inter_stats;
        *reference = source.reference;
        *hash_seen = source.hash_seen;
        let super::Scratch {
            coefficients,
            residual,
            prediction,
            output,
            scaling,
            references,
            predictors,
            inter_output,
        } = scratch;
        coefficients.copy_from_slice(&source.scratch.coefficients);
        residual.copy_from_slice(&source.scratch.residual);
        prediction.copy_from_slice(&source.scratch.prediction);
        output.copy_from_slice(&source.scratch.output);
        scaling.copy_from_slice(&source.scratch.scaling);
        references.copy_from_slice(&source.scratch.references);
        for (target, source) in predictors.iter_mut().zip(&source.scratch.predictors) {
            target.copy_from_slice(source);
        }
        inter_output.copy_from_slice(&source.scratch.inter_output);
    }

    fn reset_row(&mut self, index: usize) -> Result<()> {
        let ctb = 1usize << self.info.ctb_log2;
        let width = self.info.width as usize;
        let height = self.info.height as usize;
        let start = index.saturating_sub(1) * ctb;
        let end = ((index + 2) * ctb).min(height);
        self.cells
            .get_mut(start / 4 * (width / 4)..end / 4 * (width / 4))
            .ok_or(Error::InvalidData("wpp_row_storage"))?
            .fill(Cell {
                mode: None,
                motion: None,
                skipped: false,
                luma_residual: false,
                depth: 0,
                reconstructed: 0,
                qp: 0,
                slice: 0,
                edges: 0,
                transform_edges: 0,
                protected: false,
            });
        let columns = width.div_ceil(ctb);
        self.filter_ctus
            .get_mut(
                index.saturating_sub(1) * columns..(index + 2).min(height.div_ceil(ctb)) * columns,
            )
            .ok_or(Error::InvalidData("wpp_row_storage"))?
            .fill(Ctu::default());
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "same slice and resource boundary as serial reconstruction"
    )]
    pub(super) fn decode_wpp(
        &mut self,
        input: &SliceInput<'_>,
        dpb: Option<&Dpb>,
        remaining: &mut u64,
        ranges: &[Range<usize>],
        fixed: usize,
        limit: usize,
        threads: usize,
        mut cache: Option<&mut Cache>,
    ) -> Result<Option<usize>> {
        if threads <= 1
            || cfg!(target_family = "wasm")
            || !input.pps.wpp
            || input.pps.tiles
            || !input.header.first
            || input.header.dependent
            || self.next_ctu != 0
            || ranges.len() != input.layout.height as usize
            || ranges.len() < 2
        {
            return Ok(None);
        }
        let reserved_bins = ranges.iter().try_fold(0u64, |total, range| {
            total.checked_add(Cabac::max_bins(range.len()))
        });
        if reserved_bins.is_none_or(|bins| bins > *remaining) {
            return Ok(None);
        }
        let frame_bytes = self.heap_bytes()?;
        let maximum = threads.clamp(1, 16).min(ranges.len());
        let count = (2..=maximum).rev().find(|count| {
            frame_bytes
                .checked_mul(*count)
                .and_then(|n| {
                    n.checked_add((*count - 1) * (STACK_BYTES + core::mem::size_of::<Frame>()))
                })
                .and_then(|n| n.checked_add(ranges.len() * core::mem::size_of::<Row>()))
                .and_then(|n| n.checked_add(fixed))
                .is_some_and(|n| n <= limit)
        });
        let Some(count) = count else { return Ok(None) };
        let mut reusable = match &mut cache {
            Some(cache) => cache.take(self, count)?,
            None => None,
        };
        let setup = (|| {
            let mut memory = Memory::new(fixed, limit)?;
            let warm = reusable.is_some();
            let mut workers = reusable.take().unwrap_or_default();
            if warm {
                memory.charge(super::allocation_bytes(&workers)?)?;
                for frame in &workers {
                    memory.charge(STACK_BYTES)?;
                    memory.charge(frame.heap_bytes()?)?;
                }
            }
            let mut rows = Vec::new();
            reserve(&mut rows, ranges.len(), &mut memory)?;
            for y in 0..ranges.len() {
                rows.push(Row {
                    band: Mutex::new(Band::new(
                        self,
                        y << input.sps.ctb_log2,
                        1 << input.sps.ctb_log2,
                        input.layout.width as usize,
                        &mut memory,
                    )?),
                    ready: Condvar::new(),
                });
            }
            if warm {
                let mut cold = Memory::new(memory.used, limit)?;
                cold.charge(input.scratch_bytes)?;
                for frame in &mut workers {
                    frame.reset_private(self);
                }
            } else {
                reserve(&mut workers, count - 1, &mut memory)?;
                for _ in 1..count {
                    memory.charge(STACK_BYTES)?;
                    let mut frame =
                        Self::allocate(input, self.reference, memory.used, limit, None)?;
                    frame.deadline = self.deadline;
                    #[cfg(test)]
                    {
                        frame.test_hook = self.test_hook.clone();
                    }
                    memory.charge(frame.heap_bytes()?)?;
                    workers.push(frame);
                }
            }
            Ok((rows, workers, memory.used))
        })();
        let (rows, mut workers, peak) = match setup {
            Ok(setup) => setup,
            Err(Error::Limit(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let next = AtomicUsize::new(0);
        self.workers = std::thread::scope(|scope| {
            let mut started = 1;
            for frame in &mut workers {
                let (rows, next) = (&rows, &next);
                if std::thread::Builder::new()
                    .stack_size(STACK_BYTES)
                    .spawn_scoped(scope, move || {
                        work(frame, input, dpb, ranges, rows, next);
                    })
                    .is_ok()
                {
                    started += 1;
                }
            }
            work(self, input, dpb, ranges, &rows, &next);
            started
        });
        let mut summary = DecodeSummary::default();
        for row in rows {
            let mut band = row
                .band
                .into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let outcome = band
                .outcome
                .take()
                .ok_or(Error::InvalidData("wpp_unfinished_row"))?;
            if outcome.consumed > *remaining {
                *remaining = 0;
                return Err(Error::Limit("cabac_bin_budget"));
            }
            *remaining -= outcome.consumed;
            let continuation = outcome.result?;
            band.import(self, 0..band.completed, 0..band.height)?;
            summary.add(continuation.stats, continuation.inter)?;
            self.next_ctu = continuation.next_ctu;
            self.segment_closed = continuation.closed;
            self.contexts = continuation.contexts;
            self.wpp = continuation.checkpoint;
            self.qp = continuation.qp;
        }
        self.stats = summary.reconstruction;
        self.inter_stats = summary.inter;
        if let Some(cache) = cache {
            cache.frames = workers;
        }
        Ok(Some(peak))
    }
}

fn work(
    frame: &mut Frame,
    input: &SliceInput<'_>,
    dpb: Option<&Dpb>,
    ranges: &[Range<usize>],
    rows: &[Row],
    next: &AtomicUsize,
) {
    loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(row) = rows.get(index) else { return };
        let allowance = ranges
            .get(index)
            .map_or(0, |range| Cabac::max_bins(range.len()));
        frame.next_ctu = index as u32 * input.layout.width;
        frame.segment_closed = false;
        frame.stats = IntraStats::default();
        frame.inter_stats = InterStats::default();
        frame.wpp = None;
        let mut budget = allowance;
        let result = caught(|| {
            frame.reset_row(index)?;
            decode_row(frame, input, dpb, ranges, rows, index, &mut budget)
        })
        .map(|()| Continuation {
            next_ctu: frame.next_ctu,
            closed: frame.segment_closed,
            contexts: frame.contexts,
            checkpoint: frame.wpp,
            qp: frame.qp,
            stats: frame.stats,
            inter: frame.inter_stats,
        });
        row.lock().outcome = Some(Outcome {
            result,
            consumed: allowance - budget,
        });
        row.ready.notify_all();
    }
}

fn decode_row(
    frame: &mut Frame,
    input: &SliceInput<'_>,
    dpb: Option<&Dpb>,
    ranges: &[Range<usize>],
    rows: &[Row],
    index: usize,
    budget: &mut u64,
) -> Result<()> {
    let row = rows
        .get(index)
        .ok_or(Error::InvalidData("wpp_row_storage"))?;
    let range = ranges
        .get(index)
        .ok_or(Error::InvalidData("intra_substream_range"))?;
    let bytes = input
        .rbsp
        .get(range.clone())
        .ok_or(Error::InvalidData("intra_substream_range"))?;
    let width = input.layout.width as usize;
    let slice = input
        .syntax
        .independent_address
        .checked_add(1)
        .ok_or(Error::InvalidData("intra_slice_address"))?;
    let mut cabac = Cabac::new(bytes, budget)?;
    for x in 0..width {
        if let Some(above) = index.checked_sub(1).and_then(|index| rows.get(index)) {
            #[cfg(test)]
            if let Some(hook) = &frame.test_hook {
                frame.deadline = hook.waiting(index, x).or(frame.deadline);
            }
            let mut band = above.lock();
            loop {
                frame.check_deadline()?;
                if let Some(outcome) = &band.outcome {
                    outcome.result?;
                }
                if band.completed >= (x + 2).min(width) {
                    break;
                }
                band = above
                    .ready
                    .wait_timeout(band, std::time::Duration::from_millis(10))
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
            let columns = x.saturating_sub(1)..(x + 2).min(width);
            band.import(
                frame,
                columns.clone(),
                band.height.saturating_sub(4)..band.height,
            )?;
            if x == 0 {
                band.import(frame, columns, 0..4.min(band.height))?;
                frame.wpp = band.checkpoint;
            }
        }
        frame.decode_ctu(input, dpb, slice, &mut cabac)?;
        #[cfg(test)]
        if let Some(hook) = &frame.test_hook {
            hook.publication(index, x);
        }
        {
            let mut band = row.lock();
            band.publish(frame, x..x + 1)?;
            band.completed = x + 1;
            band.checkpoint = frame.wpp;
        }
        row.ready.notify_all();
        if cabac.terminate()? {
            if index + 1 != ranges.len() {
                return Err(Error::InvalidData("intra_unconsumed_substreams"));
            }
            super::syntax::end_range(&mut cabac, bytes.len(), true)?;
            frame.segment_closed = true;
            return Ok(());
        }
        if frame.next_ctu == input.layout.width * input.layout.height {
            return Err(Error::InvalidData("intra_picture_termination"));
        }
        if x + 1 == width {
            if !cabac.terminate()? {
                return Err(Error::InvalidData("end_of_subset_one_bit"));
            }
            super::syntax::end_range(&mut cabac, bytes.len(), false)?;
        }
    }
    if index + 1 == ranges.len() {
        Err(Error::InvalidData("intra_missing_substream"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn reused_metadata_is_invalidated_only_in_the_assigned_row_and_neighbour_halos() -> Result<()> {
        let mut core = crate::parser::StreamCore::new(crate::Limits::default());
        for set in crate::tests::synthetic_parameter_nals_with_wpp(true) {
            core.read_nal(&set, |_| Ok(()))?;
        }
        let nal = crate::tests::intra_first_with_wpp(21, 1, Some(0));
        core.read_selected(
            &nal,
            |_| Ok(()),
            Some(0),
            usize::MAX,
            |input| {
                for height in [504, 512] {
                    let mut sps = *input.sps;
                    sps.height = height;
                    sps.visible_height = height;
                    sps.ctu_count = 8;
                    let input = SliceInput {
                        sps: &sps,
                        picture: crate::PictureInfo {
                            height,
                            visible_height: height,
                            ..input.picture
                        },
                        ..input
                    };
                    let source = Frame::new(&input, 0, usize::MAX)?;
                    let mut frame = Frame::new(&input, 0, usize::MAX)?;
                    for index in [0usize, 4, 7] {
                        for plane in &mut frame.planes {
                            plane.fill(255);
                        }
                        for cell in &mut frame.cells {
                            cell.slice = 1;
                            cell.reconstructed = 7;
                            cell.mode = Some(coreav_kernels_hevc::IntraMode::new(34)?);
                        }
                        for ctu in &mut frame.filter_ctus {
                            ctu.seen = true;
                        }
                        frame.filtered = true;
                        frame.hash_seen = true;
                        frame.next_ctu = 99;
                        frame.segment_closed = true;
                        frame.reset_private(&source);
                        assert!(!frame.filtered && !frame.hash_seen && !frame.segment_closed);
                        assert_eq!(frame.next_ctu, 0);
                        assert!(frame.cells.iter().all(|cell| cell.slice == 1));
                        assert!(frame.filter_ctus.iter().all(|ctu| ctu.seen));
                        frame.reset_row(index)?;
                        let start = index.saturating_sub(1);
                        let end = (index + 2).min(8);
                        for (at, cell) in frame.cells.iter().enumerate() {
                            let row = at / (64 * 64 / 16);
                            let reset = (start..end).contains(&row);
                            assert_eq!(cell.slice == 0, reset);
                            assert_eq!(cell.reconstructed == 0, reset);
                            assert_eq!(cell.mode.is_none(), reset);
                        }
                        for (row, ctu) in frame.filter_ctus.iter().enumerate() {
                            assert_eq!(!ctu.seen, (start..end).contains(&row));
                        }
                        assert!(frame.planes.iter().flatten().all(|sample| *sample == 255));
                    }
                }
                Ok(())
            },
        )?;
        Ok(())
    }

    #[test]
    fn empty_allocated_worker_slots_are_charged_and_revocable() -> Result<()> {
        let mut cache = Cache::default();
        cache
            .frames
            .try_reserve_exact(3)
            .map_err(|_| Error::Limit("picture_allocation"))?;
        let bytes = cache.frames.capacity() * core::mem::size_of::<Frame>();
        assert!(bytes > 0);
        assert_eq!(cache.retained_bytes(0, bytes)?, bytes);
        assert_eq!(cache.retained_bytes(0, bytes - 1)?, 0);
        assert_eq!(cache.frames.capacity(), 0);
        Ok(())
    }
}
