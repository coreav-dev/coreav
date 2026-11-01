//! The decoder: NAL units in, pictures out.
//!
//! It holds the parameter sets, the picture being decoded, the decoded
//! picture buffer and the state that picture order counts and reference
//! marking carry from one picture to the next (clauses 7.4.1.2.4, 8.2.1,
//! 8.2.5).
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::Arc;

use coreav_kernels_h264::{Auto, Kernels};

use crate::bits::BitReader;
use crate::dpb::{Dpb, Entry, RefMark};
use crate::error::{Error, LimitKind, Result};
use crate::limits::Limits;
use crate::nal::{NalHeader, NalKind, rbsp};
use crate::params::{MAX_PPS, MAX_SPS, Pps, Sps, parse_pps, parse_sps};
use crate::picture::{BandData, Canvas, Geometry, Picture, SLOTS};
use crate::slice::{SliceHeader, SliceType, parse_slice_header, peek_pps_id};
use crate::tables as t;
use crate::tier::Choice;
use crate::work::{
    Done, JOB_BYTES, Message, PictureWork, QUEUE_BYTES, QUEUE_MESSAGES, SliceJob, Spare, Worker,
    Workers,
};

/// What a decoder spends its work on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Every picture of the stream (the default).
    #[default]
    EveryPicture,
    /// Only the pictures that need no other picture: the IDR pictures and
    /// the other intra ones. A thumbnail needs one of those and nothing
    /// else, and skipping the rest is most of the work of a stream.
    ///
    /// The picture this returns for an access unit is the picture full
    /// decoding *decodes* for it, sample for sample: an intra picture
    /// references nothing, and the deblocking filter of a picture reads
    /// only that picture. The pictures come in decode order.
    ///
    /// Two things follow from decoding each keyframe as if the stream
    /// started at it (`docs/design/h264.md`, "Pictures that need no other
    /// picture"):
    ///
    /// - A later access unit may tell a full decoder to drop pictures it
    ///   has decoded but not yet output (`no_output_of_prior_pics_flag`,
    ///   clause C.4.4). This mode has returned them already.
    /// - [`Picture::poc`] counts from the keyframe, not from the start of
    ///   the stream, so it is not the picture order count full decoding
    ///   gives the same picture. [`Decoder::set_mark`] and
    ///   [`Picture::mark`] are how a caller tells which access unit a
    ///   picture came from.
    KeyframesOnly,
}

/// A problem that does not stop decoding, but that may change what the
/// pictures look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    /// frame_num skipped values in a stream that does not allow gaps. The
    /// specification leaves what to do open; the decoder keeps the
    /// reference pictures it has (clause 8.2.5.2).
    FrameNumGap,
    /// A picture could not be finished. The pictures that reference it may
    /// differ from what a complete stream would give.
    PictureIncomplete(Error),
}

/// Warnings a decoder keeps ([`Decoder::warnings`]).
pub const MAX_WARNINGS: usize = 256;

/// What tells the slices of one picture from the next picture's
/// (clause 7.4.1.2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Boundary {
    frame_num: u32,
    pps_id: u32,
    field_pic: bool,
    /// `nal_ref_idc != 0`: what the clause compares is whether the slice
    /// belongs to a reference picture, not the value.
    reference: bool,
    poc_lsb: u32,
    delta_poc_bottom: i32,
    delta_poc: [i32; 2],
    idr: bool,
    idr_pic_id: u32,
    /// first_mb_in_slice: a slice that does not start after the previous
    /// one starts a picture (`docs/design/h264.md`, "Bit-exact whatever
    /// the thread count").
    first_mb: u32,
}

/// Where the slices of the picture being decoded go.
enum Feed<K> {
    /// Decoded on the caller's thread as they arrive.
    Inline(Box<PictureWork<K>>),
    /// Sent to the worker of this index.
    Worker(usize),
}

/// A picture a worker is decoding, in decoding order.
struct Flight {
    /// The picture's place in decoding order.
    seq: u64,
    worker: usize,
}

/// What is known about the picture being decoded.
struct Current<K> {
    picture: Arc<Picture>,
    feed: Feed<K>,
    /// The picture order count the picture is decoded with. The picture
    /// itself carries the count it is output with, which a marking
    /// operation 5 resets to 0 (clause 8.2.1).
    current_poc: i32,
    hdr: SliceHeader,
    sps: Arc<Sps>,
    poc_msb: i32,
    top: i32,
    bottom: i32,
    frame_num_offset: i32,
    slices: usize,
    /// False for a picture that precedes the random access point it was
    /// decoded after, which is decoded but not output.
    output: bool,
    /// False once a slice of this picture turned out not to be intra,
    /// which only [`Mode::KeyframesOnly`] looks at: a picture that is
    /// partly predicted from another is not one this mode returns.
    keyframe: bool,
}

/// A decoder for one H.264 stream.
///
/// Feed it NAL units with [`Decoder::decode_nal`], take the pictures it has
/// finished with [`Decoder::take_pictures`], and call [`Decoder::flush`] at
/// the end of the stream.
pub struct Decoder<K: Kernels = Auto> {
    kernels: Arc<K>,
    /// The instance of the hot paths its slices run, chosen once
    /// ([`Decoder::with_x86_64_v3`]).
    choice: Choice,
    limits: Limits,
    mode: Mode,
    /// Pictures decoded at once, one thread each; 1 decodes on the
    /// caller's thread and starts none.
    threads: usize,
    /// Macroblock rows per band, where a test sets it.
    band_mbs: Option<usize>,
    workers: Workers<K>,
    /// The pictures sent to workers and not yet retired, in decoding
    /// order.
    flights: VecDeque<Flight>,
    /// The place in decoding order of the newest picture started.
    seq: u64,
    /// Pictures the output process has released, each with the newest
    /// picture started when it was released: it is returned once every
    /// picture up to that one is retired and its errors reported.
    ready: VecDeque<(Arc<Picture>, u64)>,
    /// Errors of retired pictures not yet reported, in decoding order.
    errors: VecDeque<(u64, Error)>,
    /// Sequence parameter sets, shared with the slices that use them.
    sps: Vec<Option<Arc<Sps>>>,
    /// Picture parameter sets are kept as their RBSP and resolved when a
    /// slice activates them: their sequence parameter set may arrive later
    /// or be replaced.
    pps: Vec<Option<Vec<u8>>>,
    /// Each picture parameter set as last resolved, with the sequence
    /// parameter set it was resolved against: valid while that one is
    /// still the active one of its id, so that the slices of a stream share
    /// one parse rather than each carrying a copy to its thread.
    resolved: Vec<Option<(Arc<Sps>, Arc<Pps>)>>,
    dpb: Dpb,
    current: Option<Current<K>>,
    /// Where the search for the next picture's slot starts
    /// ([`Decoder::picture_slot`]).
    next_slot: usize,
    /// Pictures this decoder made, reused once nothing else holds them.
    pool: Vec<Arc<Picture>>,
    rbsp: Vec<u8>,
    /// The buffers of the decoding on the caller's thread.
    spare: Spare,
    // The picture order count and frame_num state of the previous picture.
    prev_ref_poc_msb: i32,
    prev_ref_poc_lsb: i32,
    prev_ref_frame_num: u32,
    prev_frame_num: u32,
    prev_frame_num_offset: i32,
    prev_has_mmco5: bool,
    active_size: Option<(usize, usize)>,
    /// The picture size and buffer size the memory budget was last
    /// checked for ([`Decoder::fit_memory`]).
    memory_checked: Option<(usize, usize, usize)>,
    /// A picture has been decoded since the start of the stream.
    started: bool,
    /// Set while decoding after a random access point that is not an IDR
    /// picture, until the next IDR picture or a marking operation 5.
    after_random_access: bool,
    /// The picture order count of that random access point.
    random_access_poc: Option<i32>,
    /// The most recent reference picture, for concealing frame_num gaps.
    last_reference: Option<Arc<Picture>>,
    /// The header of the last slice seen, decoded or skipped, which is
    /// what a picture boundary is detected against (clause 7.4.1.2.4).
    last_slice: Option<Boundary>,
    /// Set while the slices of a picture are being skipped whole
    /// ([`Mode::KeyframesOnly`]).
    skipping: bool,
    /// Slices of the picture being skipped, against the limit.
    skipped_slices: usize,
    /// What [`Decoder::set_mark`] attaches to the pictures that follow.
    mark: u64,
    warnings: Vec<Warning>,
    warnings_dropped: u64,
    /// How many pictures came from the pool rather than a new allocation
    /// ([`Decoder::pooled_pictures`]).
    pooled: u64,
}

impl Decoder<Auto> {
    /// A decoder with the fastest kernels this CPU supports.
    pub fn new(limits: Limits) -> Decoder<Auto> {
        Decoder::with_kernels(Auto::detect(), limits)
    }
}

