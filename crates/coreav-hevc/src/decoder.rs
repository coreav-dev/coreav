use crate::{
    Error, InterStats, IntraPlane, IntraStats, Limits, PictureInfo, Result, SeiInfo, Summary,
    dpb::{Dpb, Emission},
    parser::{PictureObserver, SliceInput, StreamCore},
    reconstruction::{Cache, Frame, Memory, ReclaimMemory},
};

#[cfg(feature = "native-hevc-rows-private")]
mod rows;
mod scheduler;

trait Delivery {
    fn output(&mut self, picture: DecodedPicture<'_>, memory: &mut OutputMemory<'_>) -> Result<()>;
    #[cfg(feature = "native-hevc-rows-private")]
    fn row_sink(&mut self) -> Option<&mut dyn crate::native_rows::RowSink> {
        None
    }
}

impl<F: FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>> Delivery for F {
    fn output(&mut self, picture: DecodedPicture<'_>, memory: &mut OutputMemory<'_>) -> Result<()> {
        self(picture, memory)
    }
}

#[cfg(feature = "native-hevc-rows-private")]
struct RowsOutput<'a>(&'a mut dyn crate::native_rows::RowSink);

#[cfg(feature = "native-hevc-rows-private")]
impl Delivery for RowsOutput<'_> {
    fn row_sink(&mut self) -> Option<&mut dyn crate::native_rows::RowSink> {
        Some(self.0)
    }
    fn output(&mut self, picture: DecodedPicture<'_>, memory: &mut OutputMemory<'_>) -> Result<()> {
        let offset = self.0.cost(0, 0)?.queued_frames;
        if self.0.prepare(
            crate::native_rows::OutputHeader {
                index: picture.index,
                poc: picture.poc,
                timestamp: picture.timestamp,
                origin: picture.origin,
                info: picture.info,
            },
            memory,
        )? {
            let result = self.0.fill(offset, picture);
            if result.is_err() {
                self.0.truncate(offset);
            }
            result?;
        }
        Ok(())
    }
}

/// Independent admission, live-memory and entropy-work limits for a complete stream.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    /// Requested reconstruction workers, bounded to one through sixteen and available CPUs.
    pub threads: usize,
    /// Header admission, including the stream-wide coded-picture limit.
    pub headers: Limits,
    /// Maximum simultaneous codec state, pictures and transient scratch bytes.
    pub max_memory_bytes: usize,
    /// Maximum regular, bypass and termination bins over the complete stream.
    pub max_stream_bins: u64,
    /// Maximum bins in any one reconstructed picture.
    pub max_picture_bins: u64,
    /// Maximum bytes of a packed visible output picture, checked before sample allocation.
    pub max_frame_bytes: u64,
    /// Optional absolute decoder-session deadline, including filtering and output work.
    pub deadline: Option<std::time::Instant>,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            threads: 1,
            headers: Limits::default(),
            max_memory_bytes: 512 * 1024 * 1024,
            max_stream_bins: 1 << 40,
            max_picture_bins: 268_435_456,
            max_frame_bytes: u64::MAX,
            deadline: None,
        }
    }
}

/// One complete filtered picture, borrowed in presentation order.
#[derive(Clone, Copy, Debug)]
pub struct DecodedPicture<'a> {
    /// Original zero-based coded-picture index, including non-output pictures.
    pub index: u64,
    /// Full signed picture order count from the shared DPB.
    pub poc: i32,
    /// Caller timestamp supplied with the first slice, in the caller's own clock.
    pub timestamp: Option<i64>,
    /// Opaque caller identity supplied with the picture's first slice.
    pub origin: Option<u64>,
    /// Activated geometry, crop and original display declarations.
    pub info: PictureInfo,
    /// Complete coded Y, Cb and Cr planes; cropping is not applied to these views.
    pub planes: [IntraPlane<'a>; 3],
    /// Reconstruction and serial-tool work through the picture's finalization.
    pub stats: IntraStats,
    /// Effective inter prediction and boundary-strength work.
    pub inter: InterStats,
}

/// A callback-scoped reservation in the decoder's combined live-memory account.
pub struct OutputMemory<'a> {
    codec_bytes: usize,
    reserved: &'a mut usize,
    limit: usize,
    reclaim: Option<&'a mut dyn ReclaimMemory>,
}

impl OutputMemory<'_> {
    /// Charges external capacity before allocating output or queue storage.
    pub fn reserve(&mut self, bytes: usize) -> Result<()> {
        let reserved = self
            .reserved
            .checked_add(bytes)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let mut memory = Memory::with_reclaim(
            self.codec_bytes,
            self.limit,
            match &mut self.reclaim {
                Some(reclaim) => Some(&mut **reclaim),
                None => None,
            },
        )?;
        memory.charge(reserved)?;
        *self.reserved = reserved;
        Ok(())
    }

    /// Releases external capacity after it is freed or transferred to the caller.
    pub fn release(&mut self, bytes: usize) -> Result<()> {
        *self.reserved = self
            .reserved
            .checked_sub(bytes)
            .ok_or(Error::InvalidData("output_reservation_underflow"))?;
        Ok(())
    }
}

/// Successful complete-stream accounting, including pictures not selected for output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeSummary {
    /// Pictures whose reconstruction overlapped another whole-picture job.
    pub frame_parallel_pictures: u64,
    /// Pictures reconstructed with whole-picture overlap or multiple WPP workers.
    pub parallel_pictures: u64,
    /// Greatest executing picture-job count or WPP worker count, including the caller.
    pub max_workers: usize,
    /// Header syntax and presentation-order output counts.
    pub headers: Summary,
    /// Pictures reconstructed and finalized, including non-output reference pictures.
    pub decoded_pictures: u64,
    /// Temporally extracted or suppressed RASL pictures with no reconstructed samples.
    pub skipped_pictures: u64,
    /// Defined hash messages attached to pictures outside the target decoding set.
    /// These messages have no reconstructed samples and receive no checked-hash credit.
    pub skipped_picture_hashes: u64,
    /// Reserved hash messages attached to pictures outside the target decoding set.
    pub skipped_reserved_hashes: u64,
    /// Work and checked hashes over all reconstructed pictures, not only outputs.
    pub reconstruction: IntraStats,
    /// Effective inter work over all reconstructed pictures.
    pub inter: InterStats,
    /// Peak accounted simultaneous codec bytes.
    pub memory_bytes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Finished,
    Failed,
}

#[derive(Default)]
struct Slots {
    active: Vec<Frame>,
    cache: Cache,
    scheduler: scheduler::Scheduler,
}

impl Slots {
    fn as_ref(&self) -> Option<&Frame> {
        self.active.first()
    }
    fn is_none(&self) -> bool {
        self.active.is_empty()
    }
    fn take(&mut self) -> Option<Frame> {
        self.active.pop()
    }

    fn prepare(&mut self) -> Result<()> {
        if self.active.capacity() == 0 {
            self.active
                .try_reserve_exact(1)
                .map_err(|_| Error::Limit("picture_allocation"))?;
        }
        Ok(())
    }

    fn put(&mut self, frame: Frame) -> Result<()> {
        if !self.active.is_empty() || self.active.capacity() == 0 {
            return Err(Error::Internal("working_picture_storage"));
        }
        self.active.push(frame);
        Ok(())
    }
}

