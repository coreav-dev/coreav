//! One decoding request after its track is chosen: the decoder, the
//! accounting of samples (trimming, concealment, limits, warnings) and,
//! with several threads, the pool that decodes packets in parallel
//! (`docs/design/audio-decode.md`, "Threads").

use crate::conversion::Plan;
use crate::convert::{default_mask, interleave};
use crate::decode::{
    Codec, Options, OutputFormat, SampleFormat, SilenceSpan, Sink, StreamFormat, Summary, Timeline,
};
use crate::flac::{FlacDecoder, StreamInfo};
use crate::mp3::{self, Mp3Decoder};
use crate::{Error, LimitKind, Limits, Pcm, Warning};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{Scope, ScopedJoinHandle};

/// The most threads a request uses.
pub const MAX_THREADS: usize = 16;

/// What a job of the pool may cost, in bytes (see [`cost`]).
const JOB_BYTES: u64 = 2 << 20;
/// Packets in one job at most.
const JOB_PACKETS: usize = 256;
/// Bytes the pool may hold, in packets read and samples decoded or being
/// decoded, before they are written: 64 MiB, plus at most one job. A
/// packet costs its bytes plus 8 bytes for each sample it decodes to (its
/// planar buffer and its output), the sample count read from its frame
/// header before it is decoded, so the bound holds whatever a frame codes
/// (RFC 9639 section 11) and however large its packets are.
const IN_FLIGHT_BYTES: u64 = 64 << 20;

/// The cost of a packet of `bytes` bytes that decodes to `samples`
/// samples, all channels counted, with 128 bytes for its bookkeeping (its
/// buffer's header and allocation, its place in a job and in the results).
fn cost(bytes: usize, samples: u64) -> u64 {
    (bytes as u64)
        .saturating_add(samples.saturating_mul(8))
        .saturating_add(128)
}

/// The largest packet buffer kept for reuse, and the most kept in all:
/// buffers outside the bound are freed when their packet is written, so a
/// buffer that once held a large packet does not stay large unseen.
const SPARE_BUFFER_BYTES: usize = 1 << 20;
const SPARE_TOTAL_BYTES: usize = 16 << 20;

/// The most packets, and bytes, a job's priming copies (`Worker::priming`).
const MAX_PRIMING: usize = 64;
const MAX_PRIMING_BYTES: usize = 256 << 10;

/// The most samples per channel a damaged packet is replaced by: 2^16, the
/// largest frame of the codecs decoded (FLAC's 65,535).
const MAX_CONCEALED: u64 = 1 << 16;

#[derive(Clone)]
enum Decoder {
    Flac(FlacDecoder),
    Mp3(Box<Mp3Decoder>),
}

/// A decoder with its buffer: the one of a single-threaded request, or one
/// per thread of the pool.
#[derive(Clone)]
struct Worker {
    decoder: Decoder,
    pcm: Pcm,
    format: SampleFormat,
}

/// What decoding one packet gave besides its samples.
#[derive(Clone, Copy, Debug)]
struct Decoded {
    /// Samples per channel.
    frames: u64,
    crc_ok: bool,
    trailing: bool,
}

impl Worker {
    /// Starts a job: a decoder whose frames depend on the ones before
    /// forgets them (the job's priming packets set its state again).
    fn reset(&mut self) {
        match &mut self.decoder {
            Decoder::Flac(_) => {}
            Decoder::Mp3(decoder) => decoder.reset(),
        }
    }