impl<K: Kernels> Decoder<K> {
    /// A decoder that uses the given kernels.
    pub fn with_kernels(kernels: K, limits: Limits) -> Decoder<K> {
        Decoder {
            kernels: Arc::new(kernels),
            choice: Choice::detect(true),
            limits,
            mode: Mode::EveryPicture,
            threads: 1,
            band_mbs: None,
            workers: Workers { list: Vec::new() },
            flights: VecDeque::new(),
            seq: 0,
            ready: VecDeque::new(),
            errors: VecDeque::new(),
            sps: vec![None; MAX_SPS],
            pps: vec![None; MAX_PPS],
            resolved: vec![None; MAX_PPS],
            dpb: Dpb::new(),
            current: None,
            next_slot: 0,
            pool: Vec::new(),
            rbsp: Vec::new(),
            spare: Spare::default(),
            prev_ref_poc_msb: 0,
            prev_ref_poc_lsb: 0,
            prev_ref_frame_num: 0,
            prev_frame_num: 0,
            prev_frame_num_offset: 0,
            prev_has_mmco5: false,
            active_size: None,
            memory_checked: None,
            started: false,
            after_random_access: false,
            random_access_poc: None,
            last_reference: None,
            last_slice: None,
            skipping: false,
            skipped_slices: 0,
            mark: 0,
            warnings: Vec::new(),
            warnings_dropped: 0,
            pooled: 0,
        }
    }

    /// Decodes only the pictures that need no other picture.
    ///
    /// A thumbnail needs one keyframe and nothing else; skipping the rest
    /// is most of the work of a stream.
    pub fn with_mode(mut self, mode: Mode) -> Decoder<K> {
        self.mode = mode;
        self
    }

    /// Decodes up to `threads` pictures at once, each on a thread of its
    /// own, at most [`Limits::max_threads`].
    ///
    /// The default is one, which decodes on the caller's thread and starts
    /// no thread at all. With more, the caller's thread reads the headers
    /// and keeps the decoded picture buffer while the threads decode the
    /// samples; the pictures are the same, sample for sample, whatever the
    /// count (`docs/design/h264.md`, "Threading (step 7): our design").
    /// Where a thread cannot be started — the `wasm32-unknown-unknown`
    /// standard library cannot start one — the caller's thread decodes the
    /// picture instead.
    ///
    /// Two things change with threads:
    ///
    /// - [`Decoder::take_pictures`] returns a picture once it is decoded,
    ///   so a picture comes out up to `threads` pictures later than it
    ///   would on one thread; [`Decoder::flush`] waits for all of them.
    /// - An error in a slice's data is reported by the first call to
    ///   [`Decoder::decode_nal`] or [`Decoder::end_access_unit`] after its
    ///   picture is decoded, rather than by the call that sent the slice,
    ///   and only the first error of a picture is. A picture released for
    ///   output after one that failed is not returned before that error
    ///   is reported; a caller that carries on after errors receives the
    ///   pictures one thread gives, in the same order.
    pub fn with_threads(mut self, threads: usize) -> Decoder<K> {
        self.threads = threads.clamp(1, self.limits.max_threads.max(1));
        self
    }

    /// Whether the decoder may run its x86-64-v3 instance of the hot paths
    /// (the default) or only the baseline one.
    ///
    /// A few functions of the decoder are compiled twice, for the x86-64
    /// baseline and for x86-64-v3 (AVX2, BMI2, LZCNT and the rest of that
    /// level), and the decoder runs the v3 instance where the CPU has
    /// every feature it was compiled for. The pictures are the same,
    /// sample for sample, and so are the errors. `false` keeps the baseline
    /// instance, for comparing the two; on other architectures, and in a
    /// build without the feature `fast`, there is only the baseline
    /// instance and this does nothing (`docs/design/h264.md`, "Kernel
    /// round: the hot paths compiled twice").
    pub fn with_x86_64_v3(mut self, allow: bool) -> Decoder<K> {
        self.choice = Choice::detect(allow);
        self
    }

    /// Cuts pictures into bands of `mb_rows` macroblock rows, as threaded
    /// decoding does, whatever the thread count. For tests: the samples do
    /// not depend on it.
    #[doc(hidden)]
    pub fn with_band_height(mut self, mb_rows: usize) -> Decoder<K> {
        self.band_mbs = Some(mb_rows.max(1));
        self
    }

    /// The most pictures, with their samples and motion, a decoder with
    /// `threads` threads holds at once under `limits`: the references, as
    /// many again that pictures in flight still predict from after the
    /// buffer let them go, the pictures in flight and those waiting to be
    /// taken, and pictures kept for reuse — a picture of another size or
    /// band layout than the next stays until the pool is pruned. A memory
    /// budget counts this many.
    pub fn most_pictures(limits: &Limits, threads: usize) -> usize {
        most_pictures(limits.max_dpb_frames, threads)
    }

    /// What a decoder needs of [`Limits::max_memory_bytes`] for pictures
    /// of `mb_width` by `mb_height` macroblocks decoded with `threads`
    /// threads: [`Decoder::most_pictures`] pictures stored in the bands
    /// those threads cut them into, with their motion and the slack rows
    /// the bands at each picture's edges keep; a copy of the
    /// samples of every picture one flush releases (the references and
    /// the pictures in flight); each thread's state; and the decoder's
    /// own. A bound, checked by the native backend's `tests/memory.rs`.
    pub fn memory_needed(
        mb_width: usize,
        mb_height: usize,
        threads: usize,
        limits: &Limits,
    ) -> u64 {
        let band_mbs = Geometry::band_mbs_for(mb_height, threads);
        need(
            mb_width,
            mb_height,
            threads,
            band_mbs,
            limits.max_dpb_frames,
            limits,
        )
    }

    /// The number of pictures decoded at once ([`Decoder::with_threads`]).
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Lowers the number of pictures decoded at once to `threads`, at
    /// least one, for the pictures started from here on; it never raises
    /// it. The decoder lowers it itself where the pictures of a size would
    /// not fit [`Limits::max_memory_bytes`]; a caller may lower it further.
    /// The pictures are the same.
    pub fn limit_threads(&mut self, threads: usize) {
        self.threads = self.threads.min(threads.max(1));
    }

    /// Whether the parameter sets read so far describe a stream this
    /// decoder decodes (`docs/design/h264.md`, "Scope").
    ///
    /// The scope of a stream is in its parameter sets, and a caller that
    /// has them before the first slice — out of the `avcC` of an MP4,
    /// say — can find out before it decides which decoder to use.
    /// Decoding finds out anyway, at the first slice that activates a
    /// parameter set, which is too late for a caller that would
    /// otherwise have chosen another decoder.
    ///
    /// What the slices themselves decide is not here: a stream may still
    /// turn out to use data partitioning or SP and SI slices, which this
    /// decoder refuses when it reads them.
    pub fn parameter_sets_in_scope(&self) -> Result<()> {
        for sps in self.sps.iter().flatten() {
            check_sps_scope(sps)?;
        }
        let sps_list = &self.sps;
        for raw in self.pps.iter().flatten() {
            // A picture parameter set whose sequence parameter set is
            // not here yet says nothing about the scope: the stream may
            // carry that sequence parameter set in band. Only one that
            // can be read is judged.
            let Ok(pps) = parse_pps(raw, |id| {
                sps_list
                    .get(id as usize)
                    .and_then(|s| s.as_deref().cloned())
            }) else {
                continue;
            };
            check_pps_scope(&pps)?;
        }
        Ok(())
    }

    /// Whether the parameter sets read so far describe a stream this
    /// decoder's [`Limits`] allow.
    ///
    /// The answer is in the parameter sets, as the scope is: a caller
    /// that has them can be told that a stream needs more reference
    /// pictures, or a larger picture, than it allows — and choose
    /// another decoder — instead of being told at the first slice.
    pub fn parameter_sets_within_limits(&self) -> Result<()> {
        for sps in self.sps.iter().flatten() {
            if sps.max_num_ref_frames as usize > self.limits.max_dpb_frames {
                return Err(Error::Limit(LimitKind::DpbFrames));
            }
            let mbs = (sps.width_mbs as usize).checked_mul(sps.frame_height_mbs() as usize);
            if !mbs.is_some_and(|m| m > 0 && m <= self.limits.max_mbs) {
                return Err(Error::Limit(LimitKind::PictureSize));
            }
        }
        Ok(())
    }

    /// Attaches `mark` to the pictures decoded from the NAL units that
    /// follow, until the next call.
    ///
    /// The decoder returns pictures in output order, which is not the
    /// order they were decoded in. A caller that has to tell which packet
    /// a picture came from — to give a frame its timestamp, say — marks
    /// each packet and reads the mark back with [`Picture::mark`].
    pub fn set_mark(&mut self, mark: u64) {
        self.mark = mark;
    }

    /// The pictures that are ready, in output order.
    ///
    /// With threads ([`Decoder::with_threads`]) a picture is ready once it
    /// is decoded, and this returns the pictures up to the first that is
    /// not, without waiting for it.
    pub fn take_pictures(&mut self) -> Vec<Arc<Picture>> {
        self.retire(Wait::No);
        self.release_output();
        let cleared = self.cleared();
        let mut out = Vec::new();
        while let Some((_, gate)) = self.ready.front() {
            if *gate > cleared {
                break;
            }
            if let Some((picture, _)) = self.ready.pop_front() {
                out.push(picture);
            }
        }
        out
    }

    /// The newest picture in decoding order such that it and every picture
    /// before it are decoded and their errors reported.
    fn cleared(&self) -> u64 {
        let decoded = self
            .flights
            .front()
            .map_or(self.seq, |f| f.seq.saturating_sub(1));
        let reported = self
            .errors
            .front()
            .map_or(u64::MAX, |(seq, _)| seq.saturating_sub(1));
        decoded.min(reported)
    }

    /// Moves what the output process has released to the pictures waiting
    /// to be returned, each behind every picture started so far.
    fn release_output(&mut self) {
        let gate = self.seq;
        self.ready
            .extend(self.dpb.output.drain(..).map(|picture| (picture, gate)));
    }

