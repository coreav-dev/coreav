use super::*;
use crate::{
    cabac::Cabac,
    ctu::Layout,
    dpb::{Publication, ReferenceIdentity, ReferenceProvider},
    params::{Pps, Sps},
    parser::SliceSyntax,
    reconstruction::{
        RetainedPicture, RowPublisher, RowStore,
        raster::{Completion, Producer},
    },
};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

const STACK_BYTES: usize = 2 * 1024 * 1024;

#[cfg(feature = "native-hevc-rows-private")]
pub(super) fn job_record_bytes() -> usize {
    core::mem::size_of::<Job>()
}

#[cfg(feature = "native-hevc-rows-private")]
pub(super) fn qualify_capacities(
    input: &SliceInput<'_>,
    limits: DecodeLimits,
    sink: crate::native_rows::SinkCost,
    probe: &mut crate::native_rows::CapacityProbe,
) -> bool {
    let ctus = input.sps.ctu_count as usize;
    let nal = limits.headers.max_nal_bytes.min(sink.max_packet_bytes);
    let Some(&references) = input.sps.max_dpb.get(input.sps.sub_layers as usize) else {
        return false;
    };
    let references = references as usize;
    crate::reconstruction::qualify_capacities(input.picture, ctus, probe)
        && [
            input.original.len(),
            input.rbsp.len(),
            nal,
            nal.saturating_sub(2),
        ]
        .into_iter()
        .all(|count| probe.check::<u8>(count))
        && [
            input.entry_lengths.len(),
            limits.headers.max_ctus.saturating_sub(1) as usize,
        ]
        .into_iter()
        .all(|count| probe.check::<u64>(count))
        && probe.check::<core::ops::Range<usize>>(ctus)
        && probe.check::<OwnedSlice>(ctus.min(sink.max_nals))
        && (0..references).all(|count| probe.check::<(ReferenceIdentity, Arc<Publication>)>(count))
        && probe.check::<super::rows::Rows>(1)
}