    /// The packets at the end of `packets` that a job starting after them
    /// decodes first, so that its first packet decodes as it would in one
    /// pass (`docs/design/audio-decode.md`, "Threads"): none for FLAC, whose
    /// frames stand alone. An MP3 frame needs the state of the filter
    /// banks, which each channel's two last granules set (the synthesis
    /// filter reads back into the granule before the last, through its
    /// IMDCT overlap): the frames back to the second granule of each
    /// channel, the second channel only once a stereo frame has passed
    /// (`stereo`; before, its state is the reset one). Those frames need
    /// the main data of the frames before them (`main_data_begin`, at most
    /// 511 bytes back): before them, frames with at least 511 bytes of it.
    /// At most [`MAX_PRIMING`] packets and [`MAX_PRIMING_BYTES`] bytes: a
    /// stream that needs more (frames without main data, long runs of one
    /// channel) is primed with that much, and its job boundaries may then
    /// differ from one pass.
    fn priming(&self, packets: &[&[u8]], stereo: bool) -> usize {
        match &self.decoder {
            Decoder::Flac(_) => 0,
            Decoder::Mp3(_) => {
                let mut count = 0;
                let mut bytes = 0usize;
                let mut granules = [0usize; 2];
                let needed = [2, if stereo { 2 } else { 0 }];
                let mut main = 0;
                for packet in packets.iter().rev() {
                    bytes = bytes.saturating_add(packet.len());
                    if count >= MAX_PRIMING || bytes > MAX_PRIMING_BYTES {
                        break;
                    }
                    count += 1;
                    if granules[0] < needed[0] || granules[1] < needed[1] {
                        if let Ok(header) = mp3::Header::parse(packet) {
                            granules[0] += header.granules();
                            if header.channels() == 2 {
                                granules[1] += header.granules();
                            }
                        }
                    } else {
                        main += mp3::main_bytes(packet);
                        if main >= mp3::RESERVOIR {
                            break;
                        }
                    }
                }
                count
            }
        }
    }

    /// Decodes `data` and writes its samples, interleaved in the output
    /// format, into `out` from byte `at` on, growing it as needed; bytes
    /// past them are left as they were.
    fn decode(&mut self, data: &[u8], out: &mut Vec<u8>, at: usize) -> Result<Decoded, Error> {
        let decoded = match &mut self.decoder {
            Decoder::Flac(decoder) => {
                let frame = decoder.decode(data, &mut self.pcm)?;
                Decoded {
                    frames: u64::from(frame.block_size),
                    crc_ok: frame.crc_ok,
                    trailing: frame.bytes < data.len(),
                }
            }
            Decoder::Mp3(decoder) => {
                let frame = decoder.decode(data, &mut self.pcm)?;
                Decoded {
                    frames: frame.header.samples() as u64,
                    crc_ok: frame.crc_ok,
                    trailing: frame.bytes < data.len(),
                }
            }
        };
        interleave(&self.pcm, 0, self.pcm.frames(), self.format, out, at);
        Ok(decoded)
    }

    /// The samples per channel a packet will decode to, from its header.
    fn frames_hint(&self, data: &[u8]) -> Option<u64> {
        match &self.decoder {
            Decoder::Flac(_) => crate::flac::frame_samples(data).map(u64::from),
            Decoder::Mp3(_) => mp3::Header::parse(data).ok().map(|h| h.samples() as u64),
        }
    }
}

/// A packet handed to the pool.
struct Pending {
    data: Vec<u8>,
    duration: Option<u64>,
    discard: u64,
}

/// Packets for one thread, in stream order.
struct Job {
    seq: u64,
    cost: u64,
    /// Copies of the packets before `packets`, decoded first for the
    /// decoder's state, their samples dropped ([`Worker::priming`]).
    prime: Vec<Pending>,
    packets: Vec<Pending>,
}

/// A decoded job: the packets' samples back to back in `bytes`, and per
/// packet its result and where its samples end.
struct Done {
    seq: u64,
    cost: u64,
    prime: Vec<Pending>,
    packets: Vec<Pending>,
    bytes: Vec<u8>,
    results: Vec<(Result<Decoded, Error>, usize)>,
}

/// What a thread of the pool sends back.
enum Message {
    Done(Box<Done>),
    /// The thread stopped before the queue closed.
    Stopped,
}

/// Sends [`Message::Stopped`] when dropped armed: when its thread unwinds.
struct Alive {
    sender: mpsc::Sender<Message>,
    armed: bool,
}

impl Alive {
    /// The thread ends normally.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.sender.send(Message::Stopped);
        }
    }
}

/// Threads that decode jobs as they come, while the caller's thread reads
/// packets and writes what was decoded, in order.
struct Pool<'scope> {
    jobs: Option<mpsc::Sender<Job>>,
    done: mpsc::Receiver<Message>,
    _threads: Vec<ScopedJoinHandle<'scope, ()>>,
    job: Vec<Pending>,
    job_cost: u64,
    /// The priming packets of the next job, and their cost.
    prime: Vec<Pending>,
    prime_cost: u64,
    /// Whether an MP3 frame of two channels has been handed out.
    stereo: bool,
    next_seq: u64,
    next_commit: u64,
    waiting: BTreeMap<u64, Done>,
    in_flight: u64,
    /// Packet buffers of written jobs, reused.
    spare: Vec<Vec<u8>>,
    /// The capacity of the buffers in `spare`.
    spare_bytes: usize,
}