    /// Retires the pictures the workers have finished, in decoding order:
    /// those finished so far, the oldest one, or all of them.
    fn retire(&mut self, wait: Wait) {
        while let Some(flight) = self.flights.front() {
            let block = wait != Wait::No;
            let done = match self.workers.list.get_mut(flight.worker) {
                Some(worker) => worker.done(block),
                // Not a worker of this decoder: nothing will report the
                // picture, which is as good as lost.
                None => Some(Done {
                    error: Some(Error::Geometry("a picture without a thread")),
                }),
            };
            let Some(done) = done else {
                return;
            };
            if let Some(flight) = self.flights.pop_front() {
                if let Some(e) = done.error {
                    self.errors.push_back((flight.seq, e));
                }
            }
            if wait == Wait::Oldest {
                return;
            }
        }
    }

    /// The result of a call, after the pictures finished since are
    /// retired: an error of a retired picture is reported here if the
    /// call itself succeeded.
    fn report(&mut self, result: Result<()>) -> Result<()> {
        self.retire(Wait::No);
        result?;
        match self.errors.pop_front() {
            Some((_, e)) => Err(e),
            None => Ok(()),
        }
    }

    /// What went wrong without stopping the decoding: the first
    /// [`MAX_WARNINGS`]; [`Decoder::warnings_dropped`] counts the rest.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// How many warnings came after the first [`MAX_WARNINGS`] and were
    /// not kept: a stream can give one a picture, and the decoder's memory
    /// must not grow with the stream.
    pub fn warnings_dropped(&self) -> u64 {
        self.warnings_dropped
    }