#[expect(
    dead_code,
    reason = "The original typed accounting layout fixes cold admission independently of optional counters"
)]
struct SerialAccounting {
    parallel_pictures: u64,
    max_workers: usize,
    headers: Summary,
    decoded_pictures: u64,
    skipped_pictures: u64,
    skipped_picture_hashes: u64,
    skipped_reserved_hashes: u64,
    reconstruction: IntraStats,
    inter: InterStats,
    memory_bytes: usize,
}

struct Pictures<Storage = Slots, Accounting = DecodeSummary> {
    limits: DecodeLimits,
    frame: Storage,
    timestamp: Option<i64>,
    origin: Option<u64>,
    external_memory: usize,
    skipped: Option<u64>,
    stream_remaining: u64,
    picture_remaining: u64,
    summary: Accounting,
}

struct Session<Storage = Slots, Accounting = DecodeSummary> {
    core: StreamCore,
    pictures: Pictures<Storage, Accounting>,
}

impl Pictures {
    fn synchronize(&mut self) -> Result<()> {
        self.frame
            .scheduler
            .drain(&mut self.summary, &mut self.stream_remaining, None)?;
        self.frame.scheduler.flush(
            &mut self.frame.active,
            &mut self.stream_remaining,
            &mut self.picture_remaining,
        )?;
        Ok(())
    }
}

/// Sans-IO Main/Main10 scalar decoder with one DPB and borrowed output callbacks.
pub struct PictureDecoder {
    limits: DecodeLimits,
    session: Vec<Session>,
    state: State,
}

fn worker_count(requested: usize, available: Option<core::num::NonZeroUsize>) -> usize {
    requested
        .clamp(1, 16)
        .min(available.map_or(1, core::num::NonZeroUsize::get))
}

impl PictureDecoder {
    /// Selects reconstruction workers without changing sample or output ordering.
    pub fn with_threads(mut self, threads: usize) -> Self {
        self.limits.threads = worker_count(threads, std::thread::available_parallelism().ok());
        for session in &mut self.session {
            session.pictures.frame.scheduler.join_pending();
            session.pictures.limits.threads = self.limits.threads;
            session.pictures.frame.cache.clear();
        }
        self
    }

    /// Creates an opt-in decoder without allocating picture samples.
    pub fn new(mut limits: DecodeLimits) -> Self {
        limits.threads = worker_count(limits.threads, std::thread::available_parallelism().ok());
        Self {
            limits,
            session: Vec::new(),
            state: State::Active,
        }
    }

    fn fixed_memory(&self) -> Result<usize> {
        core::mem::size_of::<Self>()
            .checked_add(
                core::mem::size_of::<Session<Option<Frame>, SerialAccounting>>()
                    .checked_mul(self.session.capacity().max(1) + 1)
                    .ok_or(Error::Limit("reconstruction_memory"))?,
            )
            .and_then(|n| n.checked_add(coreav_kernels_hevc::SCRATCH_BYTES))
            .and_then(|n| n.checked_add(4 * core::mem::size_of::<crate::params::Sps>()))
            .and_then(|n| n.checked_add(2 * core::mem::size_of::<crate::params::Pps>()))
            .ok_or(Error::Limit("reconstruction_memory"))
    }

    fn fail(&mut self) {
        self.state = State::Failed;
        self.session = Vec::new();
    }

    fn initialize(&mut self) -> Result<()> {
        if !self.session.is_empty() {
            return Ok(());
        }
        Memory::new(self.fixed_memory()?, self.limits.max_memory_bytes)?;
        self.session
            .try_reserve_exact(1)
            .map_err(|_| Error::Limit("decoder_state_allocation"))?;
        Memory::new(self.fixed_memory()?, self.limits.max_memory_bytes)?;
        let limits = self.limits;
        self.session.push(Session {
            core: StreamCore::new_reconstruction(limits.headers),
            pictures: Pictures {
                limits,
                frame: Slots::default(),
                timestamp: None,
                origin: None,
                external_memory: 0,
                skipped: None,
                stream_remaining: limits.max_stream_bins,
                picture_remaining: limits.max_picture_bins,
                summary: DecodeSummary::default(),
            },
        });
        Ok(())
    }