impl<'scope> Pool<'scope> {
    fn start<'env>(scope: &'scope Scope<'scope, 'env>, threads: usize, worker: &Worker) -> Self {
        let (jobs, queue) = mpsc::channel::<Job>();
        let (finished, done) = mpsc::channel::<Message>();
        let queue = Arc::new(Mutex::new(queue));
        let handles = (0..threads)
            .map(|_| {
                let queue = Arc::clone(&queue);
                let finished = finished.clone();
                let mut worker = worker.clone();
                scope.spawn(move || {
                    // Tells the caller's thread if this one stops early,
                    // which none should: a job it held would never come
                    // back, and the caller would wait for it forever.
                    let guard = Alive {
                        sender: finished.clone(),
                        armed: true,
                    };
                    // The samples of priming packets, dropped.
                    let mut scratch = Vec::new();
                    loop {
                        let job = match queue.lock() {
                            Ok(queue) => queue.recv(),
                            Err(_) => return,
                        };
                        // A closed queue: the request is done.
                        let Ok(job) = job else {
                            break;
                        };
                        worker.reset();
                        for pending in &job.prime {
                            let _ = worker.decode(&pending.data, &mut scratch, 0);
                        }
                        let mut bytes = Vec::new();
                        let mut results = Vec::with_capacity(job.packets.len());
                        for pending in &job.packets {
                            let at = bytes.len();
                            let result = worker.decode(&pending.data, &mut bytes, at);
                            results.push((result, bytes.len()));
                        }
                        let done = Done {
                            seq: job.seq,
                            cost: job.cost,
                            prime: job.prime,
                            packets: job.packets,
                            bytes,
                            results,
                        };
                        if finished.send(Message::Done(Box::new(done))).is_err() {
                            break;
                        }
                    }
                    guard.disarm();
                })
            })
            .collect();
        Self {
            jobs: Some(jobs),
            done,
            _threads: handles,
            job: Vec::new(),
            job_cost: 0,
            prime: Vec::new(),
            prime_cost: 0,
            stereo: false,
            next_seq: 0,
            next_commit: 0,
            waiting: BTreeMap::new(),
            in_flight: 0,
            spare: Vec::new(),
            spare_bytes: 0,
        }
    }
}

/// A piece of a stream's presentation timeline, in samples per channel
/// (`docs/design/audio-decode.md`, "Edit lists").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    /// Silence: an empty edit.
    Silence(u64),
    /// The decoded samples `from..to`, counted from the stream's first
    /// packet.
    Media { from: u64, to: u64 },
}

/// The accounting of a request: what was decoded and written, the
/// trimming, the limits and the warnings.
pub(crate) struct Output {
    pub(crate) format: StreamFormat,
    pub(crate) output: OutputFormat,
    limits: Limits,
    /// What is still to write, in order: by default every decoded sample.
    pub(crate) plan: VecDeque<Piece>,
    /// Samples per channel written before the first presented media sample,
    /// once one is written.
    media_start: Option<u64>,
    /// The spans of silence the empty edits wrote.
    silence: Vec<SilenceSpan>,
    /// Samples per channel decoded so far, before trimming.
    decoded: u64,
    /// Samples per channel written.
    written: u64,
    packets: u64,
    /// Samples per channel of the last packet that decoded.
    last_frames: u64,
    concealed: u64,
    first_error: Option<String>,
    crc_failures: u64,
    trailing: u64,
    pub(crate) warnings: Vec<Warning>,
}