    /// Keeps `warning`, or counts it once [`MAX_WARNINGS`] are kept.
    fn warn(&mut self, warning: Warning) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(warning);
        } else {
            self.warnings_dropped = self.warnings_dropped.saturating_add(1);
        }
    }

    /// How many pictures this decoder has taken from its pool rather than
    /// allocating.
    ///
    /// A diagnostic, not part of the decoding contract: a caller cannot
    /// tell the difference in the pictures it receives. Tests of the
    /// samples a reused buffer must not show use it to check that they
    /// are testing reuse at all — without it, a change that stopped the
    /// pool recycling would turn those tests into silent passes.
    #[doc(hidden)]
    pub fn pooled_pictures(&self) -> u64 {
        self.pooled
    }

    /// Ends the access unit: finishes the picture its slices carried,
    /// which is then output when the output order allows it (Annex
    /// C.4.5).
    ///
    /// A caller that knows where the access units are — one packet of a
    /// container holds one — calls this after the last NAL unit of each,
    /// and the picture's samples are ready without waiting for the
    /// access unit after it. A caller that does not know never has to:
    /// the first slice of the next picture ends this one just the same
    /// (clause 7.4.1.2.4).
    ///
    /// The slice that follows starts a picture, whatever its header
    /// says: the caller has stated that the access unit ended here, and
    /// that is a stronger statement than the comparison between
    /// consecutive slices. A caller that calls this in the middle of a
    /// picture gets that picture as it stands, with the macroblocks of
    /// the slices that had not arrived left as they were, and the slices
    /// after it as a picture of their own; the decoder cannot tell that
    /// from a stream whose last slices are missing.
    pub fn end_access_unit(&mut self) -> Result<()> {
        let result = self.finish_picture();
        self.end_of_access_unit();
        self.report(result)
    }

    /// Ends the stream: finishes the picture being decoded and outputs
    /// every picture still held.
    ///
    /// With one thread this always succeeds: an error finishing the last
    /// picture becomes a [`Warning::PictureIncomplete`], as it always has.
    /// With threads ([`Decoder::with_threads`]) it waits for every picture
    /// to be decoded and returns the first error of a slice not reported
    /// yet, so that a caller that stops at the first error stops here and
    /// takes no picture released after the one that failed; any later
    /// ones become warnings. A caller that carries on takes the pictures
    /// one thread gives.
    pub fn flush(&mut self) -> Result<()> {
        if let Err(e) = self.finish_picture() {
            self.warn(Warning::PictureIncomplete(e));
        }
        self.end_of_access_unit();
        self.dpb.flush();
        self.release_output();
        self.retire(Wait::All);
        let first = self.errors.pop_front();
        while let Some((_, e)) = self.errors.pop_front() {
            self.warn(Warning::PictureIncomplete(e));
        }
        match first {
            Some((_, e)) => Err(e),
            None => Ok(()),
        }
    }

    /// Forgets the slice a picture boundary is compared against, so that
    /// the next slice starts a picture. Called where the stream itself
    /// says an access unit ended.
    fn end_of_access_unit(&mut self) {
        self.last_slice = None;
        self.skipping = false;
        self.skipped_slices = 0;
    }

    /// Decodes one NAL unit, with its header byte and without emulation
    /// prevention bytes removed (that happens here).
    pub fn decode_nal(&mut self, nal: &[u8]) -> Result<()> {
        let result = self.decode_unit(nal);
        self.report(result)
    }

    /// [`Decoder::decode_nal`], before the pictures finished meanwhile
    /// are retired.
    fn decode_unit(&mut self, nal: &[u8]) -> Result<()> {
        let header = NalHeader::parse(nal)?;
        let payload = nal.get(1..).unwrap_or_default();
        match header.kind {
            NalKind::Slice | NalKind::IdrSlice => {
                let mut buffer = std::mem::take(&mut self.rbsp);
                rbsp(payload, &mut buffer);
                let idr = header.kind == NalKind::IdrSlice;
                let result = self.decode_slice(&mut buffer, idr, header.nal_ref_idc);
                self.rbsp = buffer;
                result
            }
            NalKind::Partition => Err(Error::Unsupported("data partitioning")),
            NalKind::Sps => {
                // Only the first bytes are read, as for a picture parameter
                // set; the set is kept parsed, not as its bytes.
                let mut buffer = Vec::new();
                let prefix = payload.get(..MAX_SPS_BYTES * 3 / 2).unwrap_or(payload);
                rbsp(prefix, &mut buffer);
                buffer.truncate(MAX_SPS_BYTES);
                let sps = parse_sps(&buffer)?;
                if let Some(slot) = self.sps.get_mut(sps.id as usize) {
                    *slot = Some(Arc::new(sps));
                }
                Ok(())
            }
            NalKind::Pps => {
                // Only the first bytes are kept: escaping adds at most one
                // byte in three, so this prefix yields them.
                let mut buffer = Vec::new();
                let prefix = payload.get(..MAX_PPS_BYTES * 3 / 2).unwrap_or(payload);
                rbsp(prefix, &mut buffer);
                buffer.truncate(MAX_PPS_BYTES);
                buffer.shrink_to_fit();
                let id = BitReader::new(&buffer).ue()? as usize;
                let slot = self
                    .pps
                    .get_mut(id)
                    .ok_or(Error::Invalid("pic_parameter_set_id"))?;
                *slot = Some(buffer);
                if let Some(resolved) = self.resolved.get_mut(id) {
                    *resolved = None;
                }
                Ok(())
            }
            // These end the picture being decoded, and the access unit
            // with it (clause 7.4.1.2.3): the slice that follows starts a
            // picture, whatever its header holds.
            NalKind::Sei
            | NalKind::AccessUnitDelimiter
            | NalKind::EndOfSequence
            | NalKind::EndOfStream => {
                let result = self.finish_picture();
                self.end_of_access_unit();
                result
            }
            _ => Ok(()),
        }
    }

    /// One coded slice.
    ///
    /// A slice that goes to a worker takes `rbsp` with it.
    fn decode_slice(&mut self, rbsp: &mut Vec<u8>, idr: bool, nal_ref_idc: u8) -> Result<()> {
        let pps_id = peek_pps_id(rbsp)? as usize;
        let raw = self
            .pps
            .get(pps_id)
            .and_then(|p| p.as_ref())
            .ok_or(Error::Invalid(
                "a slice names a missing picture parameter set",
            ))?;
        let cached = self
            .resolved
            .get(pps_id)
            .and_then(Option::as_ref)
            .filter(|(sps, pps)| {
                self.sps
                    .get(pps.sps_id as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|active| Arc::ptr_eq(active, sps))
            });
        let (sps, pps) = match cached {
            Some((sps, pps)) => (Arc::clone(sps), Arc::clone(pps)),
            None => {
                let sps_list = &self.sps;
                let pps = parse_pps(raw, |id| {
                    sps_list
                        .get(id as usize)
                        .and_then(|s| s.as_deref().cloned())
                })?;
                let sps = self
                    .sps
                    .get(pps.sps_id as usize)
                    .and_then(Option::clone)
                    .ok_or(Error::Invalid(
                        "a picture parameter set names a missing sequence parameter set",
                    ))?;
                let pps = Arc::new(pps);
                if let Some(slot) = self.resolved.get_mut(pps_id) {
                    *slot = Some((Arc::clone(&sps), Arc::clone(&pps)));
                }
                (sps, pps)
            }
        };
        check_scope(&sps, &pps)?;
        let mut r = BitReader::new(rbsp);
        let nal_unit_type = if idr { 5 } else { 1 };
        let hdr = parse_slice_header(&mut r, nal_unit_type, nal_ref_idc, &sps, &pps)?;
        if hdr.field_pic {
            return Err(Error::Unsupported("field pictures"));
        }
        if matches!(hdr.slice_type, SliceType::Sp | SliceType::Si) {
            return Err(Error::Unsupported("SP and SI slices"));
        }
        if hdr.redundant_pic_cnt > 0 {
            // A redundant coded picture is not needed while the primary one
            // is intact (clause 7.4.1.2.5).
            return Ok(());
        }
        // A picture boundary is a change between one slice and the next
        // (clause 7.4.1.2.4), so it is detected against the last slice
        // seen, whether or not this decoder decoded it.
        // So does a slice that does not start after the previous one: the
        // slices of a picture come in increasing macroblock order, and the
        // test reads only headers, so that the caller's thread can make it
        // while workers decode the samples (`docs/design/h264.md`,
        // "Bit-exact whatever the thread count").
        // A slice whose header matches the previous one's but which starts
        // before it is arbitrary slice order (clause 7.4.3, Baseline
        // only), which this decoder does not decode: taking it for a new
        // picture would split a picture in two without saying so. The
        // CAVLC slice loop relies on this too: it derives a macroblock's
        // neighbours from the slice's first address
        // (`SliceDecoder::derive_neighbours_in_order`).
        let new_picture = match self.last_slice {
            None => true,
            Some(last) if last.starts_new_picture(&hdr, &sps) => true,
            Some(last) if hdr.first_mb < last.first_mb => {
                return Err(Error::Unsupported("arbitrary slice order"));
            }
            Some(last) => hdr.first_mb == last.first_mb,
        };
        self.last_slice = Some(Boundary::of(&hdr));
        if self.mode == Mode::KeyframesOnly {
            if new_picture {
                self.finish_picture()?;
                // Only a picture whose first slice is intra can be a
                // keyframe; the rest are skipped whole, which is most of
                // the work of a stream.
                self.skipping = !hdr.slice_type.is_intra();
                self.skipped_slices = 0;
                if !self.skipping {
                    // A keyframe needs no picture before it, and this mode
                    // returns nothing else, so it is decoded as if the
                    // stream started here: the skipped pictures leave no
                    // state behind to carry.
                    self.restart();
                }
            }
            if self.skipping {
                // A skipped slice still counts against the limit on the
                // slices of one picture.
                self.skipped_slices += 1;
                if self.skipped_slices > self.limits.max_slices_per_picture {
                    return Err(Error::Limit(LimitKind::SlicesPerPicture));
                }
                return Ok(());
            }
            if !hdr.slice_type.is_intra() {
                // A picture may mix intra and predicted slices. One that
                // does is not returned, because the samples of its
                // predicted slices are not decoded here.
                if let Some(current) = self.current.as_mut() {
                    current.keyframe = false;
                }
                return Ok(());
            }
        }
        if new_picture {
            if self.mode != Mode::KeyframesOnly {
                self.finish_picture()?;
            }
            self.start_picture(&hdr, &sps, &pps)?;
        }
        let Some(current) = self.current.as_mut() else {
            return Err(Error::Invalid("a slice without a picture"));
        };
        current.slices += 1;
        if current.slices > self.limits.max_slices_per_picture {
            return Err(Error::Limit(LimitKind::SlicesPerPicture));
        }
        let slice_id = current.slices as u32;
        let refs =
            self.dpb
                .ref_lists(&hdr, current.current_poc, sps.max_frame_num(), sps.poc_type)?;
        if !hdr.slice_type.is_intra() && refs.first().is_none_or(|l| l.is_empty()) {
            return Err(Error::Invalid("an empty reference picture list 0"));
        }
        if hdr.slice_type.is_b() && refs.get(1).is_none_or(|l| l.is_empty()) {
            return Err(Error::Invalid("an empty reference picture list 1"));
        }
        let mut job = SliceJob {
            rbsp: Vec::new(),
            hdr,
            sps,
            pps,
            refs,
            slice_id,
            current_poc: current.current_poc,
        };
        match &mut current.feed {
            Feed::Inline(work) => work.decode_slice(&job, rbsp),
            Feed::Worker(index) => {
                job.rbsp = std::mem::take(rbsp);
                if let Some(worker) = self.workers.list.get_mut(*index) {
                    // A worker that stopped reports it when its picture
                    // is retired.
                    worker.send(Message::Slice(Box::new(job)));
                }
                Ok(())
            }
        }
    }

    /// Puts the decoder back in the state of one that has just read the
    /// parameter sets, so that the picture about to be decoded is decoded
    /// as if the stream started at it ([`Mode::KeyframesOnly`]).
    ///
    /// A keyframe needs no picture before it, and this mode returns
    /// nothing else, so nothing the skipped pictures would have left
    /// behind is wanted: carrying it over is what made a keyframe come
    /// out with another picture's picture order count, or look like a
    /// gap in `frame_num`.
    ///
    /// The parameter sets stay, and so do the two things derived from
    /// them that only describe a buffer this empties: the active picture
    /// size, whose change flushes a buffer that is already empty, and
    /// the longest long-term frame index, which gates entries there are
    /// none of.
    fn restart(&mut self) {
        self.dpb.clear();
        self.last_reference = None;
        self.prev_ref_poc_msb = 0;
        self.prev_ref_poc_lsb = 0;
        self.prev_ref_frame_num = 0;
        self.prev_frame_num = 0;
        self.prev_frame_num_offset = 0;
        self.prev_has_mmco5 = false;
        self.started = false;
        self.after_random_access = false;
        self.random_access_poc = None;
    }

    /// Starts a new picture (clause 8.2).
    fn start_picture(&mut self, hdr: &SliceHeader, sps: &Arc<Sps>, pps: &Pps) -> Result<()> {
        let mb_w = sps.width_mbs as usize;
        let mb_h = sps.frame_height_mbs() as usize;
        let mbs = mb_w
            .checked_mul(mb_h)
            .filter(|&m| m > 0 && m <= self.limits.max_mbs)
            .ok_or(Error::Limit(LimitKind::PictureSize))?;
        let dpb_size = self.dpb_size(sps, mbs);
        if self.memory_checked != Some((mb_w, mb_h, dpb_size)) {
            self.fit_memory(mb_w, mb_h, dpb_size)?;
            self.memory_checked = Some((mb_w, mb_h, dpb_size));
        }
        if self.active_size != Some((mb_w, mb_h)) {
            if self.active_size.is_some() {
                self.dpb.flush();
                self.dpb.clear();
                // Pictures of another size are of no use to this one: the
                // ones nobody holds go now rather than when the pool is
                // next pruned.
                self.pool.retain(|p| {
                    let g = p.geometry();
                    Arc::strong_count(p) > 1 || (g.mb_width, g.mb_height) == (mb_w, mb_h)
                });
            }
            self.active_size = Some((mb_w, mb_h));
        }
        // The caller's limit bounds what the decoder retains: the buffer
        // holds the reference frames the stream asks for plus the pictures
        // waiting for output.
        if sps.max_num_ref_frames as usize > self.limits.max_dpb_frames {
            return Err(Error::Limit(LimitKind::DpbFrames));
        }
        self.dpb.size = dpb_size;
        self.dpb.max_num_reorder_frames = sps
            .vui
            .max_num_reorder_frames
            .map_or(self.dpb.size, |n| (n as usize).min(self.dpb.size));
        let max_frame_num = sps.max_frame_num();
        if hdr.idr {
            self.prev_ref_frame_num = 0;
            self.after_random_access = false;
        } else if !self.started {
            // The stream starts at a picture that is not an IDR picture.
            // Its frame_num is not a gap, and the pictures that follow it
            // in decoding order but precede it in output order may
            // reference pictures from before the start: they are decoded
            // but not output (clause D.2.8, recovery point).
            self.prev_ref_frame_num = hdr.frame_num;
            self.after_random_access = true;
        } else if hdr.frame_num != self.prev_ref_frame_num
            && hdr.frame_num != (self.prev_ref_frame_num + 1) % max_frame_num
        {
            if sps.gaps_in_frame_num_allowed {
                self.fill_frame_num_gap(hdr, sps, mb_w, mb_h)?;
            } else {
                // Clause 8.2.5.2 applies only where the stream allows gaps;
                // otherwise the decoder "should infer an unintentional loss
                // of pictures" and concealment is not specified. Inserting
                // the inferred frames here would evict the real reference
                // pictures, so the gap is only recorded.
                self.warn(Warning::FrameNumGap);
            }
        }
        let (poc_msb, top, bottom, offset) =
            self.picture_order_count(hdr, sps, hdr.frame_num, hdr.nal_ref_idc, hdr.idr);
        let output = match (self.started, self.after_random_access) {
            // The random access picture itself.
            (false, true) => {
                self.random_access_poc = Some(top.min(bottom));
                true
            }
            (_, true) => self.random_access_poc.is_none_or(|r| top.min(bottom) >= r),
            _ => true,
        };
        self.started = true;
        // No other picture reads a picture that is not a reference, so
        // nothing waits for its bands: it is cut into one, which has no
        // aprons to copy.
        let band_mbs = match (hdr.nal_ref_idc, self.band_mbs) {
            (0, None) => mb_h,
            (_, None) if self.threads == 8 && mb_h < 32 => mb_h,
            _ => self.band_mbs(mb_h),
        };
        let geometry = Geometry::new(mb_w, mb_h, band_mbs);
        let (picture, bands) = self.new_picture(geometry)?;
        let slot = self.picture_slot()?;
        let crop = [
            sps.crop[0] as usize,
            sps.crop[1] as usize,
            sps.crop[2] as usize,
            sps.crop[3] as usize,
        ];
        // The picture is shared from here on, so it carries the count it
        // is output with: 0 for a picture whose marking resets the count
        // (clause 8.2.1), which every slice of the picture carries.
        let current_poc = top.min(bottom);
        let poc = if hdr.has_mmco5() { 0 } else { current_poc };
        let picture = Arc::new(picture.with_output(poc, crop, slot, self.mark));
        self.pool.push(Arc::clone(&picture));
        let canvas = Canvas::new(Arc::clone(&picture), bands);
        let work = PictureWork::new(
            Arc::clone(&self.kernels),
            self.choice,
            canvas,
            pps.chroma_qp_index_offset,
        );
        // What the output process released before this picture is behind
        // the pictures started before it.
        self.release_output();
        let feed = self.dispatch(work);
        self.current = Some(Current {
            picture,
            feed,
            current_poc,
            hdr: hdr.clone(),
            sps: Arc::clone(sps),
            poc_msb,
            top,
            bottom,
            frame_num_offset: offset,
            slices: 0,
            output,
            keyframe: true,
        });
        Ok(())
    }

    /// Fits the thread count to the memory budget for pictures of `mb_w`
    /// by `mb_h` macroblocks and a decoded picture buffer of `dpb_size`
    /// frames, before the first of those pictures is made: lowered as far
    /// as it takes, down to one, as [`Decoder::limit_threads`] does, or the
    /// size refused where one thread does not fit.
    ///
    /// At a change of size the pictures of the old size nobody holds are
    /// dropped. Those still held — the buffer's, being output, until the
    /// caller takes them — are held beside the first pictures of the new
    /// size for as long as that takes; they were counted when their size
    /// was checked, and the check does not add the two.
    fn fit_memory(&mut self, mb_w: usize, mb_h: usize, dpb_size: usize) -> Result<()> {
        let fits = |threads: usize| {
            let band_mbs = self
                .band_mbs
                .unwrap_or_else(|| Geometry::band_mbs_for(mb_h, threads));
            need(mb_w, mb_h, threads, band_mbs, dpb_size, &self.limits)
                <= self.limits.max_memory_bytes
        };
        let mut threads = self.threads.max(1);
        while threads > 1 && !fits(threads) {
            threads -= 1;
        }
        if !fits(threads) {
            return Err(Error::Limit(LimitKind::Memory));
        }
        self.threads = threads;
        Ok(())
    }

    /// Macroblock rows per band of a picture `mb_h` rows high.
    fn band_mbs(&self, mb_h: usize) -> usize {
        self.band_mbs
            .unwrap_or_else(|| Geometry::band_mbs_for(mb_h, self.threads))
    }

    /// Hands a new picture to a worker, or keeps it on the caller's
    /// thread: with one thread, or where no thread can be started.
    ///
    /// With as many pictures in flight as there are threads, this waits
    /// for the oldest to be decoded, which is what bounds the memory of
    /// threaded decoding. The picture before this one has been finished
    /// already, so every picture in flight can be decoded to the end.
    fn dispatch(&mut self, mut work: PictureWork<K>) -> Feed<K> {
        self.seq += 1;
        if self.threads <= 1 {
            work.adopt(std::mem::take(&mut self.spare));
            return Feed::Inline(Box::new(work));
        }
        while self.flights.len() >= self.threads {
            self.retire(Wait::Oldest);
        }
        let flights = &self.flights;
        let idle = self
            .workers
            .list
            .iter()
            .enumerate()
            .position(|(i, w)| w.alive() && flights.iter().all(|f| f.worker != i));
        let index = match idle {
            Some(i) => Some(i),
            None if self.workers.list.len() < self.threads => Worker::spawn().map(|worker| {
                self.workers.list.push(worker);
                self.workers.list.len() - 1
            }),
            None => None,
        };
        let Some((i, worker)) = index.and_then(|i| self.workers.list.get_mut(i).map(|w| (i, w)))
        else {
            work.adopt(std::mem::take(&mut self.spare));
            return Feed::Inline(Box::new(work));
        };
        // A worker that has stopped drops the picture, which publishes
        // its bands as they stand, and reports the failure when the
        // picture is retired.
        worker.send(Message::Start(Box::new(work)));
        self.flights.push_back(Flight {
            seq: self.seq,
            worker: i,
        });
        Feed::Worker(i)
    }

    /// How many pictures the decoded picture buffer holds (clause A.3.1 and
    /// Annex E.2.1), bounded by the caller's limits.
    fn dpb_size(&self, sps: &Sps, mbs: usize) -> usize {
        // Level 1b is listed as level_idc 9 in Table A-1.
        let level = if sps.level_idc == 11 && sps.constraint_set(3) && sps.profile_idc != 100 {
            9
        } else {
            sps.level_idc
        };
        let max_dpb_mbs = t::MAX_DPB_MBS
            .iter()
            .find(|(l, _)| *l == level)
            .map_or(696_320, |(_, m)| *m as usize);
        let max_frames = (max_dpb_mbs / mbs).min(16);
        let mut size = max_frames;
        if let Some(buffering) = sps.vui.max_dec_frame_buffering {
            // max_dec_frame_buffering shall not exceed MaxDpbFrames.
            size = (buffering as usize).min(max_frames.max(1));
        }
        size.max(sps.max_num_ref_frames as usize)
            .max(1)
            .min(self.limits.max_dpb_frames.max(1))
    }

    /// Picture order count (clause 8.2.1): (PicOrderCntMsb,
    /// TopFieldOrderCnt, BottomFieldOrderCnt, FrameNumOffset).
    fn picture_order_count(
        &self,
        hdr: &SliceHeader,
        sps: &Sps,
        frame_num: u32,
        nal_ref_idc: u8,
        idr: bool,
    ) -> (i32, i32, i32, i32) {
        // The arithmetic runs in 64 bits and is clipped to a picture order
        // count's range: a stream that leaves that range is not
        // conforming, and a count that saturates still orders its pictures
        // the same way as the one before it.
        let max_frame_num = i64::from(sps.max_frame_num());
        match sps.poc_type {
            0 => {
                let (prev_msb, prev_lsb) = if idr {
                    (0, 0)
                } else {
                    (
                        i64::from(self.prev_ref_poc_msb),
                        i64::from(self.prev_ref_poc_lsb),
                    )
                };
                let max_lsb = 1i64 << sps.log2_max_poc_lsb.min(31);
                let lsb = i64::from(hdr.poc_lsb);
                let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
                    prev_msb + max_lsb
                } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
                    prev_msb - max_lsb
                } else {
                    prev_msb
                };
                let top = msb + lsb;
                (
                    clip_poc(msb),
                    clip_poc(top),
                    clip_poc(top + i64::from(hdr.delta_poc_bottom)),
                    0,
                )
            }
            1 => {
                let offset = self.frame_num_offset(idr, frame_num, max_frame_num);
                let cycle_len = sps.offset_for_ref_frame.len() as i64;
                let mut absolute = if cycle_len != 0 {
                    offset + i64::from(frame_num)
                } else {
                    0
                };
                if nal_ref_idc == 0 && absolute > 0 {
                    absolute -= 1;
                }
                let mut expected = 0i64;
                if absolute > 0 && cycle_len > 0 {
                    let per_cycle: i64 =
                        sps.offset_for_ref_frame.iter().map(|&o| i64::from(o)).sum();
                    let cycles = (absolute - 1) / cycle_len;
                    let into_cycle = (absolute - 1) % cycle_len;
                    expected = cycles.saturating_mul(per_cycle);
                    expected = expected.saturating_add(
                        sps.offset_for_ref_frame
                            .iter()
                            .take(into_cycle as usize + 1)
                            .map(|&o| i64::from(o))
                            .sum::<i64>(),
                    );
                }
                if nal_ref_idc == 0 {
                    expected = expected.saturating_add(i64::from(sps.offset_for_non_ref_pic));
                }
                let top = expected.saturating_add(i64::from(hdr.delta_poc[0]));
                let bottom = top
                    .saturating_add(i64::from(sps.offset_for_top_to_bottom_field))
                    .saturating_add(i64::from(hdr.delta_poc[1]));
                (0, clip_poc(top), clip_poc(bottom), clip_poc(offset))
            }
            _ => {
                let offset = self.frame_num_offset(idr, frame_num, max_frame_num);
                let value = if idr {
                    0
                } else if nal_ref_idc == 0 {
                    2 * (offset + i64::from(frame_num)) - 1
                } else {
                    2 * (offset + i64::from(frame_num))
                };
                let value = clip_poc(value);
                (0, value, value, clip_poc(offset))
            }
        }
    }

    /// FrameNumOffset (clauses 8.2.1.2 and 8.2.1.3).
    fn frame_num_offset(&self, idr: bool, frame_num: u32, max_frame_num: i64) -> i64 {
        let previous = if self.prev_has_mmco5 {
            0
        } else {
            i64::from(self.prev_frame_num_offset)
        };
        if idr {
            0
        } else if self.prev_frame_num > frame_num {
            previous.saturating_add(max_frame_num)
        } else {
            previous
        }
    }

    /// The decoding process for gaps in frame_num (clause 8.2.5.2).
    fn fill_frame_num_gap(
        &mut self,
        hdr: &SliceHeader,
        sps: &Sps,
        mb_w: usize,
        mb_h: usize,
    ) -> Result<()> {
        // The frames a gap invents may push out of the buffer, at once,
        // every reference the pictures in flight predict from; those
        // finish first, so that what they hold stays within what the
        // memory budget counts ([`most_pictures`]).
        self.retire(Wait::All);
        let max = sps.max_frame_num();
        let mut frame_num = (self.prev_ref_frame_num + 1) % max;
        let gap = (hdr.frame_num + max - frame_num) % max;
        // Only the last Max(max_num_ref_frames, 1) inferred frames can
        // survive the sliding window; the earlier ones share one picture,
        // so a long gap costs no allocations.
        let keep = sps.max_num_ref_frames.max(1);
        let shared_slot = self.picture_slot()?;
        let shared =
            Arc::new(Picture::black(1, 1, &self.limits)?.with_output(0, [0; 4], shared_slot, 0));
        let mut made = 0;
        while frame_num != hdr.frame_num {
            made += 1;
            if made > max {
                return Err(Error::Invalid("a frame_num gap longer than MaxFrameNum"));
            }
            let mut gap_hdr = hdr.clone();
            gap_hdr.frame_num = frame_num;
            gap_hdr.delta_poc = [0, 0];
            gap_hdr.idr = false;
            gap_hdr.nal_ref_idc = 1;
            let (_, top, bottom, offset) =
                self.picture_order_count(&gap_hdr, sps, frame_num, 1, false);
            let picture = if gap - made < keep {
                let geometry = Geometry::new(mb_w, mb_h, self.band_mbs(mb_h));
                let (fresh, bands) = self.new_picture(geometry)?;
                let slot = self.picture_slot()?;
                let poc = if sps.poc_type == 0 {
                    0
                } else {
                    top.min(bottom)
                };
                let picture = Arc::new(fresh.with_output(poc, [0; 4], slot, 0));
                // In the pool like every picture made, so that pruning
                // counts it.
                self.pool.push(Arc::clone(&picture));
                let mut canvas = Canvas::new(Arc::clone(&picture), bands);
                // "The sample values of the generated frame may be set to
                // any value" (clause 8.2.5.2). The previous reference
                // picture is the most useful concealment if a stream that
                // does not conform predicts from it; it is copied band by
                // band as its decoding publishes them.
                //
                // Where there is no reference to conceal from, the picture
                // is grey, as `Picture::black` leaves a new picture: it
                // comes from the pool, which decoding no longer clears, and
                // without this it would keep the samples of whichever
                // picture used the buffer last, and this one is kept as a
                // reference that a later picture may predict from.
                //
                // Either way the motion is cleared, and for a sharper
                // reason. No slice covers this picture, so nothing writes
                // its motion and nothing clears it, and it is stored as a
                // reference whose co-located motion a later picture's
                // direct prediction reads (`inter::colocated`): leaving it
                // would let an unrelated earlier picture's vectors decide
                // this stream's samples -- and which earlier picture,
                // through the pool, depends on how long the caller held the
                // pictures it was given.
                let concealed = self
                    .last_reference
                    .as_ref()
                    .is_some_and(|previous| canvas.conceal_from(previous));
                if !concealed {
                    canvas.fill_black();
                }
                canvas.publish_all()?;
                self.last_reference = Some(Arc::clone(&picture));
                picture
            } else {
                Arc::clone(&shared)
            };
            self.dpb
                .sliding_window(sps.max_num_ref_frames, frame_num, max);
            self.dpb.store(Entry {
                picture,
                frame_num,
                long_term_frame_idx: 0,
                mark: RefMark::Short,
                needed_for_output: false,
                non_existing: true,
            });
            self.prev_frame_num = frame_num;
            self.prev_frame_num_offset = offset;
            self.prev_has_mmco5 = false;
            self.prev_ref_frame_num = frame_num;
            frame_num = (frame_num + 1) % max;
        }
        Ok(())
    }

    /// The slot of the picture about to start (`Picture::slot`): the first,
    /// counting on from the last one handed out, that no picture of the
    /// decoded picture buffer holds (`docs/design/h264.md`, "Motion round:
    /// a block's motion in 12 bytes, on the current decoder").
    ///
    /// Every picture a picture's blocks refer to is in its reference
    /// lists, so in the buffer when it starts, and so holds a slot no other
    /// picture there holds: comparing slots is comparing pictures, which is
    /// all the deblocking filter and temporal direct prediction ask of
    /// them. The buffer holds at most 18 pictures (16 frames, Table A-1 and
    /// `max_num_ref_frames`, and those `Dpb::store` keeps past `dpb_size`
    /// where no picture can be bumped), far fewer than the 255 slots; none
    /// free is refused all the same.
    fn picture_slot(&mut self) -> Result<u8> {
        let held = self.dpb.entries.iter().map(|e| e.picture.slot);
        let slot = free_slot(held, self.next_slot)
            .ok_or(Error::Unsupported("a slot for each picture of the buffer"))?;
        self.next_slot = (usize::from(slot) + 1) % SLOTS;
        Ok(slot)
    }

    /// A picture of `geometry` whose bands are not published, and buffers
    /// for its bands: those of a picture nothing else holds, or none.
    fn new_picture(&mut self, geometry: Geometry) -> Result<(Picture, Vec<BandData>)> {
        let picture = Picture::new(geometry, &self.limits)?;
        let free = self
            .pool
            .iter()
            .position(|p| Arc::strong_count(p) == 1 && *p.geometry() == geometry);
        if let Some(i) = free {
            let old = self.pool.swap_remove(i);
            if let Ok(old) = Arc::try_unwrap(old) {
                self.pooled = self.pooled.saturating_add(1);
                return Ok((picture, old.into_bands()));
            }
        }
        // Keep the pool bounded: drop the pictures nobody else holds.
        if self.pool.len() >= most_pictures(self.dpb.size, self.threads) {
            self.pool.retain(|p| Arc::strong_count(p) > 1);
        }
        Ok((picture, Vec::new()))
    }

    /// Finishes the picture being decoded: filters the rows that are left,
    /// marks references (clause 8.2.5) and stores it.
    ///
    /// A picture decoded on the caller's thread is finished here, and its
    /// error returned; one decoded by a worker is told it has no more
    /// slices, and its error is reported when it is retired. Either way
    /// the picture is marked and stored: that depends on headers alone.
    fn finish_picture(&mut self) -> Result<()> {
        let Some(current) = self.current.take() else {
            return Ok(());
        };
        let result = match current.feed {
            Feed::Inline(work) => {
                let (result, spare) = work.finish();
                self.spare = spare;
                result
            }
            Feed::Worker(index) => {
                if let Some(worker) = self.workers.list.get_mut(index) {
                    worker.send(Message::Finish);
                }
                Ok(())
            }
        };
        let hdr = &current.hdr;
        let sps = &current.sps;
        let max_frame_num = sps.max_frame_num();
        let mmco5 = hdr.has_mmco5();
        if mmco5 {
            self.after_random_access = false;
        }
        let mut top = current.top;
        if mmco5 {
            // The picture order count of a picture with marking operation 5
            // becomes 0 (clause 8.2.1); the picture has carried 0 since it
            // started (`start_picture`).
            let smallest = current.top.min(current.bottom);
            top -= smallest;
        }
        let mut mark = RefMark::Unused;
        let mut long_term_frame_idx = 0;
        if hdr.nal_ref_idc != 0 {
            if hdr.idr {
                for e in self.dpb.entries.iter_mut() {
                    e.mark = RefMark::Unused;
                }
                if hdr.no_output_of_prior_pics {
                    self.dpb.clear();
                } else {
                    self.dpb.flush();
                }
                if hdr.long_term_reference {
                    mark = RefMark::Long;
                    self.dpb.max_long_term_frame_idx = Some(0);
                } else {
                    mark = RefMark::Short;
                    self.dpb.max_long_term_frame_idx = None;
                }
            } else if hdr.adaptive_marking {
                let long = self.dpb.adaptive(&hdr.mmcos, hdr.frame_num, max_frame_num);
                // The current picture takes one more reference slot.
                self.dpb.enforce_max_refs(
                    sps.max_num_ref_frames.saturating_sub(1),
                    hdr.frame_num,
                    max_frame_num,
                );
                if mmco5 {
                    self.dpb.flush();
                }
                match long {
                    Some(idx) => {
                        mark = RefMark::Long;
                        long_term_frame_idx = idx;
                    }
                    None => mark = RefMark::Short,
                }
            } else {
                self.dpb
                    .sliding_window(sps.max_num_ref_frames, hdr.frame_num, max_frame_num);
                mark = RefMark::Short;
            }
        }
        // The state the next picture starts from.
        if hdr.nal_ref_idc != 0 {
            if mmco5 {
                self.prev_ref_poc_msb = 0;
                self.prev_ref_poc_lsb = top;
                self.prev_ref_frame_num = 0;
            } else {
                self.prev_ref_poc_msb = current.poc_msb;
                self.prev_ref_poc_lsb = hdr.poc_lsb as i32;
                self.prev_ref_frame_num = hdr.frame_num;
            }
        }
        self.prev_frame_num = if mmco5 { 0 } else { hdr.frame_num };
        self.prev_frame_num_offset = current.frame_num_offset;
        self.prev_has_mmco5 = mmco5;
        let frame_num = if mmco5 { 0 } else { hdr.frame_num };
        // In keyframe mode a picture that turned out not to be one is
        // decoded no further and not returned.
        let needed_for_output =
            current.output && (current.keyframe || self.mode != Mode::KeyframesOnly);
        let picture = current.picture;
        if mark != RefMark::Unused {
            self.last_reference = Some(Arc::clone(&picture));
        }
        self.dpb.store(Entry {
            picture,
            frame_num,
            long_term_frame_idx,
            mark,
            needed_for_output,
            non_existing: false,
        });
        if self.mode == Mode::KeyframesOnly {
            // Every picture decoded in this mode is a keyframe, and the
            // mode returns nothing else, so there is nothing for it to
            // wait behind.
            self.dpb.flush();
        } else {
            self.dpb.output_reordered();
        }
        self.release_output();
        result
    }
}