#[cfg(feature = "native-hevc-rows-private")]
pub(super) fn continuation_job_bytes(
    info: PictureInfo,
    capacity: &crate::reconstruction::ContinuationCapacity,
    sink: crate::native_rows::SinkCost,
    references: usize,
    ctus: usize,
    nal: usize,
) -> Result<usize> {
    let overflow = || Error::Limit("reconstruction_memory");
    let operations = ctus.min(sink.max_nals);
    let sources = sink
        .max_packet_bytes
        .checked_mul(2)
        .and_then(|n| n.checked_add(operations.checked_mul(core::mem::size_of::<OwnedSlice>())?))
        .ok_or_else(overflow)?;
    let area = (info.width as usize)
        .checked_mul(info.height as usize)
        .ok_or_else(overflow)?;
    let fallback = area
        .checked_mul(4)
        .and_then(|n| n.checked_add(nal))
        .and_then(|n| n.checked_add(4096 + 32))
        .ok_or_else(overflow)?;
    let records = references
        .checked_mul(2)
        .and_then(|n| n.checked_mul(core::mem::size_of::<(ReferenceIdentity, Arc<Publication>)>()))
        .and_then(|n| n.checked_add(core::mem::size_of::<Job>()))
        .and_then(|n| {
            n.checked_add(
                4 * core::mem::size_of::<Work>() + core::mem::size_of::<ReadyReferences>(),
            )
        })
        .ok_or_else(overflow)?;
    [
        capacity.base,
        capacity.row_extra.max(fallback),
        sources,
        records,
        STACK_BYTES + 65536,
    ]
    .into_iter()
    .try_fold(0usize, |total, bytes| total.checked_add(bytes))
    .ok_or_else(overflow)
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct TestHook {
    entered: Mutex<u64>,
    ready: std::sync::Condvar,
    overlap: bool,
    completed: std::sync::atomic::AtomicU64,
    raster_conversions: std::sync::atomic::AtomicUsize,
    dependent_rows: bool,
    fault: Option<TestFault>,
}

#[cfg(test)]
enum TestFault {
    Panic(u64),
    ReverseErrors,
    Deadline,
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum AdmissionFault {
    Input,
    Snapshot,
    Record,
    Spawn,
    SpawnAfterOne,
}

#[cfg(test)]
impl TestHook {
    fn enter(&self, index: u64) -> Result<()> {
        let mut entered = self
            .entered
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *entered |= 1 << index;
        self.ready.notify_all();
        if self.overlap && matches!(index, 1 | 2) {
            let (entered, _) = self
                .ready
                .wait_timeout_while(entered, std::time::Duration::from_secs(3), |entered| {
                    *entered & 6 != 6
                })
                .unwrap_or_else(|error| error.into_inner());
            if *entered & 6 != 6 {
                return Err(Error::Internal("picture_jobs_did_not_overlap"));
            }
        } else {
            drop(entered);
        }
        if matches!(self.fault, Some(TestFault::ReverseErrors)) && index == 1 {
            let entered = self
                .entered
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (_entered, result) = self
                .ready
                .wait_timeout_while(entered, std::time::Duration::from_secs(3), |_| {
                    self.completed.load(Ordering::Acquire) & 4 == 0
                })
                .unwrap_or_else(|error| error.into_inner());
            if result.timed_out() {
                return Err(Error::Internal("picture_job_completion_order"));
            }
            return Err(Error::InvalidData("earlier_picture_job"));
        }
        match self.fault {
            Some(TestFault::Panic(fault)) if fault == index => panic!("picture job panic"),
            Some(TestFault::ReverseErrors) if index == 2 => {
                Err(Error::InvalidData("later_picture_job"))
            }
            Some(TestFault::Deadline) if matches!(index, 1 | 2) => {
                Err(Error::Limit("decode_deadline"))
            }
            _ => Ok(()),
        }
    }

    fn complete(&self, index: u64) {
        let _entered = self
            .entered
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.completed.fetch_or(1 << index, Ordering::Release);
        self.ready.notify_all();
    }
}

struct OwnedSlice {
    index: u64,
    hash: Option<SeiInfo>,
    header: crate::SliceHeader,
    picture: PictureInfo,
    original: Vec<u8>,
    rbsp: Vec<u8>,
    entries: Vec<u64>,
    syntax: SliceSyntax,
    sps: Sps,
    pps: Pps,
    layout: Layout,
}

impl OwnedSlice {
    fn new(input: SliceInput<'_>, memory: &mut Memory<'_>) -> Result<Self> {
        fn copy<T: Copy>(input: &[T], memory: &mut Memory<'_>) -> Result<Vec<T>> {
            memory.charge(
                input
                    .len()
                    .checked_mul(core::mem::size_of::<T>())
                    .ok_or(Error::Limit("reconstruction_memory"))?,
            )?;
            let mut output = Vec::new();
            output
                .try_reserve_exact(input.len())
                .map_err(|_| Error::Limit("picture_allocation"))?;
            memory.charge(
                (output.capacity() - input.len())
                    .checked_mul(core::mem::size_of::<T>())
                    .ok_or(Error::Limit("reconstruction_memory"))?,
            )?;
            output.extend_from_slice(input);
            Ok(output)
        }
        Ok(Self {
            index: input.index,
            hash: None,
            header: input.header,
            picture: input.picture,
            original: copy(input.original, memory)?,
            rbsp: copy(input.rbsp, memory)?,
            entries: copy(input.entry_lengths, memory)?,
            syntax: input.syntax,
            sps: *input.sps,
            pps: *input.pps,
            layout: *input.layout,
        })
    }

    fn bytes(&self) -> usize {
        self.original.capacity()
            + self.rbsp.capacity()
            + self.entries.capacity() * core::mem::size_of::<u64>()
    }

    fn input(&self) -> SliceInput<'_> {
        SliceInput {
            index: self.index,
            header: self.header,
            picture: self.picture,
            original: &self.original,
            rbsp: &self.rbsp,
            entry_lengths: &self.entries,
            syntax: self.syntax,
            sps: &self.sps,
            pps: &self.pps,
            layout: &self.layout,
            scratch_bytes: self.rbsp.capacity()
                + self.entries.capacity() * core::mem::size_of::<u64>(),
        }
    }
}

#[derive(Default)]
pub(super) struct ReadyReferences {
    entries: Vec<(ReferenceIdentity, Arc<Publication>)>,
    #[cfg(test)]
    hook: Option<(u64, Arc<TestHook>)>,
}

impl ReferenceProvider for ReadyReferences {
    fn reference(&self, identity: ReferenceIdentity) -> Result<&RetainedPicture> {
        self.publication(identity)?
            .picture()
            .ok_or(Error::InvalidData("unavailable reference samples"))
    }

    fn reference_info(&self, identity: ReferenceIdentity) -> Result<PictureInfo> {
        self.publication(identity)?.reference_info(identity)
    }

    fn read_reference(
        &self,
        identity: ReferenceIdentity,
        support: crate::reconstruction::ReadSupport,
    ) -> Result<crate::reconstruction::ReferenceRead<'_>> {
        self.publication(identity)?
            .read_reference(identity, support)
    }
}

impl ReadyReferences {
    fn publication(&self, identity: ReferenceIdentity) -> Result<&Publication> {
        #[cfg(test)]
        if let Some((index, hook)) = &self.hook {
            hook.enter(*index)?;
        }
        self.entries
            .iter()
            .find(|(entry, _)| entry.index == identity.index && entry.poc == identity.poc)
            .map(|(_, publication)| publication.as_ref())
            .ok_or(Error::InvalidData("unavailable reference samples"))
    }
}

struct RasterReferences<'a> {
    ready: &'a ReadyReferences,
    consumer: u64,
}

impl RasterReferences<'_> {
    fn publication(&self, identity: ReferenceIdentity) -> Result<&Publication> {
        let publication = self.ready.publication(identity)?;
        if publication.rows.is_some() && identity.index >= self.consumer {
            return Err(Error::Internal("reference_row_order"));
        }
        Ok(publication)
    }
}

impl ReferenceProvider for RasterReferences<'_> {
    fn reference(&self, identity: ReferenceIdentity) -> Result<&RetainedPicture> {
        self.publication(identity)?
            .picture()
            .ok_or(Error::InvalidData("unavailable reference samples"))
    }
    fn reference_info(&self, identity: ReferenceIdentity) -> Result<PictureInfo> {
        self.publication(identity)?.reference_info(identity)
    }
    fn read_reference(
        &self,
        identity: ReferenceIdentity,
        support: crate::reconstruction::ReadSupport,
    ) -> Result<crate::reconstruction::ReferenceRead<'_>> {
        self.publication(identity)?
            .read_reference(identity, support)
    }
}