impl Output {
    /// Accounts for one decoded packet, in stream order, and writes its kept
    /// samples (`bytes`, interleaved), or silence for a packet that failed.
    fn commit(
        &mut self,
        result: Result<Decoded, Error>,
        bytes: &[u8],
        duration: Option<u64>,
        discard: u64,
        sink: &mut dyn Sink,
    ) -> Result<(), Error> {
        self.packets += 1;
        match result {
            Ok(decoded) => {
                self.crc_failures += u64::from(!decoded.crc_ok);
                self.trailing += u64::from(decoded.trailing);
                self.last_frames = decoded.frames;
                self.emit(decoded.frames.saturating_sub(discard), Some(bytes), sink)
            }
            // Limits, and streams the decoder refuses as a whole (a frame
            // that changes the format), end the request; anything else is a
            // damaged packet.
            Err(error @ (Error::LimitExceeded(_) | Error::Unsupported(_))) => Err(error),
            Err(error) => {
                self.concealed += 1;
                if self.concealed > self.limits.max_decode_errors {
                    return Err(Error::LimitExceeded(LimitKind::DecodeErrors));
                }
                self.first_error.get_or_insert_with(|| error.to_string());
                let frames = duration.unwrap_or(self.last_frames).min(MAX_CONCEALED);
                self.emit(frames.saturating_sub(discard), None, sink)
            }
        }
    }

    /// Writes what the timeline keeps of a packet's `frames` samples per
    /// channel: `bytes` holds them interleaved, or `None` writes silence.
    /// Silence of the empty edits it reaches is written before or after.
    fn emit(
        &mut self,
        frames: u64,
        bytes: Option<&[u8]>,
        sink: &mut dyn Sink,
    ) -> Result<(), Error> {
        let start = self.decoded;
        let end = start.saturating_add(frames);
        self.decoded = end;
        while let Some(&piece) = self.plan.front() {
            match piece {
                Piece::Silence(samples) => {
                    self.silence(samples, sink)?;
                    self.plan.pop_front();
                }
                Piece::Media { from, to } => {
                    let (first, last) = (start.max(from), end.min(to));
                    if first < last {
                        if self.media_start.is_none() {
                            self.media_start = Some(self.written);
                        }
                        self.write(bytes, first - start, last - first, sink)?;
                    }
                    if end < to {
                        break;
                    }
                    self.plan.pop_front();
                }
            }
        }
        Ok(())
    }

    /// Writes `samples` of an empty edit's silence and records the span.
    fn silence(&mut self, samples: u64, sink: &mut dyn Sink) -> Result<(), Error> {
        if samples == 0 {
            return Ok(());
        }
        let at = self.written;
        self.write(None, 0, samples, sink)?;
        self.silence.push(SilenceSpan { at, samples });
        Ok(())
    }

    /// The end of the stream: the silence still planned is written (an
    /// empty edit after the last media edit), media past the decoded
    /// samples is not there to write.
    fn close(&mut self, sink: &mut dyn Sink) -> Result<(), Error> {
        while let Some(piece) = self.plan.pop_front() {
            if let Piece::Silence(samples) = piece {
                self.silence(samples, sink)?;
            }
        }
        Ok(())
    }

    /// Writes `count` samples per channel from sample `offset` of `bytes`,
    /// or `count` of silence: checked against `max_output_samples` before
    /// anything is written, and silence in chunks of at most 4 KiB.
    fn write(
        &mut self,
        bytes: Option<&[u8]>,
        offset: u64,
        count: u64,
        sink: &mut dyn Sink,
    ) -> Result<(), Error> {
        // Past u64 is past any limit, `u64::MAX` included.
        let written = self.written.checked_add(count);
        let total = written.and_then(|w| w.checked_mul(u64::from(self.output.channels)));
        let (Some(written), Some(total)) = (written, total) else {
            return Err(Error::LimitExceeded(LimitKind::OutputSamples));
        };
        if total > self.limits.max_output_samples {
            return Err(Error::LimitExceeded(LimitKind::OutputSamples));
        }
        let frame = usize::from(self.output.channels).saturating_mul(self.output.format.bytes());
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let count_usize = usize::try_from(count).unwrap_or(usize::MAX);
        match bytes {
            Some(bytes) => {
                let first = offset.saturating_mul(frame);
                let last = first.saturating_add(count_usize.saturating_mul(frame));
                sink.write(bytes.get(first..last).unwrap_or_default())?;
            }
            None => {
                // Zero bytes are silence in every output format; each write
                // is whole frames, as `Sink` promises (4 KiB is not a
                // multiple of a 3-byte or 24-byte frame).
                let zeros = vec![0u8; (4096 / frame.max(1)).max(1) * frame];
                let frame = u64::try_from(frame).unwrap_or(u64::MAX);
                let mut left = u128::from(count) * u128::from(frame);
                while left > 0 && !zeros.is_empty() {
                    let n = usize::try_from(left).map_or(zeros.len(), |left| left.min(zeros.len()));
                    sink.write(zeros.get(..n).unwrap_or_default())?;
                    left -= u128::try_from(n).unwrap_or(left);
                }
            }
        }
        self.written = written;
        Ok(())
    }
}

