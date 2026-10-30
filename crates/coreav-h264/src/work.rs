//! The decoding of one picture's slices into its bands, on the caller's
//! thread or on a worker thread (`docs/design/h264.md`, "Threads, and what
//! stays on the caller's thread").
//!
//! Everything that depends on decoded samples is here: the macroblocks,
//! their deblocking and the publication of the picture's bands. The
//! decoder ([`crate::Decoder`]) keeps what depends only on headers, so the
//! samples are the same whichever thread decodes a picture.
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

use coreav_kernels_h264::{Kernels, Macroblocks};

use crate::deblock::{SliceFilter, ThresholdTable, deblock_rows};
use crate::dpb::RefPic;
use crate::error::{Error, Result};
use crate::inter::{InterTables, MotionCtx, Scratch};
use crate::mb::{
    BAND_VIEWS, KeptViews, MbData, MbInfo, NbInfo, SliceDecoder, band_shift, hints, ring_entries,
    whole_references,
};
use crate::nb::NbCache;
use crate::params::{Pps, Sps};
use crate::picture::{BandData, Canvas};
use crate::recon::{CoeffTables, LevelScales};
use crate::slice::SliceHeader;
use crate::tier::Choice;

/// One slice of a picture, with everything its decoding reads.
pub(crate) struct SliceJob {
    /// The slice's RBSP, owned where the slice goes to a worker; empty
    /// where the caller's thread decodes it from its own buffer.
    pub(crate) rbsp: Vec<u8>,
    pub(crate) hdr: SliceHeader,
    /// Shared with the decoder and the picture's other slices.
    pub(crate) sps: Arc<Sps>,
    pub(crate) pps: Arc<Pps>,
    pub(crate) refs: [Vec<RefPic>; 2],
    /// 1 + the index of the slice in its picture.
    pub(crate) slice_id: u32,
    /// The picture order count of the picture as it is decoded, before a
    /// marking operation 5 resets it (clause 8.2.1).
    pub(crate) current_poc: i32,
}

/// The buffers a decoding keeps from one picture to the next.
#[derive(Default)]
pub(crate) struct Spare {
    pub(crate) mb: Option<Box<MbData>>,
    pub(crate) scratch: Option<Box<Scratch>>,
    /// The ring of what macroblocks A and B read
    /// ([`SliceDecoder::ring`]), never cleared: an entry is read only
    /// after the slice that reads it wrote it.
    pub(crate) ring: Vec<NbInfo>,
    pub(crate) coeff_tables: Option<Box<CoeffTables>>,
    /// The deblocking thresholds of the last filter offsets
    /// (`deblock.rs`), kept from one call of the filter to the next and
    /// from one picture to the next.
    pub(crate) thresholds: Option<Box<ThresholdTable>>,
}

/// The decoding state of one picture.
pub(crate) struct PictureWork<K> {
    kernels: Arc<K>,
    /// The instance of the hot paths its slices run.
    choice: Choice,
    canvas: Canvas,
    mbs: Vec<MbInfo>,
    filters: Vec<SliceFilter>,
    /// Macroblock rows that the deblocking filter has already run over.
    deblocked: usize,
    /// The last macroblock a slice started, which the next slice must
    /// start after.
    last_started: Option<usize>,
    /// chroma_qp_index_offset of the picture's first slice, which filters
    /// the rows left when the picture is finished.
    chroma_qp_offset: [i32; 2],
    spare: Spare,
}

impl<K: Kernels> PictureWork<K> {
    /// The decoding of the picture `canvas` writes.
    pub(crate) fn new(
        kernels: Arc<K>,
        choice: Choice,
        canvas: Canvas,
        chroma_qp_offset: [i32; 2],
    ) -> Self {
        let g = canvas.geometry();
        PictureWork {
            kernels,
            choice,
            canvas,
            mbs: vec![MbInfo::NONE; g.mb_width * g.mb_height],
            filters: Vec::new(),
            deblocked: 0,
            last_started: None,
            chroma_qp_offset,
            spare: Spare::default(),
        }
    }

    /// Takes the buffers an earlier picture left.
    pub(crate) fn adopt(&mut self, spare: Spare) {
        self.spare = spare;
    }