    /// Feeds one original complete NAL and emits completed pictures in presentation order.
    ///
    /// Supply a timestamp on the first slice of a picture. Any supplied later-slice
    /// timestamp must agree. Its time base and container edit mapping remain the caller's.
    /// An error closes the session and releases its pictures; no concealment is attempted.
    pub fn read_nal(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        mut emit: impl FnMut(DecodedPicture<'_>) -> Result<()>,
    ) -> Result<()> {
        self.read(bytes, timestamp, None, |picture, _| emit(picture))
    }

    /// Feeds an original NAL with an opaque picture identity, independently of PTS.
    /// Later slices with an identity must match the first slice's identity.
    pub fn read_nal_with_origin(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        origin: u64,
        emit: impl FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>,
    ) -> Result<()> {
        self.read(bytes, timestamp, Some(origin), emit)
    }

    /// Feeds a NAL while allowing complete-reference picture jobs to overlap.
    ///
    /// Unlike `read_nal`, reconstruction failures may be deferred until a required
    /// reference, output, reservation or final drain synchronizes the serial prefix.
    /// Callbacks run only on the caller, and `finish_with_output` is required.
    pub fn read_nal_scheduled(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        origin: Option<u64>,
        emit: impl FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>,
    ) -> Result<()> {
        self.read_mode(bytes, timestamp, origin, true, emit)
    }

    #[cfg(feature = "native-hevc-rows-private")]
    #[doc(hidden)]
    pub fn read_nal_rows(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        origin: Option<u64>,
        sink: &mut impl crate::native_rows::RowSink,
    ) -> Result<()> {
        self.read_mode(bytes, timestamp, origin, true, RowsOutput(sink))
    }

    #[cfg(feature = "native-hevc-rows-private")]
    #[doc(hidden)]
    pub fn settle_rows(&mut self, sink: &mut impl crate::native_rows::RowSink) -> Result<()> {
        self.settle_delivery(&mut RowsOutput(sink), true)
    }

    #[cfg(feature = "native-hevc-rows-private")]
    #[doc(hidden)]
    pub fn has_native_rows(&self) -> bool {
        self.session
            .first()
            .is_some_and(|session| session.pictures.frame.scheduler.rows().is_some())
    }

    #[cfg(feature = "native-hevc-rows-private")]
    #[doc(hidden)]
    pub fn receive_rows(&mut self) -> Result<Option<crate::native_rows::RowReceive>> {
        self.session
            .first_mut()
            .and_then(|session| session.pictures.frame.scheduler.rows_mut())
            .map(rows::Rows::receive)
            .transpose()
    }

    #[cfg(feature = "native-hevc-rows-private")]
    #[doc(hidden)]
    pub fn release_rows_memory(&mut self, bytes: usize) -> Result<()> {
        let session = self
            .session
            .first_mut()
            .ok_or(Error::Internal("row_owner"))?;
        if bytes > session.pictures.external_memory {
            return Err(Error::Internal("output_reservation_growth"));
        }
        session.pictures.external_memory = bytes;
        Ok(())
    }

    /// Reconciles deferred reconstruction before reporting a caller-owned input error.
    ///
    /// This does not seal the current picture, emit output or validate end of stream.
    /// Closed sessions have no pending work; an already returned error stays authoritative.
    pub fn synchronize_pending(&mut self) -> Result<()> {
        if self.state != State::Active {
            return Ok(());
        }
        let result = self
            .session
            .first_mut()
            .map_or(Ok(()), |session| session.pictures.synchronize());
        if result.is_err() {
            self.fail();
        }
        result
    }

    fn read(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        origin: Option<u64>,
        emit: impl FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>,
    ) -> Result<()> {
        self.read_mode(bytes, timestamp, origin, false, emit)
    }

    fn read_mode(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        origin: Option<u64>,
        scheduled: bool,
        mut emit: impl Delivery,
    ) -> Result<()> {
        if self.state != State::Active {
            self.fail();
            return Err(Error::InvalidData("picture_decoder_closed"));
        }
        let result = (|| {
            #[cfg(hevc_thread_diagnostics)]
            crate::thread_diagnostics::check_row_threads(self.limits.threads)?;
            self.initialize()?;
            let fixed = self.fixed_memory()?;
            let session = self
                .session
                .first_mut()
                .ok_or(Error::InvalidData("decoder_state"))?;
            session.pictures.frame.scheduler.enabled =
                scheduled && session.pictures.limits.threads > 1;
            if !session.pictures.frame.scheduler.enabled {
                session.pictures.synchronize()?;
            }
            session.core.unshare();
            let needed = fixed
                + session.core.heap_bytes()?
                + session
                    .pictures
                    .frame
                    .as_ref()
                    .map_or(Ok(0), Frame::heap_bytes)?
                + session.pictures.external_memory
                + session
                    .pictures
                    .frame
                    .scheduler
                    .bytes(Some(session.core.dpb()))?
                + bytes.len().saturating_sub(2);
            if needed > session.pictures.limits.max_memory_bytes {
                crate::thread_diagnostic!(barrier MemoryInput);
                session.pictures.synchronize()?;
                session.core.unshare();
            }
            let frame_bytes = session
                .pictures
                .frame
                .as_ref()
                .map_or(Ok(0), Frame::heap_bytes)?;
            let live = fixed
                .checked_add(session.core.heap_bytes()?)
                .and_then(|n| n.checked_add(frame_bytes))
                .and_then(|n| n.checked_add(session.pictures.external_memory))
                .ok_or(Error::Limit("reconstruction_memory"))?;
            let mut memory = Memory::with_reclaim(
                live,
                session.pictures.limits.max_memory_bytes,
                Some(&mut session.pictures.frame.cache),
            )?;
            memory.charge(bytes.len().saturating_sub(2))?;
            session.pictures.summary.memory_bytes =
                session.pictures.summary.memory_bytes.max(memory.peak());
            let available = session
                .pictures
                .limits
                .max_memory_bytes
                .checked_sub(live)
                .ok_or(Error::Limit("reconstruction_memory"))?;
            let available = if scheduled {
                session.pictures.limits.max_memory_bytes
            } else {
                available
            };
            session.core.read_reconstructed(
                bytes,
                available,
                &mut Observer {
                    pictures: &mut session.pictures,
                    fixed,
                    incoming_timestamp: timestamp,
                    incoming_origin: origin,
                    #[cfg(feature = "native-hevc-rows-private")]
                    activation: None,
                    emit: &mut emit,
                },
            )?;
            #[cfg(feature = "native-hevc-rows-private")]
            Observer {
                pictures: &mut session.pictures,
                fixed,
                incoming_timestamp: timestamp,
                incoming_origin: origin,
                activation: None,
                emit: &mut emit,
            }
            .row_enforce(session.core.dpb(), 0)?;
            session.pictures.summary.memory_bytes = session
                .pictures
                .summary
                .memory_bytes
                .max(session.pictures.frame.cache.peak());
            Ok(())
        })();
        let result = match result {
            Err(error) => match self.settle_delivery(&mut emit, true) {
                Err(earlier) => Err(earlier),
                Ok(_) => Err(error),
            },
            result => result,
        };
        if result.is_err() {
            self.fail();
        }
        result
    }

    fn settle_delivery(&mut self, emit: &mut impl Delivery, truncate: bool) -> Result<()> {
        let fixed = self.fixed_memory()?;
        let Some(session) = self.session.first_mut() else {
            return Ok(());
        };
        #[cfg(feature = "native-hevc-rows-private")]
        {
            let mut observer = Observer {
                pictures: &mut session.pictures,
                fixed,
                incoming_timestamp: None,
                incoming_origin: None,
                activation: None,
                emit,
            };
            let result = observer.row_settle(session.core.dpb(), 0);
            if truncate {
                if let Some(rows) = observer.pictures.frame.scheduler.rows() {
                    let keep = rows
                        .validated
                        .checked_sub(rows.returned)
                        .and_then(|n| usize::try_from(n).ok());
                    if let Some(sink) = observer.emit.row_sink() {
                        sink.truncate(keep.unwrap_or(0));
                    }
                    if keep.is_none() {
                        return Err(Error::Internal("output_ordinal"));
                    }
                }
            }
            result
        }
        #[cfg(not(feature = "native-hevc-rows-private"))]
        {
            let _ = (fixed, emit, truncate);
            session.pictures.synchronize()
        }
    }

    /// Finalizes the last picture and drains output; success requires this call.
    /// Earlier callbacks remain valid if a later NAL, hash or callback fails.
    pub fn finish(
        &mut self,
        mut emit: impl FnMut(DecodedPicture<'_>) -> Result<()>,
    ) -> Result<DecodeSummary> {
        self.finish_with_output(|picture, _| emit(picture))
    }

    /// Drains pictures with callback-scoped output reservations in the same account.
    pub fn finish_with_output(
        &mut self,
        emit: impl FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>,
    ) -> Result<DecodeSummary> {
        self.finish_mode(emit)
    }

    #[cfg(feature = "native-hevc-rows-private")]
    #[doc(hidden)]
    pub fn finish_rows(
        &mut self,
        sink: &mut impl crate::native_rows::RowSink,
    ) -> Result<DecodeSummary> {
        self.finish_mode(RowsOutput(sink))
    }

    fn finish_mode(&mut self, mut emit: impl Delivery) -> Result<DecodeSummary> {
        if self.state == State::Failed {
            return Err(Error::InvalidData("picture_decoder_failed"));
        }
        let result = (|| {
            let fixed = self.fixed_memory()?;
            let session = self
                .session
                .first_mut()
                .ok_or(Error::InvalidData("no_coded_pictures"))?;
            let headers = session.core.finish_reconstructed(&mut Observer {
                pictures: &mut session.pictures,
                fixed,
                incoming_timestamp: None,
                incoming_origin: None,
                #[cfg(feature = "native-hevc-rows-private")]
                activation: None,
                emit: &mut emit,
            })?;
            #[cfg(feature = "native-hevc-rows-private")]
            Observer {
                pictures: &mut session.pictures,
                fixed,
                incoming_timestamp: None,
                incoming_origin: None,
                activation: None,
                emit: &mut emit,
            }
            .row_settle(session.core.dpb(), 0)?;
            #[cfg(not(feature = "native-hevc-rows-private"))]
            session.pictures.synchronize()?;
            session.core.unshare();
            session.pictures.summary.headers = headers;
            session.pictures.summary.memory_bytes = session
                .pictures
                .summary
                .memory_bytes
                .max(session.pictures.frame.cache.peak());
            session.pictures.frame.cache.clear();
            Ok(session.pictures.summary)
        })();
        let result = match result {
            Err(error) => self.settle_delivery(&mut emit, true).and(Err(error)),
            result => result,
        };
        if result.is_err() {
            self.fail();
        } else {
            self.state = State::Finished;
        }
        result
    }

    /// Sets retained external capacity between operations, charging live codec state too.
    /// Reservation failure is terminal, like a decode or output-callback failure.
    pub fn set_external_memory(&mut self, bytes: usize) -> Result<()> {
        if self.state == State::Failed {
            return Err(Error::InvalidData("picture_decoder_failed"));
        }
        let result = (|| {
            self.initialize()?;
            let fixed = self.fixed_memory()?;
            let session = self
                .session
                .first_mut()
                .ok_or(Error::InvalidData("decoder_state"))?;
            let optional = session
                .pictures
                .frame
                .scheduler
                .bytes(Some(session.core.dpb()))?;
            let required = fixed
                .checked_add(session.core.heap_bytes()?)
                .and_then(|n| {
                    n.checked_add(
                        session
                            .pictures
                            .frame
                            .as_ref()
                            .map_or(Ok(0), Frame::heap_bytes)
                            .ok()?,
                    )
                })
                .and_then(|n| n.checked_add(optional))
                .and_then(|n| n.checked_add(bytes))
                .ok_or(Error::Limit("reconstruction_memory"))?;
            if required > self.limits.max_memory_bytes {
                session.pictures.synchronize()?;
                session.core.unshare();
            }
            let frame = session
                .pictures
                .frame
                .as_ref()
                .map_or(Ok(0), Frame::heap_bytes)?;
            let live = fixed
                .checked_add(session.core.heap_bytes()?)
                .and_then(|n| n.checked_add(frame))
                .and_then(|n| {
                    n.checked_add(
                        session
                            .pictures
                            .frame
                            .scheduler
                            .bytes(Some(session.core.dpb()))
                            .ok()?,
                    )
                })
                .ok_or(Error::Limit("reconstruction_memory"))?;
            let mut memory = Memory::with_reclaim(
                live,
                self.limits.max_memory_bytes,
                Some(&mut session.pictures.frame.cache),
            )?;
            memory.charge(bytes)?;
            session.pictures.external_memory = bytes;
            session.pictures.summary.memory_bytes =
                session.pictures.summary.memory_bytes.max(memory.peak());
            Ok(())
        })();
        let result = match result {
            Err(error) => match self
                .session
                .first_mut()
                .map(|session| session.pictures.synchronize())
                .transpose()
            {
                Err(earlier) => Err(earlier),
                Ok(_) => Err(error),
            },
            result => result,
        };
        if result.is_err() {
            self.fail();
        }
        result
    }
}

struct Observer<'a, F> {
    pictures: &'a mut Pictures,
    fixed: usize,
    incoming_timestamp: Option<i64>,
    incoming_origin: Option<u64>,
    #[cfg(feature = "native-hevc-rows-private")]
    activation: Option<(u32, [u8; 32])>,
    emit: &'a mut F,
}

impl<F: Delivery> PictureObserver for Observer<'_, F> {
    #[cfg(feature = "native-hevc-rows-private")]
    fn activated_sps(&mut self, identity: (u32, [u8; 32])) {
        self.activation = Some(identity);
    }
    fn reconcile(&mut self) -> Result<()> {
        crate::thread_diagnostic!(event Reconcile);
        self.pictures.synchronize()
    }