/// The state of one decoding request.
pub(crate) struct Stream<'scope> {
    worker: Worker,
    pool: Option<Pool<'scope>>,
    bytes: Vec<u8>,
    /// MP3: frames cut from packets that are not one frame each.
    reframer: Option<mp3::frames::Reframer>,
    frame: Vec<u8>,
    /// Samples per channel handed to the decoder so far, from the frame
    /// headers; cut frames that start at or after `end` are dropped, as the
    /// container's packets are by their timestamps (`decode_mp4`), so where
    /// decoding stops never depends on the threads.
    position: u64,
    pub(crate) end: Option<u64>,
    /// Whether a packet was cut into frames.
    reframed: bool,
    /// The conversion to another rate or channel count, taken by the
    /// caller to wrap its sink.
    pub(crate) plan: Option<Plan>,
    pub(crate) out: Output,
}

/// What the container states about a stream, when it does.
#[derive(Clone, Copy, Default)]
pub(crate) struct Declared {
    pub(crate) sample_rate: Option<u32>,
    pub(crate) channels: Option<u32>,
}

impl<'scope> Stream<'scope> {
    /// A request for `codec` with its configuration `config`; the
    /// container's sample rate and channel count, when it states them, are
    /// cross-checked against the configuration's.
    pub(crate) fn new<'env>(
        codec: Codec,
        config: &[u8],
        first: Option<&[u8]>,
        container: Declared,
        options: &Options,
        limits: &Limits,
        scope: &'scope Scope<'scope, 'env>,
    ) -> Result<Self, Error> {
        let mut reframer = None;
        let (decoder, format) = match codec {
            Codec::Flac => {
                let info = StreamInfo::from_config(config)?;
                let format = StreamFormat {
                    codec,
                    sample_rate: info.sample_rate,
                    channels: u16::from(info.channels),
                    bits_per_sample: Some(u32::from(info.bits_per_sample)),
                };
                (Decoder::Flac(FlacDecoder::new(info, limits)?), format)
            }
            Codec::Mp3 => {
                // MP3 has no configuration: the first frame gives the
                // format. Layers I and II share the codec IDs of some
                // containers and are refused by name.
                // A packet that does not start with a frame (samples of any
                // size, "Reframing" below): its first header.
                let header = match first.map(|data| {
                    mp3::Header::parse(data).or_else(|error| match error {
                        Error::Unsupported(_) => Err(error),
                        _ => (0..data.len())
                            .find_map(|at| {
                                data.get(at..)
                                    .and_then(|rest| mp3::Header::parse(rest).ok())
                            })
                            .ok_or(error),
                    })
                }) {
                    Some(Ok(header)) => header,
                    Some(Err(Error::Unsupported(_))) => {
                        // The layer bits of the header (ISO/IEC 11172-3
                        // 2.4.2.3): `11` is Layer I.
                        let layer_one = first
                            .and_then(|d| d.get(1))
                            .is_some_and(|b| (b >> 1) & 3 == 3);
                        let name = if layer_one { "mp1" } else { "mp2" };
                        return Err(Error::CodecNotDecoded(name.to_owned()));
                    }
                    Some(Err(error)) => return Err(error),
                    None => return Err(Error::Unsupported("an MP3 stream without frames")),
                };
                // The first frame is the stream's configuration: its
                // channel count holds for the stream (the mode may change
                // from frame to frame), and a container that states
                // another gets the mismatch warning below.
                let channels = header.channels() as u16;
                let format = StreamFormat {
                    codec,
                    sample_rate: header.sample_rate,
                    channels,
                    bits_per_sample: None,
                };
                let mut decoder = Mp3Decoder::new(Some(channels), limits)?;
                decoder.expect_sample_rate(header.sample_rate);
                reframer = Some(mp3::frames::Reframer::new(&header));
                (Decoder::Mp3(Box::new(decoder)), format)
            }
        };
        let mut warnings = Vec::new();
        // The container's values are cross-checked; the codec
        // configuration wins.
        if let Some(rate) = container
            .sample_rate
            .filter(|&r| r != 0 && r != format.sample_rate)
        {
            // An MP4 sample entry holds rates of 2^16 Hz and above as 0 or
            // truncated; only a rate that fits is compared.
            if format.sample_rate < 65_536 {
                warnings.push(Warning::ContainerMismatch {
                    field: "sample_rate",
                    container: u64::from(rate),
                    codec: u64::from(format.sample_rate),
                });
            }
        }
        if let Some(channels) = container
            .channels
            .filter(|&c| c != u32::from(format.channels))
        {
            warnings.push(Warning::ContainerMismatch {
                field: "channels",
                container: u64::from(channels),
                codec: u64::from(format.channels),
            });
        }
        let output = OutputFormat {
            sample_rate: format.sample_rate,
            channels: format.channels,
            channel_mask: default_mask(format.channels),
            format: options.format,
        };
        // Another rate or channel count: the stream is written in floating
        // point to the conversion, which rounds once to the asked format.
        let plan = Plan::new(
            &output,
            options.sample_rate,
            options.channels,
            options.format,
            limits,
        )?;
        let output = OutputFormat {
            format: if plan.is_some() {
                SampleFormat::F32
            } else {
                options.format
            },
            ..output
        };
        let threads = match options.threads {
            0 => std::thread::available_parallelism().map_or(1, usize::from),
            n => n,
        }
        .clamp(1, MAX_THREADS);
        let worker = Worker {
            decoder,
            pcm: Pcm::new(),
            format: output.format,
        };
        let pool = (threads > 1).then(|| Pool::start(scope, threads, &worker));
        Ok(Self {
            worker,
            pool,
            bytes: Vec::new(),
            reframer,
            frame: Vec::new(),
            position: 0,
            end: None,
            reframed: false,
            plan,
            out: Output {
                format,
                output,
                limits: *limits,
                plan: VecDeque::from([Piece::Media {
                    from: 0,
                    to: u64::MAX,
                }]),
                media_start: None,
                silence: Vec::new(),
                decoded: 0,
                written: 0,
                packets: 0,
                last_frames: 0,
                concealed: 0,
                first_error: None,
                crc_failures: 0,
                trailing: 0,
                warnings,
            },
        })
    }

    /// Whether packets are cut into frames, whose times are their own.
    pub(crate) fn reframed(&self) -> bool {
        self.reframed
    }

    pub(crate) fn sink_started(&mut self, sink: &mut dyn Sink) -> Result<(), Error> {
        sink.start(&self.out.output)
    }

    /// The demuxer failed after the stream started: before any packet it is
    /// the request's error; after one, the stream ends there with a
    /// `DemuxStopped` warning, keeping what was decoded (a truncated or
    /// damaged end of file). An I/O error and a limit always fail.
    pub(crate) fn demux_error(&mut self, error: Error) -> Result<(), Error> {
        let pending = self.pool.as_ref().map_or(0, |pool| {
            pool.job.len() as u64 + pool.next_seq.saturating_sub(pool.next_commit)
        });
        // A limit is the request's error wherever it is reached, as is I/O.
        if self.out.packets == 0 && pending == 0
            || crate::decode::is_io(&error)
            || matches!(error, Error::LimitExceeded(_))
        {
            return Err(error);
        }
        self.out.warnings.push(Warning::DemuxStopped {
            after_packets: self.out.packets + pending,
            error: error.to_string(),
        });
        Ok(())
    }

    /// Takes one packet. `duration` is its length in samples per channel
    /// when the container states it, used to conceal a packet that fails;
    /// `discard` samples are dropped from its end. With one thread the
    /// packet is decoded at once; with more, it goes to the pool, and is
    /// written in order once decoded.
    pub(crate) fn packet(
        &mut self,
        data: &[u8],
        duration: Option<u64>,
        discard: u64,
        sink: &mut dyn Sink,
    ) -> Result<(), Error> {
        let Some(mut reframer) = self.reframer.take() else {
            return self.decode_packet(data, duration, discard, sink);
        };
        let result = if reframer.passes(data) {
            // After frames were cut, `decode_mp4` no longer stops by
            // timestamps: a whole packet past `end` is dropped here.
            if self.end.is_some_and(|end| self.position >= end) {
                Ok(())
            } else {
                self.position = self
                    .position
                    .saturating_add(self.worker.frames_hint(data).unwrap_or(0));
                self.decode_packet(data, duration, discard, sink)
            }
        } else {
            // The frames cut from the bytes: their lengths and times come
            // from their headers, not from the container.
            self.reframed = true;
            reframer.push(data);
            let mut frame = core::mem::take(&mut self.frame);
            let mut result = Ok(());
            while result.is_ok() && reframer.next(&mut frame) {
                if self.end.is_some_and(|end| self.position >= end) {
                    continue;
                }
                self.position = self
                    .position
                    .saturating_add(self.worker.frames_hint(&frame).unwrap_or(0));
                result = self.decode_packet(&frame, None, 0, sink);
            }
            self.frame = frame;
            result
        };
        self.reframer = Some(reframer);
        result
    }

    fn decode_packet(
        &mut self,
        data: &[u8],
        duration: Option<u64>,
        discard: u64,
        sink: &mut dyn Sink,
    ) -> Result<(), Error> {
        let Some(pool) = self.pool.as_mut() else {
            // The buffer keeps its length from packet to packet: `commit`
            // takes the packet's samples from its start by their count.
            let mut bytes = core::mem::take(&mut self.bytes);
            let result = self.worker.decode(data, &mut bytes, 0);
            let written = self.out.commit(result, &bytes, duration, discard, sink);
            self.bytes = bytes;
            return written;
        };
        let frames = self
            .worker
            .frames_hint(data)
            .or(duration)
            .unwrap_or(self.out.last_frames);
        let samples = frames.saturating_mul(u64::from(self.out.format.channels));
        let packet_cost = cost(data.len(), samples);
        if !pool.stereo && matches!(self.worker.decoder, Decoder::Mp3(_)) {
            pool.stereo = mp3::Header::parse(data).is_ok_and(|h| h.channels() == 2);
        }
        let mut buffer = pool.spare.pop().unwrap_or_default();
        pool.spare_bytes = pool.spare_bytes.saturating_sub(buffer.capacity());
        buffer.clear();
        buffer.extend_from_slice(data);
        pool.job.push(Pending {
            data: buffer,
            duration,
            discard,
        });
        pool.job_cost = pool.job_cost.saturating_add(packet_cost);
        if pool.job_cost >= JOB_BYTES || pool.job.len() >= JOB_PACKETS {
            submit(pool, &self.worker, &mut self.out, sink)?;
        }
        Ok(())
    }

    /// Writes what the pool still holds, then the summary.
    pub(crate) fn finish(
        mut self,
        stream_index: usize,
        sink: &mut dyn Sink,
    ) -> Result<Summary, Error> {
        // A last frame cut short in the reframer's buffer.
        if let Some(mut reframer) = self.reframer.take() {
            let mut frame = core::mem::take(&mut self.frame);
            if reframer.rest(&mut frame) && !self.end.is_some_and(|end| self.position >= end) {
                self.decode_packet(&frame, None, 0, sink)?;
            }
        }
        if let Some(mut pool) = self.pool.take() {
            if !pool.job.is_empty() {
                submit(&mut pool, &self.worker, &mut self.out, sink)?;
            }
            // No more jobs: the threads end once the queue is empty.
            pool.jobs = None;
            while pool.next_commit < pool.next_seq {
                receive(&mut pool, true)?;
                commit_ready(&mut pool, &mut self.out, sink)?;
            }
        }
        let mut out = self.out;
        out.close(sink)?;
        if out.concealed > 0 {
            out.warnings.push(Warning::Concealed {
                packets: out.concealed,
                first: out.first_error.take().unwrap_or_default(),
            });
        }
        if out.crc_failures > 0 {
            out.warnings.push(Warning::FrameCrc {
                frames: out.crc_failures,
            });
        }
        if out.trailing > 0 {
            out.warnings.push(Warning::TrailingBytes {
                packets: out.trailing,
            });
        }
        Ok(Summary {
            stream_index,
            stream: out.format,
            output: out.output,
            packets: out.packets,
            samples: out.written,
            timeline: Timeline {
                start: out.media_start.unwrap_or(out.written),
                silence: out.silence,
            },
            warnings: out.warnings,
        })
    }
}