/// [`Decoder::most_pictures`] for a decoded picture buffer of `dpb_size`
/// frames.
///
/// A picture in flight holds the references its slices predict from, and
/// the buffer may let them go meanwhile (a sliding window, a marking
/// operation). Each of them was in the buffer when the oldest picture in
/// flight started, or is a picture started since (counted in flight), so
/// they are at most `dpb_size` more. A frame_num gap would add more at
/// once, so the pictures in flight finish before one is filled
/// ([`Decoder::fill_frame_num_gap`]).
fn most_pictures(dpb_size: usize, threads: usize) -> usize {
    dpb_size
        .saturating_mul(2)
        .saturating_add(5)
        .saturating_add(threads.max(1))
}

/// The bytes of a picture parameter set the decoder reads and keeps: far
/// more than the syntax of one this decoder decodes (no slice groups) can
/// take, about 0.5 KB, and enough to reach `num_slice_groups_minus1`, so
/// that a set with slice groups is still refused as unsupported. What
/// lies beyond is not read.
pub(crate) const MAX_PPS_BYTES: usize = 4 << 10;

/// The bytes of a sequence parameter set the decoder reads: more than its
/// syntax can take with every list and table at its largest (about 4.2 KB
/// with six 8x8 scaling lists, 4:4:4), so that every set parses as it did
/// and one this decoder does not decode is still refused as unsupported.
/// It is kept parsed, not as its bytes.
pub(crate) const MAX_SPS_BYTES: usize = 16 << 10;