    fn before_picture(
        &mut self,
        dpb: &mut Dpb,
        nal: crate::NalHeader,
        sps: &crate::params::Sps,
    ) -> Result<()> {
        if dpb.picture_outputs(nal, sps) {
            #[cfg(feature = "native-hevc-rows-private")]
            if self
                .pictures
                .frame
                .scheduler
                .rows()
                .is_some_and(|rows| rows.plan.lag != 0)
            {
                return Ok(());
            }
            #[cfg(hevc_thread_diagnostics)]
            if self.pictures.frame.scheduler.has_work() {
                use crate::thread_diagnostics::{Barrier, barrier};
                for (active, reason) in dpb.picture_output_constraints(nal, sps).into_iter().zip([
                    Barrier::OutputRandomAccess,
                    Barrier::OutputCapacity,
                    Barrier::OutputReorder,
                    Barrier::OutputLatency,
                ]) {
                    if active {
                        barrier(reason);
                    }
                }
            }
            self.pictures.synchronize()?;
            dpb.unshare();
        }
        Ok(())
    }

    fn reserve_scratch(&mut self, scratch_bytes: usize, dpb: &mut Dpb) -> Result<()> {
        let optional = self.pictures.frame.scheduler.bytes(Some(dpb))?;
        let current = self
            .pictures
            .frame
            .as_ref()
            .map_or(Ok(0), Frame::heap_bytes)?;
        let needed = self
            .fixed
            .checked_add(dpb.heap_bytes()?)
            .and_then(|n| n.checked_add(current))
            .and_then(|n| n.checked_add(self.pictures.external_memory))
            .and_then(|n| n.checked_add(scratch_bytes))
            .and_then(|n| n.checked_add(optional))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        if needed > self.pictures.limits.max_memory_bytes {
            crate::thread_diagnostic!(barrier MemoryScratch);
            self.pictures.synchronize()?;
            dpb.unshare();
        }
        let frame = self
            .pictures
            .frame
            .as_ref()
            .map_or(Ok(0), Frame::heap_bytes)?;
        let fixed = self
            .fixed
            .checked_add(dpb.heap_bytes()?)
            .and_then(|n| n.checked_add(frame))
            .and_then(|n| n.checked_add(self.pictures.external_memory))
            .and_then(|n| n.checked_add(self.pictures.frame.scheduler.bytes(Some(dpb)).ok()?))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let mut memory = Memory::with_reclaim(
            fixed,
            self.pictures.limits.max_memory_bytes,
            Some(&mut self.pictures.frame.cache),
        )?;
        memory.charge(scratch_bytes)?;
        self.pictures.summary.memory_bytes = self.pictures.summary.memory_bytes.max(memory.peak());
        Ok(())
    }