/// Hands the current job to the threads, writes what is ready, and waits
/// while the pool holds more than [`IN_FLIGHT_BYTES`].
fn submit(
    pool: &mut Pool<'_>,
    worker: &Worker,
    out: &mut Output,
    sink: &mut dyn Sink,
) -> Result<(), Error> {
    let prime = core::mem::take(&mut pool.prime);
    let packets = core::mem::take(&mut pool.job);
    // The next job's priming: copies of the last packets so far, this
    // job's priming included when the job is short.
    let tail: Vec<&[u8]> = prime
        .iter()
        .chain(packets.iter())
        .map(|p| p.data.as_slice())
        .collect();
    let count = worker.priming(&tail, pool.stereo);
    let mut prime_cost = 0u64;
    for pending in prime.iter().chain(packets.iter()).skip(tail.len() - count) {
        let mut buffer = pool.spare.pop().unwrap_or_default();
        pool.spare_bytes = pool.spare_bytes.saturating_sub(buffer.capacity());
        buffer.clear();
        buffer.extend_from_slice(&pending.data);
        prime_cost = prime_cost.saturating_add(cost(buffer.len(), 0));
        pool.prime.push(Pending {
            data: buffer,
            duration: pending.duration,
            discard: pending.discard,
        });
    }
    let job = Job {
        seq: pool.next_seq,
        // The priming packets are held until the job is written.
        cost: pool.job_cost.saturating_add(pool.prime_cost),
        prime,
        packets,
    };
    pool.prime_cost = prime_cost;
    pool.job_cost = 0;
    pool.next_seq += 1;
    pool.in_flight = pool.in_flight.saturating_add(job.cost);
    let sent = pool
        .jobs
        .as_ref()
        .is_some_and(|jobs| jobs.send(job).is_ok());
    if !sent {
        return Err(Error::Invalid("the decoding threads stopped"));
    }
    while receive(pool, false)? {}
    commit_ready(pool, out, sink)?;
    while pool.in_flight > IN_FLIGHT_BYTES {
        receive(pool, true)?;
        commit_ready(pool, out, sink)?;
    }
    Ok(())
}