/// What each thread's decoding costs beyond the pictures, per macroblock
/// of the picture it decodes: the record of every macroblock, the ring of
/// what neighbours read (`crate::mb::ring_entries`, at most twice the
/// picture's width, so at most two entries a macroblock), and the
/// deblocking parameters of up to one slice a macroblock, in a vector that
/// may have grown to twice that.
const THREAD_BYTES_PER_MB: u64 = (size_of::<crate::mb::MbInfo>()
    + 2 * size_of::<crate::mb::NbInfo>()
    + 2 * size_of::<crate::deblock::SliceFilter>()) as u64;

/// What each thread costs whatever the pictures' size: the buffers of the
/// macroblock it decodes and the bookkeeping of its queue. A bound.
const THREAD_BYTES: u64 = 128 << 10;

/// What a decoder costs whatever its pictures' size: its tables of
/// parameter sets (256 picture parameter sets of at most
/// [`MAX_PPS_BYTES`] each, what they resolve to, and the sequence
/// parameter set each resolved one may keep alive after the stream has
/// replaced it: about 1.7 MB at most), the [`MAX_SPS_BYTES`] a sequence
/// parameter set is read from, its [`MAX_WARNINGS`] warnings, and what a
/// caller around it keeps besides the pictures. A bound.
const DECODER_BYTES: u64 = 4 << 20;