    fn boundary(&mut self, dpb: &mut Dpb, scratch_bytes: usize) -> Result<()> {
        self.pictures.skipped = None;
        let Some(mut frame) = self.pictures.frame.take() else {
            return Ok(());
        };
        if self.pictures.frame.scheduler.has_input() {
            self.pictures.frame.scheduler.seal(
                frame,
                dpb,
                self.pictures.limits,
                &mut self.pictures.summary,
                &mut self.pictures.stream_remaining,
                scheduler::JobIdentity {
                    prior_bins: self.pictures.limits.max_picture_bins
                        - self.pictures.picture_remaining,
                    timestamp: self.pictures.timestamp,
                    origin: self.pictures.origin,
                },
            )?;
            if dpb.resume_outputs() {
                #[cfg(feature = "native-hevc-rows-private")]
                if self
                    .pictures
                    .frame
                    .scheduler
                    .rows()
                    .is_some_and(|rows| rows.plan.lag != 0)
                {
                    return Ok(());
                }
                #[cfg(hevc_thread_diagnostics)]
                {
                    use crate::thread_diagnostics::{Barrier, barrier};
                    for (active, reason) in dpb
                        .resume_output_constraints()
                        .into_iter()
                        .zip([Barrier::OutputReorder, Barrier::OutputLatency])
                    {
                        if active {
                            barrier(reason);
                        }
                    }
                }
                self.pictures.synchronize()?;
                dpb.unshare();
            }
            return Ok(());
        }
        self.pictures.frame.scheduler.drain(
            &mut self.pictures.summary,
            &mut self.pictures.stream_remaining,
            None,
        )?;
        dpb.unshare();
        let fixed = self
            .fixed
            .checked_add(dpb.heap_bytes()?)
            .and_then(|n| n.checked_add(scratch_bytes))
            .and_then(|n| n.checked_add(self.pictures.external_memory))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let peak = frame.finalize_reclaiming(
            fixed,
            self.pictures.limits.max_memory_bytes,
            Some(&mut self.pictures.frame.cache),
        )?;
        self.pictures.summary.memory_bytes = self
            .pictures
            .summary
            .memory_bytes
            .max(peak)
            .max(self.pictures.frame.cache.peak());
        self.pictures.summary.parallel_pictures += u64::from(frame.workers > 1);
        self.pictures.summary.max_workers = self.pictures.summary.max_workers.max(frame.workers);
        frame.stats.bins = self.pictures.limits.max_picture_bins - self.pictures.picture_remaining;
        frame.stats.memory_bytes = self.pictures.summary.memory_bytes;
        self.pictures
            .summary
            .add(frame.stats, frame.inter_stats())?;
        #[cfg(feature = "native-hevc-rows-private")]
        let index = frame.index();
        dpb.attach(frame.retain(self.pictures.timestamp, self.pictures.origin)?)?;
        #[cfg(feature = "native-hevc-rows-private")]
        self.pictures.frame.scheduler.confirm(index)?;
        Ok(())
    }

