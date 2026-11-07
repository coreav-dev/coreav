use super::{
    Cell, Frame, InterStats, Memory, caught,
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
        let setup_started = std::time::Instant::now();
        let setup = (|| {
            let mut memory = Memory::new(fixed, limit)?;
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
            let mut workers = Vec::new();
            reserve(&mut workers, count - 1, &mut memory)?;
            for _ in 1..count {
                memory.charge(STACK_BYTES)?;
                let mut frame = Self::allocate(input, self.reference, memory.used, limit)?;
                frame.deadline = self.deadline;
                #[cfg(test)]
                {
                    frame.test_hook = self.test_hook.clone();
                }
                memory.charge(frame.heap_bytes()?)?;
                workers.push(frame);
            }
            Ok((rows, workers, memory.used))
        })();
        let (rows, workers, peak) = match setup {
            Ok(setup) => setup,
            Err(Error::Limit(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        eprintln!(
            "wpp-profile-setup {} {} {}",
            input.index,
            count,
            setup_started.elapsed().as_nanos()
        );
        let reconstruction_started = std::time::Instant::now();
        let next = AtomicUsize::new(0);
        self.workers = std::thread::scope(|scope| {
            let mut started = 1;
            for mut frame in workers {
                let (rows, next) = (&rows, &next);
                if std::thread::Builder::new()
                    .stack_size(STACK_BYTES)
                    .spawn_scoped(scope, move || {
                        work(&mut frame, input, dpb, ranges, rows, next);
                    })
                    .is_ok()
                {
                    started += 1;
                }
            }
            work(self, input, dpb, ranges, &rows, &next);
            started
        });
        eprintln!(
            "wpp-profile-reconstruct {} {}",
            input.index,
            reconstruction_started.elapsed().as_nanos()
        );
        let merge_started = std::time::Instant::now();
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
        eprintln!(
            "wpp-profile-merge {} {}",
            input.index,
            merge_started.elapsed().as_nanos()
        );
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
        let mut phases = [std::time::Duration::ZERO; 3];
        let result = caught(|| {
            decode_row(
                frame,
                input,
                dpb,
                ranges,
                rows,
                index,
                &mut budget,
                &mut phases,
            )
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
        eprintln!(
            "wpp-profile-row {} {} {} {} {}",
            input.index,
            index,
            phases[0].as_nanos(),
            phases[1].as_nanos(),
            phases[2].as_nanos()
        );
        row.lock().outcome = Some(Outcome {
            result,
            consumed: allowance - budget,
        });
        row.ready.notify_all();
    }
}

#[expect(clippy::too_many_arguments, reason = "temporary phase profile")]
fn decode_row(
    frame: &mut Frame,
    input: &SliceInput<'_>,
    dpb: Option<&Dpb>,
    ranges: &[Range<usize>],
    rows: &[Row],
    index: usize,
    budget: &mut u64,
    phases: &mut [std::time::Duration; 3],
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
        let started = std::time::Instant::now();
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
        phases[0] += started.elapsed();
        let started = std::time::Instant::now();
        frame.decode_ctu(input, dpb, slice, &mut cabac)?;
        phases[1] += started.elapsed();
        let started = std::time::Instant::now();
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
        phases[2] += started.elapsed();
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