/// [`Decoder::memory_needed`] with bands of `band_mbs` macroblock rows and
/// a decoded picture buffer of `dpb_size` frames. Beside the pictures and
/// the threads' state it counts the slice data the decoder holds: the NAL
/// unit being read and, with threads, each worker's queue, at most
/// [`QUEUE_BYTES`] or one slice larger than that, and [`QUEUE_MESSAGES`]
/// slices of [`JOB_BYTES`] each besides their data.
fn need(
    mb_w: usize,
    mb_h: usize,
    threads: usize,
    band_mbs: usize,
    dpb_size: usize,
    limits: &Limits,
) -> u64 {
    let mbs = mb_w.saturating_mul(mb_h);
    if mbs == 0 || mbs > limits.max_mbs {
        // Refused for its size before its memory matters.
        return u64::MAX;
    }
    let threads = threads.max(1);
    let pictures = most_pictures(dpb_size, threads) as u64;
    let stored = Geometry::new(mb_w, mb_h, band_mbs).stored_bytes();
    let mbs = mbs as u64;
    let samples = mbs.saturating_mul(384);
    let released = (dpb_size as u64)
        .saturating_add(1)
        .saturating_add(threads as u64);
    let per_thread = mbs
        .saturating_mul(THREAD_BYTES_PER_MB)
        .saturating_add(THREAD_BYTES);
    let nal = limits.max_nal_bytes as u64;
    let queues = if threads > 1 {
        // A queue holds at most QUEUE_BYTES of slice data, or one slice
        // larger than that, and at most QUEUE_MESSAGES slices.
        let data = (QUEUE_BYTES as u64)
            .min(nal.saturating_mul(QUEUE_MESSAGES as u64))
            .max(nal);
        let jobs = (QUEUE_MESSAGES as u64).saturating_mul(JOB_BYTES as u64);
        data.saturating_add(jobs).saturating_mul(threads as u64)
    } else {
        0
    };
    stored
        .saturating_mul(pictures)
        .saturating_add(samples.saturating_mul(released))
        .saturating_add(per_thread.saturating_mul(threads as u64))
        .saturating_add(nal)
        .saturating_add(queues)
        .saturating_add(DECODER_BYTES)
}

/// How long [`Decoder::retire`] waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// Not at all: the pictures finished so far.
    No,
    /// For the oldest picture in flight.
    Oldest,
    /// For every picture in flight.
    All,
}