/// Takes one decoded job, waiting for it when `wait`; whether one came.
fn receive(pool: &mut Pool<'_>, wait: bool) -> Result<bool, Error> {
    let done = if wait {
        pool.done
            .recv()
            .map_err(|_| Error::Invalid("the decoding threads stopped"))?
    } else {
        match pool.done.try_recv() {
            Ok(done) => done,
            Err(mpsc::TryRecvError::Empty) => return Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(Error::Invalid("the decoding threads stopped"));
            }
        }
    };
    match done {
        Message::Done(done) => {
            pool.waiting.insert(done.seq, *done);
            Ok(true)
        }
        Message::Stopped => Err(Error::Invalid("a decoding thread stopped")),
    }
}

/// Writes the decoded jobs that are next in stream order.
fn commit_ready(pool: &mut Pool<'_>, out: &mut Output, sink: &mut dyn Sink) -> Result<(), Error> {
    while let Some(done) = pool.waiting.remove(&pool.next_commit) {
        pool.next_commit += 1;
        pool.in_flight = pool.in_flight.saturating_sub(done.cost);
        let mut start = 0;
        for ((result, end), pending) in done.results.into_iter().zip(done.packets) {
            let bytes = done.bytes.get(start..end).unwrap_or_default();
            start = end;
            out.commit(result, bytes, pending.duration, pending.discard, sink)?;
            spare(pool, pending.data);
        }
        for pending in done.prime {
            spare(pool, pending.data);
        }
    }
    Ok(())
}

/// Keeps a written packet's buffer for reuse, within the bounds.
fn spare(pool: &mut Pool<'_>, buffer: Vec<u8>) {
    let capacity = buffer.capacity();
    if capacity <= SPARE_BUFFER_BYTES && pool.spare_bytes + capacity <= SPARE_TOTAL_BYTES {
        pool.spare_bytes += capacity;
        pool.spare.push(buffer);
    }
}