    /// Decodes one slice, whose RBSP is `rbsp`.
    pub(crate) fn decode_slice(&mut self, job: &SliceJob, rbsp: &[u8]) -> Result<()> {
        let hdr = &job.hdr;
        let index = (job.slice_id as usize).saturating_sub(1);
        if self.filters.len() <= index {
            // A slice the caller's thread refused never arrives here; its
            // filter is never read, since no macroblock carries its id.
            self.filters.resize(index + 1, SliceFilter::default());
        }
        if let Some(slot) = self.filters.get_mut(index) {
            *slot = SliceFilter {
                disable_idc: hdr.disable_deblocking_filter_idc,
                offset_a: hdr.filter_offset_a,
                offset_b: hdr.filter_offset_b,
            };
        }
        // Macroblocks are decoded in increasing address order: the rows
        // behind the last one started may be filtered and published
        // already (`docs/design/h264.md`, "Bit-exact whatever the thread
        // count").
        if self
            .last_started
            .is_some_and(|last| hdr.first_mb as usize <= last)
        {
            return Err(Error::Invalid("a macroblock decoded twice"));
        }
        let scale = self.choice.level_scale(&job.pps);
        let geometry = self.canvas.geometry();
        let mb = self.spare.mb.take().unwrap_or_else(MbData::new);
        let scratch = self.spare.scratch.take().unwrap_or_else(Scratch::new);
        let entries = ring_entries(geometry.mb_width);
        let mut ring = std::mem::take(&mut self.spare.ring);
        if ring.len() != entries {
            ring = vec![NbInfo::NONE; entries];
        }
        let mut coeff_tables = self
            .spare
            .coeff_tables
            .take()
            .unwrap_or_else(CoeffTables::new);
        coeff_tables.fit(&job.pps, &scale);
        let thresholds = self
            .spare
            .thresholds
            .get_or_insert_with(ThresholdTable::boxed);
        let (whole, colocated_whole) =
            whole_references(&job.refs, geometry.mb_width, geometry.mb_height);
        let mut slice = SliceDecoder {
            kernels: &*self.kernels,
            sps: &job.sps,
            pps: &job.pps,
            hdr,
            scale: &scale,
            slice_id: job.slice_id,
            mb_w: geometry.mb_width,
            mb_h: geometry.mb_height,
            canvas: &mut self.canvas,
            out: BandData::default(),
            held: None,
            geometry,
            band: usize::MAX,
            luma_first: 0,
            chroma_first: 0,
            motion_base: 0,
            stride: geometry.mb_width * 16,
            luma_origin: 0,
            chroma_origin: 0,
            macroblocks: Macroblocks::NONE,
            mbs: &mut self.mbs,
            ring,
            ring_mask: entries - 1,
            filters: &self.filters,
            deblocked: &mut self.deblocked,
            thresholds,
            refs: &job.refs,
            whole,
            band_views: KeptViews([[None; BAND_VIEWS]; 2]),
            colocated_whole,
            default_weights: hdr
                .pred_weights
                .as_ref()
                .map_or([u32::MAX; 2], |table| table.default_weights()),
            qp: hdr.slice_qp,
            prev_qp_delta_nonzero: false,
            addr: usize::MAX,
            mbx: 0,
            // No row yet: the first macroblock enters its row.
            mby: usize::MAX,
            mb,
            cur: MbInfo::NONE,
            cur_nb: NbInfo::NONE,
            nb: NbCache::NONE,
            nbs: [None; 4],
            ctx: MotionCtx::EMPTY,
            derived: 0,
            tables: InterTables::new(&job.refs, hdr.slice_type, &job.pps, job.current_poc),
            motion_first: None,
            scratch,
            cbf_nb: [0; 2],
            coeff_tables: &coeff_tables,
            level_scales: LevelScales::NONE,
            hints: hints(geometry.mb_width * geometry.mb_height, geometry.bands()),
            band_shift: band_shift(&job.refs, geometry.mb_width, geometry.mb_height),
        };
        let result = slice.decode(rbsp, self.choice);
        slice.store_band();
        if slice.addr != usize::MAX {
            self.last_started = Some(slice.addr);
        }
        // The per-macroblock state is kept between slices and cleared block
        // by block, trusting each macroblock to have recorded what it wrote.
        // A slice that failed may have stopped between writing and
        // recording, so its state is not kept.
        self.spare.mb = result.is_ok().then_some(slice.mb);
        self.spare.scratch = Some(slice.scratch);
        self.spare.ring = slice.ring;
        self.spare.coeff_tables = Some(coeff_tables);
        result
    }