/// A picture order count that fits the specification's range
/// (clause 8.2.1): -2^31 to 2^31 - 1.
fn clip_poc(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Rejects streams outside this decoder's scope (`docs/design/h264.md`).
fn check_scope(sps: &Sps, pps: &Pps) -> Result<()> {
    check_sps_scope(sps)?;
    check_pps_scope(pps)
}

/// What a sequence parameter set alone decides about the scope.
fn check_sps_scope(sps: &Sps) -> Result<()> {
    if sps.chroma_format_idc != 1 {
        return Err(Error::Unsupported("a chroma format other than 4:2:0"));
    }
    if sps.bit_depth_luma != 8 || sps.bit_depth_chroma != 8 {
        return Err(Error::Unsupported("a bit depth other than 8"));
    }
    if sps.transform_bypass {
        return Err(Error::Unsupported("lossless transform bypass"));
    }
    if sps.mb_adaptive_frame_field {
        return Err(Error::Unsupported("macroblock-adaptive frame/field coding"));
    }
    if !sps.frame_mbs_only {
        // The stream may code field pictures, which this decoder does
        // not decode (clause 7.4.2.1.1). A caller is told here rather
        // than at the first field picture, which may be far into a
        // stream it could have given to another decoder.
        return Err(Error::Unsupported("a stream that may code field pictures"));
    }
    Ok(())
}

/// What a picture parameter set alone decides.
fn check_pps_scope(pps: &Pps) -> Result<()> {
    if pps.num_slice_groups > 1 {
        return Err(Error::Unsupported("slice groups"));
    }
    Ok(())
}

/// Detection of the first slice of a new primary coded picture
/// (clause 7.4.1.2.4).
impl Boundary {
    /// What clause 7.4.1.2.4 compares, out of a slice header. The header
    /// itself owns vectors; this is what a picture boundary needs, and it
    /// is copied rather than allocated once per slice.
    fn of(h: &SliceHeader) -> Boundary {
        Boundary {
            frame_num: h.frame_num,
            pps_id: h.pps_id,
            field_pic: h.field_pic,
            reference: h.nal_ref_idc != 0,
            poc_lsb: h.poc_lsb,
            delta_poc_bottom: h.delta_poc_bottom,
            delta_poc: h.delta_poc,
            idr: h.idr,
            idr_pic_id: h.idr_pic_id,
            first_mb: h.first_mb,
        }
    }

    /// Whether the slice `h` belongs to a picture after the one this
    /// slice belonged to (clause 7.4.1.2.4).
    fn starts_new_picture(&self, h: &SliceHeader, sps: &Sps) -> bool {
        self.frame_num != h.frame_num
            || self.pps_id != h.pps_id
            || self.field_pic != h.field_pic
            || self.reference != (h.nal_ref_idc != 0)
            || (sps.poc_type == 0
                && (self.poc_lsb != h.poc_lsb || self.delta_poc_bottom != h.delta_poc_bottom))
            || (sps.poc_type == 1 && self.delta_poc != h.delta_poc)
            || self.idr != h.idr
            || (self.idr && h.idr && self.idr_pic_id != h.idr_pic_id)
    }
}

/// The first slot from `from` on, wrapping, of the [`SLOTS`] slots, that
/// no slot of `held` is; `None` if every one is held.
fn free_slot(held: impl Iterator<Item = u8>, from: usize) -> Option<u8> {
    let mut taken = [false; SLOTS];
    for h in held {
        if let Some(t) = taken.get_mut(usize::from(h)) {
            *t = true;
        }
    }
    (0..SLOTS)
        .map(|k| (from + k) % SLOTS)
        .find(|&slot| !taken.get(slot).copied().unwrap_or(true))
        .and_then(|slot| u8::try_from(slot).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annex_b;
    use crate::picture::NO_MOTION;
    use crate::test_streams::*;

    #[test]
    fn small_pictures_use_one_band_only_at_eight_threads_without_an_override() {
        for mb_h in [30, 31, 32] {
            for threads in [1, 7, 8, 16] {
                for band_height in [None, Some(1), Some(3)] {
                    for reference in [false, true] {
                        let mut decoder = Decoder::new(Limits::default()).with_threads(threads);
                        if let Some(rows) = band_height {
                            decoder = decoder.with_band_height(rows);
                        }
                        let mut stream = nal(3, 7, sps_with_max_refs(1, mb_h, 1));
                        stream.extend(nal(3, 8, pps()));
                        stream.extend(nal(3, 5, idr_pcm_slice(mb_h as usize, pcm_samples)));
                        stream.extend(nal(
                            if reference { 3 } else { 0 },
                            1,
                            skipped_p_slice_ref(1, 2, mb_h as usize, reference),
                        ));
                        for unit in annex_b(&stream) {
                            decoder.decode_nal(unit).expect("the stream decodes");
                        }
                        decoder.flush().expect("the stream flushes");
                        let expected = band_height.unwrap_or_else(|| {
                            if !reference || threads == 8 && mb_h < 32 {
                                mb_h as usize
                            } else {
                                Geometry::band_mbs_for(mb_h as usize, threads)
                            }
                        });
                        assert_eq!(
                            decoder
                                .pool
                                .last()
                                .expect("the last picture")
                                .geometry()
                                .band_mbs(),
                            expected,
                            "height {mb_h}, threads {threads}, override {band_height:?}, reference {reference}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_memory_budget_counts_the_slack_rows_of_a_picture_of_one_band() {
        // 1920x1080, coded as 120 by 68 macroblocks. Each frame more that
        // the decoded picture buffer may hold adds two stored pictures
        // (`most_pictures`) and one copy of the samples a flush releases;
        // a stored picture is its planes and its motion.
        let (mb_w, mb_h) = (120usize, 68usize);
        let (w, h) = (mb_w as u64 * 16, mb_h as u64 * 16);
        let mbs = (mb_w * mb_h) as u64;
        let motion_row = mb_w as u64 * crate::picture::MOTION_BYTES_PER_MB as u64;
        let per_frame = |threads: usize| {
            let at = |frames: usize| {
                let limits = Limits {
                    max_dpb_frames: frames,
                    ..Limits::default()
                };
                Decoder::<Auto>::memory_needed(mb_w, mb_h, threads, &limits)
            };
            at(5) - at(4)
        };
        // One thread: one band, whose planes keep 4 luma and 2 chroma rows
        // of slack before and after them, at the coded width.
        let luma = w * (h + 8);
        let chroma = 2 * (w / 2) * (h / 2 + 4);
        let one = luma + chroma + mb_h as u64 * motion_row;
        assert_eq!(Geometry::new(mb_w, mb_h, mb_h).stored_bytes(), one);
        assert_eq!(per_frame(1), 2 * one + mbs * 384);
        // What `BandData::fit` gives a picture of one band.
        let black = Picture::black(mb_w, mb_h, &Limits::default()).expect("picture");
        let data = black.whole().expect("one band");
        assert_eq!(data.y.len() as u64, luma);
        assert_eq!((data.cb.len() + data.cr.len()) as u64, chroma);
        // Eight threads: bands of 4 macroblock rows, 17 of them, every one
        // but the first with a top apron of 23 luma rows, 11 chroma rows and
        // one macroblock row of motion, and slack before the first band's
        // planes and after the last's.
        assert_eq!(Geometry::band_mbs_for(mb_h, 8), 4);
        let (luma_rows, chroma_rows, motion_rows) =
            (h + 16 * 23 + 8, h / 2 + 16 * 11 + 4, mb_h as u64 + 16);
        let banded = luma_rows * w + 2 * chroma_rows * (w / 2) + motion_rows * motion_row;
        assert_eq!(Geometry::new(mb_w, mb_h, 4).stored_bytes(), banded);
        assert_eq!(per_frame(8), 2 * banded + mbs * 384);
    }

    #[test]
    fn a_frame_invented_for_a_frame_num_gap_carries_no_motion() {
        // The frame invented for a frame_num gap (clause 8.2.5.2) is covered
        // by no slice, so the per-macroblock clears never reach it; it comes
        // from the pool and is kept as a reference, whose motion a later
        // picture's direct prediction reads. `fill_frame_num_gap` clears
        // it. The end-to-end test in `tests/decode.rs` reaches the reader
        // but cannot tell the clear from its absence, so this one looks at
        // the stored frame itself.
        //
        // Skipped P pictures leave ref_idx 0 in every block of their
        // buffers, which is not `NO_MOTION`, and are dropped so that the
        // invented frame takes one of those buffers.
        let mut stream = Vec::new();
        stream.extend(nal(3, 7, sps_with_gaps(2, 2)));
        stream.extend(nal(3, 8, pps()));
        stream.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
        for poc in 1..4 {
            stream.extend(nal(0, 1, skipped_p_slice_ref(0, poc * 2, 4, false)));
        }
        // frame_num jumps from 0 to 2, so frame 1 is invented.
        stream.extend(nal(0, 1, non_idr_pcm_slice_ref(2, 10, 4, false)));

        let limits = Limits {
            max_dpb_frames: 1,
            ..Limits::default()
        };
        let mut decoder = Decoder::new(limits);
        for unit in annex_b(&stream) {
            decoder.decode_nal(unit).expect("the stream decodes");
            drop(decoder.take_pictures());
        }
        assert!(
            decoder.pooled_pictures() > 0,
            "no picture was reused, so the invented frame did not come from a dirty buffer"
        );
        let invented = decoder
            .dpb
            .entries
            .iter()
            .find(|e| e.non_existing && e.frame_num == 1)
            .expect("the invented frame is a reference");
        let stale = invented
            .picture
            .all_motion()
            .iter()
            .filter(|&&m| m != NO_MOTION)
            .count();
        assert_eq!(
            stale, 0,
            "blocks of the invented frame kept another picture's motion"
        );
    }

    #[test]
    fn free_slots_skip_the_held_ones_wrap_and_run_out() {
        let held: Vec<u8> = (0..17).collect();
        assert_eq!(free_slot(held.iter().copied(), 0), Some(17));
        assert_eq!(free_slot(held.iter().copied(), 5), Some(17));
        assert_eq!(free_slot(held.iter().copied(), 200), Some(200));
        // From the last slot, past the held ones at the end and the start.
        let ends: Vec<u8> = (250..255).chain(0..3).collect();
        assert_eq!(free_slot(ends.iter().copied(), 250), Some(3));
        assert_eq!(free_slot(ends.iter().copied(), 254), Some(3));
        // Every slot held: none, whatever the start.
        let all: Vec<u8> = (0..255).collect();
        assert_eq!(free_slot(all.iter().copied(), 0), None);
        assert_eq!(free_slot(all.iter().copied(), 254), None);
        // One left, found from anywhere; never `NO_SLOT`.
        let one_left: Vec<u8> = (0..255).filter(|&s| s != 77).collect();
        for from in 0..SLOTS {
            assert_eq!(free_slot(one_left.iter().copied(), from), Some(77));
        }
        for from in 0..SLOTS {
            assert_ne!(
                free_slot(std::iter::empty(), from),
                Some(crate::picture::NO_SLOT)
            );
        }
    }

    #[test]
    fn slots_stay_distinct_in_a_full_buffer_with_long_term_frames() {
        // A buffer at its largest, 16 frames plus one that cannot be
        // bumped, with long-term frames kept for thousands of pictures
        // while short-term ones come and go: every slot handed out differs
        // from every slot the buffer holds, so that comparing slots is
        // comparing pictures (`Decoder::picture_slot`).
        let limits = Limits::default();
        let mut decoder = Decoder::new(limits);
        let entry = |slot: u8, mark: RefMark, frame_num: u32| Entry {
            picture: Arc::new(
                Picture::black(1, 1, &limits)
                    .expect("a picture")
                    .with_output(0, [0; 4], slot, 0),
            ),
            frame_num,
            long_term_frame_idx: 0,
            mark,
            needed_for_output: false,
            non_existing: false,
        };
        for n in 0..5000u32 {
            let slot = decoder.picture_slot().expect("a free slot");
            assert!(usize::from(slot) < SLOTS);
            assert!(
                decoder.dpb.entries.iter().all(|e| e.picture.slot != slot),
                "picture {n} took slot {slot}, which the buffer holds"
            );
            // Six long-term frames from the first pictures, kept to the end;
            // the short-term ones slide.
            let mark = if n < 6 { RefMark::Long } else { RefMark::Short };
            decoder.dpb.entries.push(entry(slot, mark, n));
            if decoder.dpb.entries.len() > 17 {
                let oldest = decoder
                    .dpb
                    .entries
                    .iter()
                    .position(|e| e.mark == RefMark::Short)
                    .expect("a short-term frame");
                decoder.dpb.entries.remove(oldest);
            }
            let mut slots: Vec<u8> = decoder.dpb.entries.iter().map(|e| e.picture.slot).collect();
            slots.sort_unstable();
            slots.dedup();
            assert_eq!(slots.len(), decoder.dpb.entries.len());
        }
    }
}