    fn slice(&mut self, input: SliceInput<'_>, dpb: &mut Dpb) -> Result<()> {
        if input.header.first {
            if self.incoming_origin.is_some() && self.incoming_origin == self.pictures.origin {
                return Err(Error::InvalidData("multiple_pictures_in_access_unit"));
            }
            self.pictures.timestamp = self.incoming_timestamp;
            self.pictures.origin = self.incoming_origin;
        } else {
            if self.incoming_timestamp.is_some()
                && self.incoming_timestamp != self.pictures.timestamp
            {
                return Err(Error::InvalidData("picture_timestamp_changed"));
            }
            if self.incoming_origin.is_some() && self.incoming_origin != self.pictures.origin {
                return Err(Error::InvalidData("picture_origin_changed"));
            }
        }
        if !dpb.reconstructs(input.index) {
            if self.pictures.skipped != Some(input.index) {
                self.pictures.summary.skipped_pictures = self
                    .pictures
                    .summary
                    .skipped_pictures
                    .checked_add(1)
                    .ok_or(Error::Limit("decode_work_count"))?;
                self.pictures.skipped = Some(input.index);
            }
            return Ok(());
        }
        #[cfg(hevc_thread_diagnostics)]
        if input.header.first {
            crate::thread_diagnostics::row_picture(
                input.index,
                dpb.current_identity(input.index)?.poc,
                input.picture,
            )?;
        }
        crate::thread_diagnostic!(picture input.index);
        #[cfg(feature = "native-hevc-rows-private")]
        if input.header.first
            && self.pictures.frame.scheduler.rows().is_some_and(|rows| {
                rows.plan.lag == 0
                    && rows.serial_received == rows.returned
                    && rows.pending.is_empty()
            })
        {
            self.pictures.frame.scheduler.rows = None;
        }
        #[cfg(feature = "native-hevc-rows-private")]
        if self.pictures.frame.scheduler.rows().is_some()
            && (input.picture.tiles
                || input.picture.wpp
                || self
                    .pictures
                    .frame
                    .scheduler
                    .rows()
                    .is_some_and(|rows| rows.plan.lag == 0))
        {
            self.row_settle(dpb, input.scratch_bytes)?;
            dpb.unshare();
            self.pictures.frame.cache.clear();
            self.pictures.frame.scheduler.enabled = false;
        }
        #[cfg(hevc_thread_diagnostics)]
        if input.header.first {
            match input.header.slice_type {
                Some(0) => crate::thread_diagnostic!(event PictureB),
                Some(1) => crate::thread_diagnostic!(event PictureP),
                Some(2) => crate::thread_diagnostic!(event PictureI),
                _ => {}
            }
        }
        if !self.pictures.frame.scheduler.enabled || input.picture.wpp {
            if input.picture.wpp {
                crate::thread_diagnostic!(barrier Wpp);
            } else {
                crate::thread_diagnostic!(barrier WorkersDisabled);
            }
            self.pictures.synchronize()?;
            dpb.unshare();
        }
        #[cfg(feature = "native-hevc-rows-private")]
        let row_references = self.pictures.frame.scheduler.enabled
            && self
                .pictures
                .frame
                .scheduler
                .rows()
                .is_some_and(|rows| rows.plan.lag != 0);
        #[cfg(not(feature = "native-hevc-rows-private"))]
        let row_references = false;
        if !row_references {
            self.pictures.frame.scheduler.required_references(
                dpb,
                &mut self.pictures.summary,
                &mut self.pictures.stream_remaining,
            )?;
        }
        if self.pictures.frame.is_none() {
            let bytes = (u64::from(input.picture.visible_width)
                * u64::from(input.picture.visible_height))
            .checked_mul(3)
            .and_then(|n| n.checked_mul(if input.picture.bit_depth == 10 { 2 } else { 1 }))
            .and_then(|n| n.checked_div(2))
            .ok_or(Error::Limit("output_frame_bytes"))?;
            if bytes > self.pictures.limits.max_frame_bytes {
                return Err(Error::Limit("output_frame_bytes"));
            }
            let identity = dpb.current_identity(input.index)?;
            #[cfg(feature = "native-hevc-rows-private")]
            {
                self.row_admit(
                    &input,
                    dpb,
                    usize::try_from(bytes).map_err(|_| Error::Limit("output_frame_bytes"))?,
                )?;
                if self.pictures.frame.scheduler.rows().is_some_and(|rows| {
                    rows.plan.lag == 0 || input.picture.tiles || input.picture.wpp
                }) {
                    self.pictures.frame.scheduler.enabled = false;
                }
            }
            let fixed = self
                .fixed
                .checked_add(dpb.heap_bytes()?)
                .and_then(|n| n.checked_add(self.pictures.external_memory))
                .ok_or(Error::Limit("reconstruction_memory"))?;
            self.pictures.frame.prepare()?;
            let optional = self.pictures.frame.scheduler.bytes(Some(dpb))?;
            let create = Frame::new_full(
                &input,
                identity.poc,
                dpb.current_references(),
                fixed
                    .checked_add(optional)
                    .ok_or(Error::Limit("reconstruction_memory"))?,
                self.pictures.limits.max_memory_bytes,
                Some(&mut self.pictures.frame.cache),
            );
            let mut frame = match create {
                Err(Error::Limit("reconstruction_memory")) if optional != 0 => {
                    crate::thread_diagnostic!(barrier MemoryFrame);
                    self.pictures.synchronize()?;
                    dpb.unshare();
                    Frame::new_full(
                        &input,
                        identity.poc,
                        dpb.current_references(),
                        self.fixed + dpb.heap_bytes()? + self.pictures.external_memory,
                        self.pictures.limits.max_memory_bytes,
                        Some(&mut self.pictures.frame.cache),
                    )?
                }
                result => result?,
            };
            frame.deadline = self.pictures.limits.deadline;
            self.pictures.frame.put(frame)?;
            self.pictures.picture_remaining = self.pictures.limits.max_picture_bins;
        }
        if self.pictures.frame.scheduler.enabled && !input.picture.wpp {
            self.pictures.frame.cache.clear();
            if self
                .pictures
                .frame
                .scheduler
                .full(self.pictures.limits.threads)
            {
                crate::thread_diagnostic!(barrier WindowFull);
                self.pictures.frame.scheduler.drain(
                    &mut self.pictures.summary,
                    &mut self.pictures.stream_remaining,
                    None,
                )?;
                dpb.unshare();
            }
            let fixed = self.fixed
                + dpb.heap_bytes()?
                + self.pictures.external_memory
                + self.pictures.frame.scheduler.bytes(Some(dpb))?
                - self.pictures.frame.scheduler.current_reservation()
                + dpb.sharing_bytes();
            let Slots {
                active, scheduler, ..
            } = &mut self.pictures.frame;
            let frame = active.first().ok_or(Error::Internal("picture_job_frame"))?;
            if scheduler.queue(
                input,
                frame,
                fixed,
                self.pictures.limits.max_memory_bytes,
                self.pictures.stream_remaining,
                self.pictures.picture_remaining,
            )? {
                if !self.pictures.frame.scheduler.references(
                    dpb,
                    &mut self.pictures.summary,
                    &mut self.pictures.stream_remaining,
                )? {
                    self.pictures.frame.scheduler.discard_current();
                } else {
                    self.pictures.summary.memory_bytes = self.pictures.summary.memory_bytes.max(
                        self.fixed
                            + dpb.heap_bytes()?
                            + self.pictures.external_memory
                            + self
                                .pictures
                                .frame
                                .as_ref()
                                .map_or(Ok(0), Frame::heap_bytes)?
                            + self.pictures.frame.scheduler.bytes(Some(dpb))?,
                    );
                    return Ok(());
                }
            }
            self.pictures.synchronize()?;
            dpb.unshare();
        }
        let fixed = self.fixed + dpb.heap_bytes()? + self.pictures.external_memory;
        #[cfg(feature = "native-hevc-rows-private")]
        let fixed = if self.pictures.frame.scheduler.rows().is_some() {
            fixed
                .checked_add(self.pictures.frame.scheduler.bytes(Some(dpb))?)
                .ok_or(Error::Limit("reconstruction_memory"))?
        } else {
            fixed
        };
        let decode_limit = self.pictures.limits.max_memory_bytes;
        let decode_threads = self.pictures.limits.threads;
        #[cfg(feature = "native-hevc-rows-private")]
        let decode_limit = match self.pictures.frame.scheduler.rows() {
            Some(rows) => rows
                .plan
                .lag
                .checked_mul(rows.plan.frame_bytes)
                .and_then(|debt| rows.plan.reserved.checked_add(debt))
                .and_then(|limit| {
                    limit.checked_sub(rows.plan.ceiling.checked_mul(rows::ATOMIC_RUNTIME_BYTES)?)
                })
                .filter(|limit| *limit <= decode_limit)
                .ok_or(Error::Internal("row_admission"))?,
            None => decode_limit,
        };
        #[cfg(feature = "native-hevc-rows-private")]
        let decode_threads = self
            .pictures
            .frame
            .scheduler
            .rows()
            .map_or(decode_threads, |rows| decode_threads.min(rows.plan.ceiling));
        let Slots { active, cache, .. } = &mut self.pictures.frame;
        let frame = active
            .first_mut()
            .ok_or(Error::InvalidData("missing_working_picture"))?;
        let allowance = self
            .pictures
            .stream_remaining
            .min(self.pictures.picture_remaining);
        let mut budget = allowance;
        let result = crate::reconstruction::caught(|| {
            frame.decode_full(
                input,
                dpb,
                &mut budget,
                fixed,
                decode_limit,
                decode_threads,
                Some(cache),
            )
        });
        let consumed = allowance - budget;
        self.pictures.stream_remaining -= consumed;
        self.pictures.picture_remaining -= consumed;
        #[cfg(feature = "native-hevc-rows-private")]
        if self.pictures.frame.scheduler.rows().is_some() {
            self.pictures.frame.cache.clear();
        }
        match result {
            Ok(peak) => {
                self.pictures.summary.memory_bytes = self
                    .pictures
                    .summary
                    .memory_bytes
                    .max(peak)
                    .max(self.pictures.frame.cache.peak());
                Ok(())
            }
            Err(Error::Limit("cabac_bin_budget")) if self.pictures.stream_remaining == 0 => {
                Err(Error::Limit("stream_cabac_bin_budget"))
            }
            Err(error) => Err(error),
        }
    }

    fn sei(&mut self, info: SeiInfo, dpb: &Dpb, scratch_bytes: usize) -> Result<()> {
        if info.picture_hash.is_none() && info.reserved_hashes == 0 {
            return Ok(());
        }
        if self.pictures.frame.scheduler.queue_hash(info) {
            return Ok(());
        }
        if self.pictures.frame.scheduler.has_input() {
            crate::thread_diagnostic!(barrier DuplicateHash);
            self.pictures.synchronize()?;
        }
        let Slots { active, cache, .. } = &mut self.pictures.frame;
        if let Some(frame) = active.first_mut() {
            let fixed = self
                .fixed
                .checked_add(dpb.heap_bytes()?)
                .and_then(|n| n.checked_add(scratch_bytes))
                .and_then(|n| n.checked_add(self.pictures.external_memory))
                .ok_or(Error::Limit("reconstruction_memory"))?;
            let peak = frame.hash_reclaiming(
                info,
                fixed,
                self.pictures.limits.max_memory_bytes,
                Some(cache),
            )?;
            self.pictures.summary.memory_bytes = self
                .pictures
                .summary
                .memory_bytes
                .max(peak)
                .max(cache.peak());
        } else if self.pictures.skipped.is_some() {
            self.pictures.summary.skipped_picture_hashes = self
                .pictures
                .summary
                .skipped_picture_hashes
                .checked_add(u64::from(info.picture_hash.is_some()))
                .ok_or(Error::Limit("decode_work_count"))?;
            self.pictures.summary.skipped_reserved_hashes = self
                .pictures
                .summary
                .skipped_reserved_hashes
                .checked_add(u64::from(info.reserved_hashes))
                .ok_or(Error::Limit("decode_work_count"))?;
        } else if info.picture_hash.is_some() || info.reserved_hashes != 0 {
            return Err(Error::InvalidData("picture_hash_without_samples"));
        }
        Ok(())
    }