    /// Filters the rows that are left and publishes every band: the
    /// picture is complete. Returns the buffers to keep for the next.
    pub(crate) fn finish(mut self) -> (Result<()>, Spare) {
        let rows = self.deblocked..self.canvas.geometry().mb_height;
        let thresholds = self
            .spare
            .thresholds
            .get_or_insert_with(ThresholdTable::boxed);
        let filtered = deblock_rows(
            &*self.kernels,
            &mut self.canvas,
            &self.mbs,
            &self.filters,
            self.chroma_qp_offset,
            thresholds,
            rows,
        );
        // Published whether or not the filter finished, so that no picture
        // predicted from this one waits for it.
        let published = self.canvas.publish_all();
        (filtered.and(published), std::mem::take(&mut self.spare))
    }
}

/// What the caller's thread sends a worker.
pub(crate) enum Message<K> {
    /// A picture to decode.
    Start(Box<PictureWork<K>>),
    /// One of its slices.
    Slice(Box<SliceJob>),
    /// The picture has no more slices.
    Finish,
}

/// What a worker reports for a finished picture: the first error of its
/// slices, if one failed.
pub(crate) struct Done {
    pub(crate) error: Option<Error>,
}

/// Messages a worker's channel holds before the caller's thread waits.
pub(crate) const QUEUE_MESSAGES: usize = 256;

/// What one queued slice holds besides its data, at most: the job, the
/// lists of its header (at most 33 reference list modifications a list
/// and 64 marking operations, in vectors that may have grown to twice
/// that), its reference lists (at most 32 entries each, likewise), and
/// the parameter sets it shares, which it alone may keep alive once the
/// stream has replaced them (with 1 KiB for the lists a sequence
/// parameter set holds).
pub(crate) const JOB_BYTES: usize = size_of::<SliceJob>()
    + 2 * 66 * size_of::<crate::slice::ListModification>()
    + 128 * size_of::<crate::slice::Mmco>()
    + 2 * 64 * size_of::<RefPic>()
    + size_of::<Sps>()
    + size_of::<Pps>()
    + 1024;

/// Bytes of slice data a worker's queue holds before the caller's thread
/// waits; a slice larger than this is still sent once the queue is empty.
pub(crate) const QUEUE_BYTES: usize = 8 << 20;

/// What the caller's thread and one worker share besides the channel.
struct Shared {
    /// Set when the decoder is dropped: the worker skips what is queued.
    cancel: AtomicBool,
    /// Bytes of slice data queued and not yet decoded, and whether the
    /// worker has stopped (after which nothing will free them).
    queue: Mutex<(usize, bool)>,
    freed: Condvar,
}

impl Shared {
    /// Waits until `bytes` more fit the queue, and counts them.
    fn reserve(&self, bytes: usize) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        while queue.0 > 0 && queue.0.saturating_add(bytes) > QUEUE_BYTES && !queue.1 {
            queue = self
                .freed
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
        queue.0 = queue.0.saturating_add(bytes);
    }

    /// `bytes` have left the queue.
    fn release(&self, bytes: usize) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.0 = queue.0.saturating_sub(bytes);
        self.freed.notify_all();
    }
}

/// Marks the worker stopped when its loop ends, by returning or by
/// unwinding, so that the caller's thread never waits for room in a queue
/// nobody empties.
struct Stopped<'a>(&'a Shared);

impl Drop for Stopped<'_> {
    fn drop(&mut self) {
        let mut queue = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.1 = true;
        self.0.freed.notify_all();
    }
}