#[derive(Default)]
struct Execution {
    active: [Option<Arc<AtomicBool>>; 16],
    maximum: usize,
}

#[expect(
    clippy::large_enum_variant,
    reason = "Keep the whole-picture fallback inline without another allocation"
)]
enum PictureWork {
    Whole(Frame),
    Raster(Producer, RowPublisher),
}

#[expect(
    clippy::large_enum_variant,
    reason = "Keep the whole-picture fallback inline without another allocation"
)]
enum PictureOutcome {
    Whole(Frame, Result<()>),
    Raster(Result<(Completion, RowPublisher)>),
}

struct Work {
    index: u64,
    picture: PictureWork,
    operations: Vec<OwnedSlice>,
    references: Arc<ReadyReferences>,
    budget: u64,
    reservation: usize,
    finalize: bool,
    execution: Arc<Mutex<Execution>>,
    overlap: Arc<AtomicBool>,
}

pub(super) struct Outcome {
    picture: PictureOutcome,
    pub(super) consumed: u64,
    overlap: bool,
}

impl Outcome {
    fn result(&self, stream_remaining: u64) -> Result<()> {
        let result = match &self.picture {
            PictureOutcome::Whole(_, result) => *result,
            PictureOutcome::Raster(result) => result.as_ref().map(|_| ()).map_err(|error| *error),
        };
        match result {
            Err(Error::Limit("cabac_bin_budget")) if stream_remaining == 0 => {
                Err(Error::Limit("stream_cabac_bin_budget"))
            }
            result => result,
        }
    }
}

impl Work {
    fn run(self) -> Outcome {
        {
            let mut execution = self
                .execution
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let count = execution.active.iter().flatten().count();
            if count != 0 {
                self.overlap.store(true, Ordering::Relaxed);
                for active in execution.active.iter().flatten() {
                    active.store(true, Ordering::Relaxed);
                }
            }
            let Some(slot) = execution.active.iter_mut().find(|slot| slot.is_none()) else {
                return Outcome {
                    picture: match self.picture {
                        PictureWork::Whole(frame) => PictureOutcome::Whole(
                            frame,
                            Err(Error::Internal("picture_worker_limit")),
                        ),
                        PictureWork::Raster(_, publisher) => {
                            publisher.fail();
                            PictureOutcome::Raster(Err(Error::Internal("picture_worker_limit")))
                        }
                    },
                    consumed: 0,
                    overlap: false,
                };
            };
            *slot = Some(Arc::clone(&self.overlap));
            execution.maximum = execution.maximum.max(count + 1);
        }
        let mut budget = self.budget;
        let fixed = self
            .operations
            .capacity()
            .checked_mul(core::mem::size_of::<OwnedSlice>())
            .and_then(|fixed| {
                self.operations
                    .iter()
                    .try_fold(fixed, |bytes, slice| bytes.checked_add(slice.bytes()))
            });
        let picture = match self.picture {
            PictureWork::Whole(mut frame) => {
                let result = crate::reconstruction::caught(|| {
                    let fixed = fixed.ok_or(Error::Limit("reconstruction_memory"))?;
                    for slice in &self.operations {
                        frame.decode_full(
                            slice.input(),
                            self.references.as_ref(),
                            &mut budget,
                            fixed,
                            self.reservation,
                            1,
                            None,
                        )?;
                        if let Some(hash) = slice.hash {
                            frame.hash_reclaiming(hash, fixed, self.reservation, None)?;
                        }
                    }
                    if self.finalize {
                        frame.finalize(fixed, self.reservation)?;
                    }
                    Ok(())
                });
                PictureOutcome::Whole(frame, result)
            }
            PictureWork::Raster(mut producer, mut publisher) => {
                let result = crate::reconstruction::caught(|| {
                    let references = RasterReferences {
                        ready: self.references.as_ref(),
                        consumer: self.index,
                    };
                    let fixed = fixed
                        .ok_or(Error::Limit("reconstruction_memory"))?
                        .checked_add(publisher.bytes()?)
                        .ok_or(Error::Limit("reconstruction_memory"))?;
                    #[cfg(test)]
                    if self
                        .references
                        .hook
                        .as_ref()
                        .is_some_and(|(index, hook)| *index == 0 && hook.dependent_rows)
                    {
                        publisher.wait_for_reader()?;
                    }
                    for slice in &self.operations {
                        producer.decode(
                            slice.input(),
                            &references,
                            &mut budget,
                            fixed,
                            self.reservation,
                            &mut publisher,
                        )?;
                        if let Some(hash) = slice.hash {
                            publisher.hash(&mut producer, hash)?;
                        }
                    }
                    Ok((producer.finish()?, publisher))
                });
                PictureOutcome::Raster(result)
            }
        };
        #[cfg(test)]
        if let Some((index, hook)) = &self.references.hook {
            hook.complete(*index);
        }
        {
            let mut execution = self
                .execution
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for active in &mut execution.active {
                if active
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, &self.overlap))
                {
                    *active = None;
                }
            }
        }
        Outcome {
            picture,
            consumed: self.budget - budget,
            overlap: self.overlap.load(Ordering::Relaxed),
        }
    }
}