    fn output(&mut self, emission: Emission<'_>, scratch_bytes: usize) -> Result<()> {
        #[cfg(feature = "native-hevc-rows-private")]
        if self.pictures.frame.scheduler.rows().is_some() {
            return self.row_output(emission, scratch_bytes);
        }
        self.pictures.frame.scheduler.drain(
            &mut self.pictures.summary,
            &mut self.pictures.stream_remaining,
            Some(emission.event.picture_index),
        )?;
        let samples = emission
            .picture
            .or_else(|| {
                emission
                    .publication
                    .and_then(|publication| publication.picture())
            })
            .ok_or(Error::InvalidData("missing_output_samples"))?;
        let picture = samples.view();
        let frame = self
            .pictures
            .frame
            .as_ref()
            .map_or(Ok(0), Frame::heap_bytes)?;
        let codec_bytes = self
            .fixed
            .checked_add(emission.dpb.heap_bytes()?)
            .and_then(|n| n.checked_add(frame))
            .and_then(|n| n.checked_add(scratch_bytes))
            .and_then(|n| {
                n.checked_add(
                    self.pictures
                        .frame
                        .scheduler
                        .bytes(Some(emission.dpb))
                        .ok()?,
                )
            })
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let mut memory = OutputMemory {
            codec_bytes,
            reserved: &mut self.pictures.external_memory,
            limit: self.pictures.limits.max_memory_bytes,
            reclaim: Some(&mut self.pictures.frame.cache),
        };
        self.emit.output(
            DecodedPicture {
                index: picture.index,
                poc: emission.event.poc,
                timestamp: samples.timestamp(),
                origin: samples.origin(),
                info: picture.info,
                planes: picture.planes,
                stats: picture.stats,
                inter: samples.inter_stats(),
            },
            &mut memory,
        )?;
        let mut used = Memory::with_reclaim(
            codec_bytes,
            self.pictures.limits.max_memory_bytes,
            Some(&mut self.pictures.frame.cache),
        )?;
        used.charge(self.pictures.external_memory)?;
        self.pictures.summary.memory_bytes = self
            .pictures
            .summary
            .memory_bytes
            .max(used.peak())
            .max(self.pictures.frame.cache.peak());
        Ok(())
    }
}

impl DecodeSummary {
    pub(crate) fn add(&mut self, reconstruction: IntraStats, inter: InterStats) -> Result<()> {
        self.decoded_pictures = self
            .decoded_pictures
            .checked_add(1)
            .ok_or(Error::Limit("decode_work_count"))?;
        macro_rules! sum {
            ($target:ident, $source:ident, $($field:ident),+ $(,)?) => {
                $(self.$target.$field = self.$target.$field.checked_add($source.$field)
                    .ok_or(Error::Limit("decode_work_count"))?;)+
            };
        }
        sum!(
            reconstruction,
            reconstruction,
            ctus,
            coding_units,
            transform_units,
            nonzero_transform_units,
            transform_skips,
            transquant_bypass_units,
            pcm_units,
            dependent_segments,
            tile_restarts,
            wpp_restarts,
            qp_deltas,
            nonzero_qp_deltas,
            scaling_list_blocks,
            sign_hidden_groups,
            dst_blocks,
            bins,
            picture_hashes,
            reserved_hashes,
            sao_merges
        );
        sum!(
            inter,
            inter,
            skip_units,
            prediction_units,
            p_units,
            b_units,
            bi_units,
            merge_units,
            amvp_lists,
            temporal_units,
            weighted_units,
            fractional_luma_units,
            fractional_chroma_units,
            negative_motion_units,
            long_term_units,
            list_modification_units
        );
        for (target, source) in [
            (
                &mut self.reconstruction.deblock_edges,
                reconstruction.deblock_edges,
            ),
            (
                &mut self.reconstruction.deblock_changed_samples,
                reconstruction.deblock_changed_samples,
            ),
            (
                &mut self.reconstruction.sao_blocks,
                reconstruction.sao_blocks,
            ),
            (
                &mut self.reconstruction.sao_changed_samples,
                reconstruction.sao_changed_samples,
            ),
            (
                &mut self.inter.weighted_changed_samples,
                inter.weighted_changed_samples,
            ),
            (&mut self.inter.boundary_strengths, inter.boundary_strengths),
        ] {
            add_counts(target, source)?;
        }
        add_counts(
            &mut self.reconstruction.luma_modes,
            reconstruction.luma_modes,
        )?;
        add_counts(
            &mut self.reconstruction.chroma_modes,
            reconstruction.chroma_modes,
        )?;
        add_counts(
            &mut self.reconstruction.dct_blocks,
            reconstruction.dct_blocks,
        )?;
        add_counts(&mut self.inter.partitions, inter.partitions)?;
        for (target, source) in self
            .inter
            .boundary_changed_samples
            .iter_mut()
            .zip(inter.boundary_changed_samples)
        {
            add_counts(target, source)?;
        }
        self.reconstruction.memory_bytes = self
            .reconstruction
            .memory_bytes
            .max(reconstruction.memory_bytes);
        Ok(())
    }
}