/// A thread that decodes one picture at a time.
pub(crate) struct Worker<K> {
    jobs: Option<SyncSender<Message<K>>>,
    done: Receiver<Done>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl<K: Kernels> Worker<K> {
    /// Starts a worker, or `None` where a thread cannot be started (the
    /// `wasm32-unknown-unknown` standard library cannot start one).
    pub(crate) fn spawn() -> Option<Worker<K>> {
        let (jobs, inbox) = mpsc::sync_channel(QUEUE_MESSAGES);
        let (outbox, done) = mpsc::channel();
        let shared = Arc::new(Shared {
            cancel: AtomicBool::new(false),
            queue: Mutex::new((0, false)),
            freed: Condvar::new(),
        });
        let theirs = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("coreav-h264".to_owned())
            .spawn(move || run(&inbox, &outbox, &theirs))
            .ok()?;
        Some(Worker {
            jobs: Some(jobs),
            done,
            shared,
            thread: Some(thread),
        })
    }

    /// Sends `message`, waiting while the worker's queue is full, or drops
    /// it if the worker has stopped (which its picture's retirement then
    /// reports).
    ///
    /// The wait ends: the worker decodes what it is sent, and waits only
    /// for pictures before its own, every one of which has been sent all
    /// its slices.
    pub(crate) fn send(&mut self, message: Message<K>) {
        if let Message::Slice(job) = &message {
            self.shared.reserve(job.rbsp.capacity());
        }
        if let Some(jobs) = &self.jobs {
            if jobs.send(message).is_err() {
                self.jobs = None;
            }
        }
    }

    /// Whether the worker can take a picture.
    pub(crate) fn alive(&self) -> bool {
        self.jobs.is_some()
    }

    /// The report of the picture the worker was given, if it is finished;
    /// with `wait`, once it is.
    pub(crate) fn done(&mut self, wait: bool) -> Option<Done> {
        let received = if wait {
            self.done.recv().map_err(|_| TryRecvError::Disconnected)
        } else {
            self.done.try_recv()
        };
        match received {
            Ok(done) => Some(done),
            Err(TryRecvError::Empty) => None,
            // The thread stopped: its picture was published as it stood
            // (`Canvas`'s `Drop`), and the worker takes no more.
            Err(TryRecvError::Disconnected) => {
                self.jobs = None;
                Some(Done {
                    error: Some(Error::Geometry("a decoding thread stopped")),
                })
            }
        }
    }
}

/// The workers of one decoder, stopped and joined when it is dropped.
pub(crate) struct Workers<K> {
    pub(crate) list: Vec<Worker<K>>,
}

impl<K> Drop for Workers<K> {
    fn drop(&mut self) {
        // What is queued is skipped, and every channel is closed before
        // any thread is joined: a worker may be waiting for a band of a
        // picture another worker holds, which that worker publishes when
        // it drops the picture.
        for worker in &mut self.list {
            worker.shared.cancel.store(true, Ordering::Relaxed);
            worker.jobs = None;
        }
        for worker in &mut self.list {
            if let Some(thread) = worker.thread.take() {
                // A worker that panicked has nothing left to report.
                let _ = thread.join();
            }
        }
    }
}

/// A worker's loop: pictures, one at a time, until its channel closes.
fn run<K: Kernels>(inbox: &Receiver<Message<K>>, outbox: &Sender<Done>, shared: &Shared) {
    let _stopped = Stopped(shared);
    let mut work: Option<Box<PictureWork<K>>> = None;
    let mut error = None;
    let mut spare = Spare::default();
    while let Ok(message) = inbox.recv() {
        let cancelled = shared.cancel.load(Ordering::Relaxed);
        match message {
            Message::Start(mut next) => {
                next.adopt(std::mem::take(&mut spare));
                work = Some(next);
                error = None;
            }
            Message::Slice(job) => {
                if !cancelled {
                    if let Some(work) = work.as_mut() {
                        if let Err(e) = work.decode_slice(&job, &job.rbsp) {
                            error.get_or_insert(e);
                        }
                    }
                }
                shared.release(job.rbsp.capacity());
            }
            Message::Finish => {
                if cancelled {
                    // Dropping the picture publishes its bands as they
                    // stand; nobody will read them.
                    work = None;
                    continue;
                }
                if let Some(finished) = work.take() {
                    let (result, kept) = finished.finish();
                    spare = kept;
                    if let Err(e) = result {
                        error.get_or_insert(e);
                    }
                }
                if outbox
                    .send(Done {
                        error: error.take(),
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
    }
    // The channel closed with a picture unfinished: dropping it publishes
    // its bands as they stand.
}