struct Job {
    index: u64,
    #[cfg(feature = "native-hevc-rows-private")]
    info: PictureInfo,
    publication: Arc<Publication>,
    references: Arc<ReadyReferences>,
    handle: Option<std::thread::JoinHandle<Result<Outcome>>>,
    outcome: Option<Result<Outcome>>,
    reservation: usize,
    budget: u64,
    prior_bins: u64,
    timestamp: Option<i64>,
    origin: Option<u64>,
}

pub(super) struct JobIdentity {
    pub(super) prior_bins: u64,
    pub(super) timestamp: Option<i64>,
    pub(super) origin: Option<u64>,
}

#[derive(Default)]
pub(super) struct Scheduler {
    pub(super) enabled: bool,
    operations: Vec<OwnedSlice>,
    references: ReadyReferences,
    jobs: Vec<Job>,
    bound: u64,
    reserved_bins: u64,
    reservation: usize,
    scratch: usize,
    execution: Option<Arc<Mutex<Execution>>>,
    failure: Option<Error>,
    #[cfg(feature = "native-hevc-rows-private")]
    pub(super) rows: Option<Box<[super::rows::Rows; 1]>>,
    #[cfg(test)]
    pub(super) hook: Option<Arc<TestHook>>,
    #[cfg(test)]
    admission_fault: Option<AdmissionFault>,
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        for job in &self.jobs {
            if let Some(rows) = &job.publication.rows {
                rows.fail();
            }
        }
        for job in &mut self.jobs {
            if let Some(handle) = job.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Scheduler {
    #[cfg(feature = "native-hevc-rows-private")]
    pub(super) fn reserve_row_jobs(&mut self, ceiling: usize) -> Result<usize> {
        if !self.jobs.is_empty() {
            return Err(Error::Internal("row_job_storage"));
        }
        self.jobs = Vec::new();
        self.jobs
            .try_reserve_exact(ceiling + 1)
            .map_err(|_| Error::Limit("picture_allocation"))?;
        self.jobs
            .capacity()
            .checked_mul(core::mem::size_of::<Job>())
            .ok_or(Error::Limit("reconstruction_memory"))
    }

    #[cfg(feature = "native-hevc-rows-private")]
    pub(super) fn release_row_jobs(&mut self) {
        if self.jobs.is_empty() {
            self.jobs = Vec::new();
        }
    }
    #[cfg(feature = "native-hevc-rows-private")]
    pub(super) fn rows(&self) -> Option<&super::rows::Rows> {
        self.rows.as_deref().and_then(|rows| rows.first())
    }

    #[cfg(feature = "native-hevc-rows-private")]
    pub(super) fn rows_mut(&mut self) -> Option<&mut super::rows::Rows> {
        self.rows.as_deref_mut().and_then(|rows| rows.first_mut())
    }

    #[cfg(feature = "native-hevc-rows-private")]
    pub(super) fn fence(&self) -> Result<u64> {
        let issued = self
            .jobs
            .last()
            .map(|job| {
                job.index
                    .checked_add(1)
                    .ok_or(Error::Limit("picture_count"))
            })
            .transpose()?
            .unwrap_or(0);
        Ok(issued.max(self.rows().map_or(0, |rows| rows.reconciled)))
    }

    #[cfg(feature = "native-hevc-rows-private")]
    pub(super) fn confirm(&mut self, index: u64) -> Result<()> {
        if let Some(rows) = self.rows_mut() {
            rows.reconciled = rows
                .reconciled
                .max(index.checked_add(1).ok_or(Error::Limit("picture_count"))?);
        }
        Ok(())
    }

    #[cfg(feature = "native-hevc-rows-private")]
    pub(super) fn header(&self, index: u64, poc: i32) -> Result<crate::native_rows::OutputHeader> {
        let job = self
            .jobs
            .iter()
            .find(|job| job.index == index)
            .ok_or(Error::InvalidData("missing_output_samples"))?;
        Ok(crate::native_rows::OutputHeader {
            index,
            poc,
            timestamp: job.timestamp,
            origin: job.origin,
            info: job.info,
        })
    }
    pub(super) fn join_pending(&mut self) {
        for job in &mut self.jobs {
            if let Some(handle) = job.handle.take() {
                job.outcome = Some(
                    handle
                        .join()
                        .unwrap_or(Err(Error::Internal("reconstruction_panic"))),
                );
            }
        }
    }

    pub(super) fn has_input(&self) -> bool {
        !self.operations.is_empty()
    }

    #[cfg(hevc_thread_diagnostics)]
    pub(super) fn has_work(&self) -> bool {
        self.has_input() || !self.jobs.is_empty()
    }

    pub(super) fn queue_hash(&mut self, info: SeiInfo) -> bool {
        let Some(slice) = self.operations.last_mut() else {
            return false;
        };
        if slice.hash.is_some() {
            return false;
        }
        slice.hash = Some(info);
        true
    }

    pub(super) fn full(&self, threads: usize) -> bool {
        #[cfg(feature = "native-hevc-rows-private")]
        let threads = self.rows().map_or(threads, |rows| rows.plan.workers);
        self.jobs.len() >= threads
    }

    pub(super) fn current_reservation(&self) -> usize {
        self.reservation
    }

    pub(super) fn required_references(
        &mut self,
        dpb: &Dpb,
        summary: &mut DecodeSummary,
        remaining: &mut u64,
    ) -> Result<()> {
        for identity in dpb.current_references().entries.iter().flatten() {
            if dpb.reference(*identity).is_err() {
                crate::thread_diagnostic!(barrier ReferenceUnavailable);
                self.drain(summary, remaining, Some(identity.index))?;
            }
            dpb.reference(*identity)?;
        }
        Ok(())
    }

    pub(super) fn bytes(&self, dpb: Option<&Dpb>) -> Result<usize> {
        let mut bytes = self.jobs.capacity() * core::mem::size_of::<Job>();
        bytes = bytes
            .checked_add(self.reservation)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        for job in &self.jobs {
            bytes = bytes
                .checked_add(job.reservation)
                .ok_or(Error::Limit("reconstruction_memory"))?;
        }
        #[cfg(feature = "native-hevc-rows-private")]
        if let Some(rows) = self.rows() {
            bytes = bytes
                .checked_add(rows.heap_bytes()?)
                .ok_or(Error::Limit("reconstruction_memory"))?;
        }
        #[cfg(feature = "native-hevc-rows-private")]
        let pending = self.rows().into_iter().flat_map(|rows| {
            rows.pending
                .iter()
                .map(|pending| (pending.header.index, &pending.publication))
        });
        #[cfg(not(feature = "native-hevc-rows-private"))]
        let pending = core::iter::empty();
        let mut seen = [None; 304];
        let mut count = 0;
        for (index, publication) in self
            .references
            .entries
            .iter()
            .chain(
                self.jobs
                    .iter()
                    .flat_map(|job| job.references.entries.iter()),
            )
            .map(|(identity, publication)| (identity.index, publication))
            .chain(self.jobs.iter().map(|job| (job.index, &job.publication)))
            .chain(pending)
        {
            if dpb
                .and_then(|dpb| dpb.publication(index))
                .is_some_and(|owner| Arc::ptr_eq(owner, publication))
            {
                continue;
            }
            let pointer = Arc::as_ptr(publication);
            if seen.iter().take(count).any(|entry| *entry == Some(pointer)) {
                continue;
            }
            if count == seen.len() {
                return Err(Error::Internal("reference_owners"));
            }
            *seen
                .get_mut(count)
                .ok_or(Error::Internal("reference_owners"))? = Some(pointer);
            count += 1;
            bytes = bytes
                .checked_add(publication.bytes()?)
                .ok_or(Error::Limit("reconstruction_memory"))?;
        }
        Ok(bytes)
    }

    pub(super) fn drain(
        &mut self,
        summary: &mut DecodeSummary,
        remaining: &mut u64,
        through: Option<u64>,
    ) -> Result<()> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = self.drain_inner(summary, remaining, through);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    fn drain_inner(
        &mut self,
        summary: &mut DecodeSummary,
        remaining: &mut u64,
        through: Option<u64>,
    ) -> Result<()> {
        while !self.jobs.is_empty() {
            if through.is_some_and(|index| self.jobs.first().is_some_and(|job| job.index > index)) {
                break;
            }
            let mut job = self.jobs.remove(0);
            let outcome = match job.outcome.take() {
                Some(outcome) => outcome?,
                None => job
                    .handle
                    .take()
                    .ok_or(Error::Internal("picture_job_handle"))?
                    .join()
                    .map_err(|_| Error::Internal("reconstruction_panic"))??,
            };
            self.reserved_bins -= job.budget;
            *remaining = remaining
                .checked_sub(outcome.consumed)
                .ok_or(Error::Internal("picture_job_budget"))?;
            outcome.result(*remaining)?;
            summary.frame_parallel_pictures += u64::from(outcome.overlap);
            summary.parallel_pictures += u64::from(outcome.overlap);
            if let Some(execution) = &self.execution {
                summary.max_workers = summary.max_workers.max(
                    execution
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .maximum,
                );
            }
            match outcome.picture {
                PictureOutcome::Whole(mut frame, _) => {
                    frame.stats.bins = job.prior_bins + outcome.consumed;
                    frame.stats.memory_bytes = summary.memory_bytes;
                    summary.add(frame.stats, frame.inter_stats())?;
                    job.publication
                        .samples
                        .set(frame.retain(job.timestamp, job.origin)?)
                        .map_err(|_| Error::Internal("picture_job_publication"))?;
                }
                PictureOutcome::Raster(result) => {
                    let (mut completion, publisher) = result?;
                    completion.stats.bins = job.prior_bins + outcome.consumed;
                    completion.stats.memory_bytes = summary.memory_bytes;
                    summary.add(completion.stats, completion.inter_stats)?;
                    publisher.complete(completion, job.timestamp, job.origin)?;
                }
            }
            #[cfg(feature = "native-hevc-rows-private")]
            self.confirm(job.index)?;
        }
        if self.jobs.is_empty() {
            #[cfg(feature = "native-hevc-rows-private")]
            let retain = self.rows().is_some();
            #[cfg(not(feature = "native-hevc-rows-private"))]
            let retain = false;
            if !retain {
                self.jobs = Vec::new();
            }
            self.execution = None;
        }
        Ok(())
    }

    pub(super) fn references(
        &mut self,
        dpb: &mut Dpb,
        summary: &mut DecodeSummary,
        remaining: &mut u64,
    ) -> Result<bool> {
        if !self.references.entries.is_empty() {
            return Ok(true);
        }
        let references = dpb.current_references();
        for identity in references.entries.iter().flatten() {
            #[cfg(feature = "native-hevc-rows-private")]
            if self.rows().is_some_and(|rows| rows.plan.lag != 0)
                && dpb.publication(identity.index).is_some_and(|publication| {
                    publication.rows.is_some() && publication.reference_info(*identity).is_ok()
                })
            {
                continue;
            }
            if dpb.reference(*identity).is_err() {
                crate::thread_diagnostic!(barrier ReferenceUnavailable);
                self.drain(summary, remaining, Some(identity.index))?;
            }
        }
        let mut entries = Vec::new();
        #[cfg(test)]
        if matches!(self.admission_fault, Some(AdmissionFault::Snapshot)) {
            return Ok(false);
        }
        if entries
            .try_reserve_exact(references.entries.iter().flatten().count())
            .is_err()
        {
            crate::thread_diagnostic!(barrier SnapshotAllocation);
            return Ok(false);
        }
        for identity in references.entries.into_iter().flatten() {
            #[cfg(feature = "native-hevc-rows-private")]
            if self.rows().is_some_and(|rows| rows.plan.lag != 0) {
                let consumer = self
                    .operations
                    .first()
                    .ok_or(Error::Internal("picture_job_input"))?
                    .index;
                entries.push((identity, dpb.share_rows(identity, consumer)?));
                continue;
            }
            entries.push((identity, dpb.share(identity)?));
        }
        self.references = ReadyReferences {
            entries,
            #[cfg(test)]
            hook: None,
        };
        Ok(true)
    }

    pub(super) fn discard_current(&mut self) {
        self.operations = Vec::new();
        self.references = ReadyReferences::default();
        self.reservation = 0;
        self.scratch = 0;
        self.bound = 0;
    }

    pub(super) fn queue(
        &mut self,
        input: SliceInput<'_>,
        frame: &Frame,
        fixed: usize,
        limit: usize,
        remaining: u64,
        picture_remaining: u64,
    ) -> Result<bool> {
        #[cfg(feature = "native-hevc-rows-private")]
        if self
            .rows()
            .is_some_and(|rows| self.operations.len() >= rows.operation_limit)
        {
            return Ok(false);
        }
        let bound = Cabac::max_bins(input.rbsp.len())
            .saturating_add((input.entry_lengths.len() as u64).saturating_mul(128))
            .min(picture_remaining.saturating_sub(self.bound));
        if bound
            > remaining
                .saturating_sub(self.reserved_bins)
                .saturating_sub(self.bound)
        {
            crate::thread_diagnostic!(barrier QueueBudget);
            return Ok(false);
        }
        let extra = input
            .original
            .len()
            .checked_add(input.rbsp.len())
            .and_then(|n| n.checked_add(input.entry_lengths.len() * 8))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let scratch = (input.picture.width as usize)
            .checked_mul(input.picture.height as usize)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_add(input.scratch_bytes))
            .and_then(|n| n.checked_add((input.entry_lengths.len() + 1) * 32))
            .and_then(|n| n.checked_add(4096))
            .ok_or(Error::Limit("reconstruction_memory"))?
            .max(self.scratch);
        let requested = self
            .reservation
            .checked_add(extra)
            .and_then(|n| n.checked_add(2 * core::mem::size_of::<OwnedSlice>()))
            .and_then(|n| n.checked_add(core::mem::size_of::<Job>()))
            .and_then(|n| n.checked_add(scratch))
            .and_then(|n| n.checked_add(STACK_BYTES + 65536 + 4 * core::mem::size_of::<Work>()))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let Some(available) = limit
            .checked_sub(fixed)
            .and_then(|n| n.checked_sub(frame.heap_bytes().ok()?))
        else {
            crate::thread_diagnostic!(barrier QueueMemory);
            return Ok(false);
        };
        if requested > available {
            crate::thread_diagnostic!(barrier QueueMemory);
            return Ok(false);
        }
        #[cfg(test)]
        if matches!(self.admission_fault, Some(AdmissionFault::Input)) {
            return Ok(false);
        }
        let mut memory = Memory::new(0, available)?;
        memory.charge(requested - extra)?;
        let capacity = self.jobs.capacity();
        #[cfg(test)]
        let refuse = matches!(self.admission_fault, Some(AdmissionFault::Record));
        #[cfg(not(test))]
        let refuse = false;
        #[cfg(feature = "native-hevc-rows-private")]
        let bounded = self.rows().is_some();
        #[cfg(not(feature = "native-hevc-rows-private"))]
        let bounded = false;
        if bounded && self.jobs.len() == self.jobs.capacity() {
            return Err(Error::Internal("row_job_storage"));
        }
        if refuse || (!bounded && self.jobs.try_reserve_exact(1).is_err()) {
            crate::thread_diagnostic!(barrier QueueMemory);
            return Ok(false);
        }
        let records = (self.jobs.capacity() - capacity) * core::mem::size_of::<Job>();
        if memory
            .charge(records.saturating_sub(core::mem::size_of::<Job>()))
            .is_err()
        {
            crate::thread_diagnostic!(barrier QueueMemory);
            return Ok(false);
        }
        let slice = match OwnedSlice::new(input, &mut memory) {
            Ok(slice) => slice,
            Err(Error::Limit(_)) => {
                crate::thread_diagnostic!(barrier QueueMemory);
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let prior_capacity = self.operations.capacity();
        #[cfg(feature = "native-hevc-rows-private")]
        let operations = self
            .rows()
            .filter(|_| self.operations.capacity() == 0)
            .map_or(1, |rows| rows.operation_limit);
        #[cfg(not(feature = "native-hevc-rows-private"))]
        let operations = 1;
        if (!bounded || self.operations.capacity() == 0)
            && self.operations.try_reserve_exact(operations).is_err()
        {
            crate::thread_diagnostic!(barrier QueueMemory);
            return Ok(false);
        }
        let actual = self.operations.capacity() * core::mem::size_of::<OwnedSlice>()
            + self.operations.iter().map(OwnedSlice::bytes).sum::<usize>()
            + slice.bytes();
        let reservation = actual
            .checked_add(scratch)
            .and_then(|n| n.checked_add(STACK_BYTES + 65536 + 4 * core::mem::size_of::<Work>()))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        if Memory::new(reservation + records, available).is_err() {
            crate::thread_diagnostic!(barrier QueueMemory);
            if self.operations.is_empty() {
                self.operations = Vec::new();
            } else {
                self.reservation += (self.operations.capacity() - prior_capacity)
                    * core::mem::size_of::<OwnedSlice>();
            }
            return Ok(false);
        }
        self.operations.push(slice);
        crate::thread_diagnostic!(event QueueAdmitted);
        self.reservation = reservation;
        self.scratch = scratch;
        self.bound += bound;
        Ok(true)
    }

    pub(super) fn run_current(&mut self, frame: Frame, finalize: bool, budget: u64) -> Outcome {
        crate::thread_diagnostic!(event CallerJob);
        let execution = self
            .execution
            .get_or_insert_with(|| Arc::new(Mutex::new(Execution::default())));
        Work {
            index: frame.index(),
            reservation: self.reservation + frame.heap_bytes().unwrap_or(0),
            picture: PictureWork::Whole(frame),
            operations: core::mem::take(&mut self.operations),
            references: Arc::new(core::mem::take(&mut self.references)),
            budget,
            finalize,
            execution: Arc::clone(execution),
            overlap: Arc::new(AtomicBool::new(false)),
        }
        .run()
    }

    pub(super) fn flush(
        &mut self,
        active: &mut Vec<Frame>,
        remaining: &mut u64,
        picture_remaining: &mut u64,
    ) -> Result<()> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if !self.has_input() {
            return Ok(());
        }
        crate::thread_diagnostic!(event ForcedFlush);
        let frame = active.pop().ok_or(Error::Internal("picture_job_frame"))?;
        let outcome = self.run_current(frame, false, (*remaining).min(*picture_remaining));
        self.bound = 0;
        self.reservation = 0;
        self.scratch = 0;
        if self.jobs.is_empty() {
            self.execution = None;
            #[cfg(feature = "native-hevc-rows-private")]
            let retain = self.rows().is_some();
            #[cfg(not(feature = "native-hevc-rows-private"))]
            let retain = false;
            if !retain {
                self.jobs = Vec::new();
            }
        }
        *remaining -= outcome.consumed;
        *picture_remaining -= outcome.consumed;
        let result = outcome.result(*remaining);
        let PictureOutcome::Whole(frame, _) = outcome.picture else {
            return Err(Error::Internal("picture_partial_owner"));
        };
        active.push(frame);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    pub(super) fn seal(
        &mut self,
        frame: Frame,
        dpb: &mut Dpb,
        limits: DecodeLimits,
        summary: &mut DecodeSummary,
        remaining: &mut u64,
        identity: JobIdentity,
    ) -> Result<()> {
        if self.full(limits.threads) {
            crate::thread_diagnostic!(barrier WindowFull);
            self.drain(summary, remaining, None)?;
        }
        #[cfg(feature = "native-hevc-rows-private")]
        let bounded = self.rows().is_some();
        #[cfg(not(feature = "native-hevc-rows-private"))]
        let bounded = false;
        if bounded && self.jobs.len() == self.jobs.capacity() {
            return Err(Error::Internal("row_job_storage"));
        }
        if !bounded && self.jobs.try_reserve_exact(1).is_err() {
            self.drain(summary, remaining, None)?;
            let outcome = self.run_current(frame, true, self.bound);
            self.discard_current();
            *remaining -= outcome.consumed;
            outcome.result(*remaining)?;
            let PictureOutcome::Whole(mut frame, _) = outcome.picture else {
                return Err(Error::Internal("picture_partial_owner"));
            };
            frame.stats.bins = identity.prior_bins + outcome.consumed;
            frame.stats.memory_bytes = summary.memory_bytes;
            summary.max_workers = summary.max_workers.max(1);
            summary.add(frame.stats, frame.inter_stats())?;
            self.execution = None;
            #[cfg(feature = "native-hevc-rows-private")]
            let index = frame.index();
            dpb.attach(frame.retain(identity.timestamp, identity.origin)?)?;
            #[cfg(feature = "native-hevc-rows-private")]
            self.confirm(index)?;
            return Ok(());
        }
        let index = frame.index();
        let info = frame.info();
        let reservation = self
            .reservation
            .checked_add(frame.heap_bytes()?)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let sources = self
            .operations
            .capacity()
            .checked_mul(core::mem::size_of::<OwnedSlice>())
            .and_then(|fixed| {
                self.operations
                    .iter()
                    .try_fold(fixed, |bytes, slice| bytes.checked_add(slice.bytes()))
            })
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let allowance = (info.width as usize)
            .checked_mul(info.height as usize)
            .and_then(|samples| samples.checked_mul(core::mem::size_of::<u16>()))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let row_limit = sources
            .checked_add(frame.heap_bytes()?)
            .and_then(|bytes| bytes.checked_add(allowance))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let fixed = sources
            .checked_add(RowStore::record_bytes())
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let deadline = frame.deadline;
        let (picture, rows) = match Producer::try_from_full(frame, fixed, row_limit) {
            Ok((producer, backing)) => {
                let backing_bytes = backing.heap_bytes()?;
                let fixed = sources
                    .checked_add(producer.heap_bytes()?)
                    .and_then(|bytes| bytes.checked_add(backing_bytes))
                    .ok_or(Error::Limit("reconstruction_memory"))?;
                let mut memory = Memory::new(fixed, row_limit)?;
                let (rows, publisher) = RowStore::new(
                    dpb.current_identity(index)?,
                    index,
                    backing,
                    deadline,
                    &mut memory,
                )?;
                (PictureWork::Raster(producer, publisher), Some(rows))
            }
            Err((frame, _)) => (PictureWork::Whole(frame), None),
        };
        #[cfg(test)]
        if rows.is_some() {
            if let Some(hook) = &self.hook {
                hook.raster_conversions.fetch_add(1, Ordering::Relaxed);
            }
        }
        let retained_reservation = match &rows {
            Some(rows) => reservation
                .checked_sub(rows.bytes()?)
                .ok_or(Error::Internal("picture_job_reservation"))?,
            None => reservation,
        };
        let execution = self
            .execution
            .get_or_insert_with(|| Arc::new(Mutex::new(Execution::default())));
        let publication = Arc::new(Publication {
            samples: OnceLock::new(),
            rows,
        });
        dpb.seal(index, info, Arc::clone(&publication))?;
        #[cfg(test)]
        {
            self.references.hook = self.hook.as_ref().map(|hook| (index, Arc::clone(hook)));
        }
        let references = Arc::new(core::mem::take(&mut self.references));
        let work = Work {
            index,
            picture,
            operations: core::mem::take(&mut self.operations),
            references: Arc::clone(&references),
            budget: self.bound,
            reservation,
            finalize: true,
            execution: Arc::clone(execution),
            overlap: Arc::new(AtomicBool::new(false)),
        };
        let budget = self.bound;
        self.reserved_bins += budget;
        self.bound = 0;
        self.reservation = 0;
        self.scratch = 0;
        let background = self.jobs.iter().filter(|job| job.handle.is_some()).count();
        let mut job = Job {
            index,
            #[cfg(feature = "native-hevc-rows-private")]
            info,
            publication,
            references,
            handle: None,
            outcome: None,
            reservation: retained_reservation,
            budget,
            prior_bins: identity.prior_bins,
            timestamp: identity.timestamp,
            origin: identity.origin,
        };
        if background < limits.threads.saturating_sub(1) {
            let owner = Arc::new(Mutex::new(Some(work)));
            let worker = Arc::clone(&owner);
            let spawn = || {
                std::thread::Builder::new()
                    .stack_size(STACK_BYTES)
                    .spawn(move || {
                        let work = worker
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .take()
                            .ok_or(Error::Internal("picture_job_owner"))?;
                        Ok(work.run())
                    })
            };
            #[cfg(test)]
            let started = if matches!(self.admission_fault, Some(AdmissionFault::Spawn))
                || matches!(self.admission_fault, Some(AdmissionFault::SpawnAfterOne))
                    && self.jobs.iter().any(|job| job.handle.is_some())
            {
                Err(std::io::Error::other("picture_job_start"))
            } else {
                spawn()
            };
            #[cfg(not(test))]
            let started = spawn();
            match started {
                Ok(handle) => {
                    crate::thread_diagnostic!(event SpawnedJob);
                    job.handle = Some(handle);
                }
                Err(_) => {
                    crate::thread_diagnostic!(barrier WorkerSpawn);
                    crate::thread_diagnostic!(event CallerJob);
                    let work = owner
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .take()
                        .ok_or(Error::Internal("picture_job_owner"))?;
                    job.outcome = Some(Ok(work.run()));
                }
            }
        } else {
            crate::thread_diagnostic!(event CallerJob);
            job.outcome = Some(Ok(work.run()));
        }
        self.jobs.push(job);
        Ok(())
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