fn add_counts<const N: usize>(target: &mut [u64; N], source: [u64; N]) -> Result<()> {
    for (target, source) in target.iter_mut().zip(source) {
        *target = target
            .checked_add(source)
            .ok_or(Error::Limit("decode_work_count"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_workers_respect_the_cpu_cap_and_unavailable_cpu_fallback() {
        for (requested, available, expected) in [
            (16, Some(4), 4),
            (8, Some(4), 4),
            (2, Some(4), 2),
            (0, Some(4), 1),
            (usize::MAX, Some(64), 16),
            (16, Some(1), 1),
            (16, None, 1),
            (0, None, 1),
        ] {
            assert_eq!(
                worker_count(requested, available.and_then(core::num::NonZeroUsize::new)),
                expected
            );
        }
    }

    #[test]
    fn public_worker_boundaries_normalize_limits_and_existing_sessions() -> Result<()> {
        let expected = worker_count(usize::MAX, std::thread::available_parallelism().ok());
        let mut decoder = PictureDecoder::new(DecodeLimits {
            threads: usize::MAX,
            ..DecodeLimits::default()
        });
        assert_eq!(decoder.limits.threads, expected);
        decoder.initialize()?;
        decoder = decoder.with_threads(0);
        assert_eq!(decoder.limits.threads, 1);
        assert_eq!(decoder.session[0].pictures.limits.threads, 1);
        decoder = decoder.with_threads(usize::MAX);
        assert_eq!(decoder.limits.threads, expected);
        assert_eq!(decoder.session[0].pictures.limits.threads, expected);
        Ok(())
    }

    #[test]
    fn cached_storage_fits_the_unchanged_cold_state_envelope() -> Result<()> {
        let mut decoder = PictureDecoder::new(DecodeLimits::default());
        let cold = decoder.fixed_memory()?;
        for stage in 0..3 {
            let capacity = decoder.session.capacity().max(1);
            let reserved =
                (capacity + 1) * core::mem::size_of::<Session<Option<Frame>, SerialAccounting>>();
            let physical = capacity * core::mem::size_of::<Session>();
            let active = decoder
                .session
                .first()
                .map_or(0, |session| session.pictures.frame.active.capacity());
            assert!((capacity + 1) * core::mem::size_of::<Session>() <= reserved);
            assert!(physical + (active + 1) * core::mem::size_of::<Frame>() <= reserved);
            assert_eq!(decoder.fixed_memory()?, cold);
            if stage == 0 {
                decoder.initialize()?;
            }
            if stage == 1 {
                decoder
                    .session
                    .first_mut()
                    .ok_or(Error::Internal("test_session"))?
                    .pictures
                    .frame
                    .prepare()?;
            }
        }
        assert_eq!(
            core::mem::size_of::<Vec<Session>>(),
            core::mem::size_of::<Vec<Session<Option<Frame>>>>()
        );
        Ok(())
    }

    #[test]
    fn output_capacity_evicts_idle_memory_without_shifting_the_cold_limit() -> Result<()> {
        struct Idle(Vec<u8>);
        impl ReclaimMemory for Idle {
            fn retained_bytes(&mut self, mandatory: usize, limit: usize) -> Result<usize> {
                if mandatory + self.0.capacity() > limit {
                    self.0 = Vec::new();
                }
                Ok(self.0.capacity())
            }
        }
        let mut idle = Idle(vec![0; 128]);
        let mut reserved = 0;
        {
            let mut memory = OutputMemory {
                codec_bytes: 128,
                reserved: &mut reserved,
                limit: 256,
                reclaim: Some(&mut idle),
            };
            memory.reserve(128)?;
            assert_eq!(
                memory.reserve(1),
                Err(Error::Limit("reconstruction_memory"))
            );
        }
        assert_eq!(reserved, 128);
        assert_eq!(idle.0.capacity(), 0);
        Ok(())
    }

    #[test]
    fn skipped_picture_scratch_preserves_cold_rbsp_and_entry_limits_with_idle_workers() -> Result<()>
    {
        fn seed_cache(sets: &[Vec<u8>; 3], cra: &[u8]) -> Result<Cache> {
            let mut core = StreamCore::new(Limits::default());
            for set in sets {
                core.read_nal(set, |_| Ok(()))?;
            }
            let mut cache = Cache::default();
            core.read_selected(
                cra,
                |_| Ok(()),
                Some(0),
                usize::MAX,
                |input| {
                    cache
                        .frames_for_test()
                        .push(Frame::new(&input, 0, usize::MAX)?);
                    Ok(())
                },
            )?;
            assert_eq!(cache.frames_for_test().len(), 1);
            Ok(cache)
        }
        fn prepare_decoder(
            decoder: &mut PictureDecoder,
            sets: &[Vec<u8>; 3],
            cra: &[u8],
            cache: Cache,
        ) -> Result<()> {
            let session = &mut decoder.session[0];
            session.core = StreamCore::new(Limits::default());
            for set in sets {
                session.core.read_nal(set, |_| Ok(()))?;
            }
            session.core.read_nal(cra, |_| Ok(()))?;
            session.pictures.frame.cache = cache;
            Ok(())
        }
        let sets = crate::tests::synthetic_parameter_nals_with_wpp(true);
        let cra = crate::tests::intra_first_with_wpp(21, 1, Some(0));
        let retained = seed_cache(&sets, &cra)?.retained_bytes(0, usize::MAX)?;
        assert!(retained > 0);
        for entries in [0, 1] {
            let skipped = crate::tests::intra_first_with_wpp(8, 0, Some(entries));
            let rbsp = crate::nal::rbsp(&skipped, &Limits::default())?;
            let required = (skipped.len() - 2).max(rbsp.capacity() + entries as usize * 8);
            for warm in [false, true] {
                for extra in [0, retained] {
                    for below in [true, false] {
                        let cache = if warm {
                            seed_cache(&sets, &cra)?
                        } else {
                            Cache::default()
                        };
                        let mut decoder = PictureDecoder::new(DecodeLimits::default());
                        decoder.initialize()?;
                        prepare_decoder(&mut decoder, &sets, &cra, cache)?;
                        let fixed = decoder.fixed_memory()?;
                        let session = &mut decoder.session[0];
                        let mandatory = fixed + session.core.heap_bytes()?;
                        session.pictures.limits.max_memory_bytes =
                            mandatory + required + extra - usize::from(below);
                        let result = decoder.read_nal(&skipped, None, |_| {
                            Err(Error::Internal("unexpected_output"))
                        });
                        if below && extra == 0 {
                            assert_eq!(result, Err(Error::Limit("reconstruction_memory")));
                        } else {
                            result?;
                            let session = &mut decoder.session[0];
                            assert_eq!(session.pictures.summary.skipped_pictures, 1);
                            assert_eq!(session.pictures.skipped, Some(1));
                            assert!(session.pictures.frame.is_none());
                            let kept =
                                session.pictures.frame.cache.retained_bytes(0, usize::MAX)?;
                            assert_eq!(kept != 0, warm && extra == retained && !below);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn skipped_hash_messages_never_receive_checked_sample_credit() -> Result<()> {
        let mut decoder = PictureDecoder::new(DecodeLimits::default());
        decoder.initialize()?;
        let fixed = decoder.fixed_memory()?;
        let session = decoder
            .session
            .first_mut()
            .ok_or(Error::InvalidData("test_session"))?;
        session.pictures.skipped = Some(7);
        let mut emit = |_: DecodedPicture<'_>, _: &mut OutputMemory<'_>| {
            Err(Error::InvalidData("unexpected_output"))
        };
        let mut observer = Observer {
            pictures: &mut session.pictures,
            fixed,
            incoming_timestamp: None,
            incoming_origin: None,
            #[cfg(feature = "native-hevc-rows-private")]
            activation: None,
            emit: &mut emit,
        };
        let mut dpb = Dpb::new_reconstruction();
        let defined = SeiInfo {
            messages: 1,
            picture_hash: Some(crate::PictureHash::Crc([[0; 2]; 3])),
            reserved_hashes: 0,
        };
        observer.sei(defined, &dpb, 0)?;
        observer.sei(
            SeiInfo {
                messages: 1,
                reserved_hashes: 1,
                ..SeiInfo::default()
            },
            &dpb,
            0,
        )?;
        assert_eq!(observer.pictures.summary.skipped_picture_hashes, 1);
        assert_eq!(observer.pictures.summary.skipped_reserved_hashes, 1);
        assert_eq!(observer.pictures.summary.reconstruction.picture_hashes, 0);
        assert_eq!(observer.pictures.summary.reconstruction.reserved_hashes, 0);
        observer.boundary(&mut dpb, 0)?;
        assert_eq!(
            observer.sei(defined, &dpb, 0),
            Err(Error::InvalidData("picture_hash_without_samples"))
        );
        observer.pictures.skipped = Some(8);
        observer.pictures.summary.skipped_picture_hashes = u64::MAX;
        assert_eq!(
            observer.sei(defined, &dpb, 0),
            Err(Error::Limit("decode_work_count"))
        );
        Ok(())
    }

    #[test]
    fn construction_keeps_parameter_slots_off_the_callers_stack() {
        assert!(core::mem::size_of::<super::PictureDecoder>() <= 128);
        let decoder = super::PictureDecoder::new(super::DecodeLimits::default());
        assert!(decoder.session.is_empty());
        assert_eq!(decoder.session.capacity(), 0);
    }
}
