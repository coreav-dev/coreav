//! CoreAV's FFmpeg fallback (`docs/design/decode.md`, "The FFmpeg
//! fallback"): a [`Decoder`] that decodes packets with an `ffmpeg` binary
//! run as a child process. FFmpeg's libraries are never linked; its C code
//! never runs in the process that uses this crate. The crate is meant to
//! run inside CoreAV's sandboxed worker (`coreav-worker`), which applies
//! resource limits before it starts FFmpeg; it is compiled only with the
//! CLI's `ffmpeg-fallback` feature, never in a default build.
//!
//! CoreAV demuxes and FFmpeg only decodes: the packets reach FFmpeg as a
//! Matroska stream CoreAV writes ([`mkv`]), with the decoder chosen by
//! name, and frames come back as raw video in the decoder's own pixel
//! format, described frame by frame by FFmpeg's `showinfo` filter
//! ([`showinfo`]). Everything FFmpeg writes is untrusted and checked.

#![forbid(unsafe_code)]

pub mod mkv;
pub mod showinfo;

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use coreav_decode::{
    BackendKind, Codec, Decoder, DecoderInfo, Error, ErrorKind, Isolation, Limits, Packet,
    StreamConfig,
};
use coreav_frame::{ColorInfo, Frame, Picture, Pixels, Plane};

use showinfo::{FrameInfo, RawFormat, truncate};

/// How to run FFmpeg.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FfmpegOptions {
    /// The `ffmpeg` binary: an absolute path, since the worker runs with an
    /// empty environment.
    pub program: PathBuf,
    /// Decoder threads (`-threads`); `None` leaves FFmpeg's default.
    pub threads: Option<u32>,
}

/// FFmpeg's address space while it decodes a stream whose frames take
/// `frame` bytes, with `threads` threads and up to `references` reference
/// frames, as measured from outside (`docs/design/decode.md`, "Memory"):
/// about 1 GiB, 96 MiB per thread, and six frames plus two per thread, plus
/// the reference frames. An upper bound on every measurement.
fn ffmpeg_memory(frame: u64, threads: u64, references: u64) -> u64 {
    (1u64 << 30)
        .saturating_add(threads.saturating_mul(96 << 20))
        .saturating_add(
            frame.saturating_mul(
                threads
                    .saturating_mul(2)
                    .saturating_add(6)
                    .saturating_add(references),
            ),
        )
}

/// Bytes of one decoded frame of `stream` at its declared size: from the
/// bit depth and chroma format its configuration record gives (samples of
/// more than 8 bits take two bytes; ProRes 4444 may carry alpha), else 8
/// bytes per pixel, the largest format (4:4:4 with alpha at 16 bits).
fn decoded_frame_bytes(stream: &StreamConfig, width: u32, height: u32) -> u64 {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    let traits = stream.traits();
    let (Some(depth), Some(chroma)) = (traits.bit_depth, traits.chroma) else {
        return pixels.saturating_mul(8);
    };
    let sample = if depth > 8 { 2 } else { 1 };
    // Planes in eighths of the luma plane.
    let chroma_eighths = match chroma {
        coreav_frame::Chroma::Mono => 0,
        coreav_frame::Chroma::Cs420 => 4,
        coreav_frame::Chroma::Cs422 => 8,
        coreav_frame::Chroma::Cs444 => 16,
    };
    let alpha = if stream.codec == Codec::ProRes && chroma == coreav_frame::Chroma::Cs444 {
        8
    } else {
        0
    };
    pixels.saturating_mul(sample * (8 + chroma_eighths + alpha)) / 8
}

/// Whether a line of FFmpeg's says its resource limits refused it: an
/// allocation failed (ENOMEM), or a thread could not start (EAGAIN, which
/// is how the address-space limit stops thread stacks).
fn says_out_of_memory(line: &str) -> bool {
    line.contains("Cannot allocate memory") || line.contains("Resource temporarily unavailable")
}

/// Whether FFmpeg ended because its resource limits refused it: it exited
/// by itself with an error code (a signal is a crash, not a limit) after
/// saying so ([`says_out_of_memory`]) at any point.
fn status_is_memory(code: Option<i32>, said: bool) -> bool {
    code.is_some_and(|c| c != 0) && said
}

/// The most threads FFmpeg's own choice is taken to be.
const FFMPEG_OWN_THREADS: u64 = 16;

/// The threads FFmpeg gets within `limits.max_memory_bytes`: those asked
/// for (`None` or 0: FFmpeg's own choice, taken as one per CPU and one
/// more, at most [`FFMPEG_OWN_THREADS`]), only ever lowered, down to one,
/// until the estimate fits, and always given explicitly. A stream whose
/// frames and one thread do not fit is refused before FFmpeg starts; this
/// trusts the configuration record and the declared size, so it is
/// advisory, and the address-space limit is what holds. Without a
/// declared size, nothing is fitted and FFmpeg's own choice stands.
pub fn threads_within(
    requested: Option<u32>,
    stream: &StreamConfig,
    limits: &Limits,
) -> Result<Option<u32>, Error> {
    let requested = requested.filter(|t| *t > 0);
    let (Some(w), Some(h)) = (stream.width, stream.height) else {
        return Ok(requested);
    };
    let frame = decoded_frame_bytes(stream, w, h);
    // Intra-only ProRes keeps no reference frames.
    let references = if stream.codec == Codec::ProRes {
        0
    } else {
        u64::from(limits.max_reference_frames)
    };
    let need = |threads| ffmpeg_memory(frame, threads, references);
    let budget = limits.max_memory_bytes;
    // Refused before start only when the frames and one thread do not fit:
    // the fixed part of the estimate is mostly address space reserved, not
    // used, and a tight limit leaves one thread to FFmpeg.
    let frames = frame
        .saturating_mul(references.saturating_add(8))
        .saturating_add(96 << 20);
    if frames > budget {
        return Err(Error::new(
            ErrorKind::Limit,
            format!(
                "frames of {w}x{h} need about {} MiB to decode, over the memory limit of {} MiB",
                frames >> 20,
                budget >> 20
            ),
        ));
    }
    // FFmpeg's own choice: one thread per CPU and one more.
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let own = u64::try_from(cpus)
        .unwrap_or(1)
        .saturating_add(1)
        .clamp(1, FFMPEG_OWN_THREADS);
    let wanted = requested.map_or(own, u64::from);
    let mut fit = wanted;
    while fit > 1 && need(fit) > budget {
        fit -= 1;
    }
    // Given explicitly: FFmpeg's own count may not see a CPU quota that
    // this one does, and would then start more threads than were fitted.
    Ok(Some(u32::try_from(fit).unwrap_or(1)))
}

/// The FFmpeg decoder used for `codec`.
pub fn decoder_name(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Av1 => "libdav1d",
        Codec::Vp9 => "vp9",
        Codec::Vp8 => "vp8",
        Codec::ProRes => "prores",
        Codec::Mpeg4 => "mpeg4",
        Codec::Mpeg1Video => "mpeg1video",
        Codec::Mpeg2Video => "mpeg2video",
        Codec::Mjpeg => "mjpeg",
        Codec::Theora => "theora",
        Codec::Ffv1 => "ffv1",
    }
}

/// The fixed command line (`docs/design/decode.md`): `-max_alloc` bounds
/// FFmpeg's largest single allocation by the frame limit.
pub fn arguments(options: &FfmpegOptions, codec: Codec, limits: &Limits) -> Vec<String> {
    let max_alloc = limits
        .max_frame_bytes
        .saturating_add(16 << 20)
        .min(i64::MAX as u64);
    let mut args: Vec<String> = [
        "-nostdin",
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "info",
        // The filter graph (`showinfo`) and the output (raw video) only
        // pass frames on: one thread each, so that FFmpeg's memory does not
        // grow with the host's CPUs (`docs/design/decode.md`, "Memory").
        "-filter_threads",
        "1",
        "-max_alloc",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.push(max_alloc.to_string());
    if let Some(threads) = options.threads {
        args.push("-threads".into());
        args.push(threads.to_string());
    }
    if codec == Codec::H264 {
        // Without probing, FFmpeg's H.264 decoder learns the reorder depth
        // as frames arrive and drops a frame each time it grows it, unless
        // the sequence parameter set states it; `-strict 1` makes it assume
        // the largest depth instead (`docs/design/decode.md`).
        args.push("-strict".into());
        args.push("1".into());
    }
    for a in [
        // Cropping exactly as the bitstream says (clause 7.4.2.1.1 for
        // H.264, the conformance window for HEVC). By default FFmpeg 7
        // keeps the decoded planes aligned and skips a left crop, such as
        // 48 samples: its frame is then the uncropped coded picture, which
        // differs from the native backend's.
        "-flags",
        "unaligned",
        "-protocol_whitelist",
        "pipe",
        "-nofind_stream_info",
        "-copyts",
        "-f",
        "matroska",
        "-c:v",
    ] {
        args.push(a.into());
    }
    args.push(decoder_name(codec).into());
    for a in [
        "-i",
        "pipe:0",
        "-map",
        "0:v:0",
        "-threads",
        "1",
        "-fps_mode",
        "passthrough",
        // Frames keep their own size when the stream changes it: by
        // default FFmpeg scales every frame to the first one's.
        "-noautoscale",
        "-vf",
        "showinfo=checksum=0",
        "-flush_packets",
        "1",
        "-f",
        "rawvideo",
        "pipe:1",
    ] {
        args.push(a.into());
    }
    args
}

/// Frames decoded but not yet received: FFmpeg's output is not read further
/// until the caller receives one (backpressure, `docs/design/decode.md`).
const MAX_QUEUED_FRAMES: usize = 4;

/// Lines of FFmpeg's other messages kept for error reports.
const KEPT_LINES: usize = 12;
/// Bytes of one standard-error line read; the rest of a longer line (a
/// `showinfo` side-data dump can be as long as a packet) is skipped.
const MAX_LINE: usize = 2048;
/// Bytes of one kept line.
const KEPT_LINE: usize = 240;

/// A decoder running an `ffmpeg` child process.
pub struct FfmpegDecoder {
    info: DecoderInfo,
    child: Child,
    input: Option<Sender<Vec<u8>>>,
    /// Bytes sent to the feeder and not yet written to FFmpeg.
    waiting: Arc<AtomicU64>,
    feeder: Option<JoinHandle<()>>,
    frames: Receiver<Result<Frame, Error>>,
    reader: Option<JoinHandle<()>>,
    messages: Arc<Mutex<VecDeque<String>>>,
    /// The thread that reads FFmpeg's messages.
    message_reader: Option<JoinHandle<()>>,
    /// FFmpeg said its resource limits refused it ([`says_out_of_memory`]).
    out_of_memory: Arc<AtomicBool>,
    /// Pts of frames the caller does not want, with their counts.
    discard: Arc<Mutex<HashMap<i64, u32>>>,
    limits: Limits,
    deadline: Instant,
    eof: bool,
    done: bool,
    last_pts: Option<i64>,
}

impl FfmpegDecoder {
    /// Starts FFmpeg for `stream` and writes the Matroska header.
    pub fn open(
        options: &FfmpegOptions,
        stream: &StreamConfig,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let deadline = limits.deadline();
        let options = &FfmpegOptions {
            threads: threads_within(options.threads, stream, limits)?,
            ..options.clone()
        };
        let mut child = Command::new(&options.program)
            .args(arguments(options, stream.codec, limits))
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::new(
                    ErrorKind::Backend,
                    format!("cannot run {}: {e}", options.program.display()),
                )
            })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::new(ErrorKind::Backend, "ffmpeg pipes missing"));
        };
        let fail = |mut child: Child, e: Error| {
            let _ = child.kill();
            let _ = child.wait();
            Err(e)
        };
        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        let waiting = Arc::new(AtomicU64::new(0));
        let feeder = {
            let waiting = Arc::clone(&waiting);
            match spawn_thread("coreav-ffmpeg-input", move || {
                feed(stdin, &input_rx, &waiting)
            }) {
                Ok(feeder) => feeder,
                Err(e) => return fail(child, e),
            }
        };
        let header = mkv::header(stream);
        waiting.fetch_add(
            u64::try_from(header.len()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        if input.send(header).is_err() {
            return fail(child, Error::new(ErrorKind::Backend, "ffmpeg input closed"));
        }
        let messages = Arc::new(Mutex::new(VecDeque::new()));
        let out_of_memory = Arc::new(AtomicBool::new(false));
        let (info_tx, info_rx) = mpsc::channel();
        let message_reader = {
            let messages = Arc::clone(&messages);
            let out_of_memory = Arc::clone(&out_of_memory);
            let started = spawn_thread("coreav-ffmpeg-messages", move || {
                read_messages(stderr, &info_tx, &messages, &out_of_memory);
            });
            match started {
                Ok(handle) => handle,
                Err(e) => return fail(child, e),
            }
        };
        let (frame_tx, frames) = mpsc::sync_channel(MAX_QUEUED_FRAMES);
        let discard = Arc::new(Mutex::new(HashMap::new()));
        let reader = {
            let discard = Arc::clone(&discard);
            let limits = limits.clone();
            let color = stream.color;
            let started = spawn_thread("coreav-ffmpeg-frames", move || {
                let output = Output {
                    frames: &frame_tx,
                    discard: &discard,
                };
                read_frames(stdout, &info_rx, &output, &limits, color);
            });
            match started {
                Ok(reader) => reader,
                Err(e) => return fail(child, e),
            }
        };
        Ok(Self {
            info: DecoderInfo {
                backend: BackendKind::Fallback,
                isolation: Isolation::Sandboxed,
                decoder: format!("ffmpeg {}", decoder_name(stream.codec)),
            },
            child,
            input: Some(input),
            waiting,
            feeder: Some(feeder),
            frames,
            reader: Some(reader),
            messages,
            message_reader: Some(message_reader),
            out_of_memory,
            discard,
            limits: limits.clone(),
            deadline,
            eof: false,
            done: false,
            last_pts: None,
        })
    }

    /// FFmpeg's last messages, for an error report.
    fn tail(&self) -> String {
        self.messages
            .lock()
            .map(|m| m.iter().cloned().collect::<Vec<_>>().join(" | "))
            .unwrap_or_default()
    }

    fn failure(&mut self, what: &str) -> Error {
        let status = self.wait_exit(Duration::from_millis(200));
        if status.is_some() {
            self.settle_messages(Duration::from_millis(200));
        }
        let tail = self.tail();
        let status_code = status.and_then(|s| s.code());
        let status = status.map_or_else(|| "running".to_owned(), |s| s.to_string());
        let kind = if status_is_memory(status_code, self.out_of_memory.load(Ordering::Relaxed)) {
            ErrorKind::Limit
        } else {
            ErrorKind::Backend
        };
        Error::new(kind, format!("{what} (ffmpeg {status}): {tail}"))
    }

    fn wait_exit(&mut self, patience: Duration) -> Option<ExitStatus> {
        let until = Instant::now() + patience;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => return None,
            }
        }
    }

    /// The error of a failed frame read. A read that came up short is
    /// usually FFmpeg ending early: once it has exited, its own messages
    /// say why, and an allocation its memory limit refused is `Limit`.
    fn reader_failed(&mut self, error: Error) -> Error {
        let patience =
            Duration::from_millis(500).min(self.deadline.saturating_duration_since(Instant::now()));
        let code = if error.kind == ErrorKind::Backend {
            self.wait_exit(patience).and_then(|s| s.code())
        } else {
            None
        };
        // FFmpeg's last messages are read on their own thread, which ends
        // at the end of FFmpeg's standard error once FFmpeg has exited.
        if code.is_some() {
            self.settle_messages(patience);
        }
        let error = if status_is_memory(code, self.out_of_memory.load(Ordering::Relaxed)) {
            Error::new(
                ErrorKind::Limit,
                format!("ffmpeg ran into its resource limits: {}", self.tail()),
            )
        } else {
            error
        };
        self.kill();
        error
    }

    /// Waits up to `patience` for the message thread to read FFmpeg's last
    /// messages.
    fn settle_messages(&mut self, patience: Duration) {
        let until = Instant::now() + patience;
        while self
            .message_reader
            .as_ref()
            .is_some_and(|h| !h.is_finished())
            && Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn finish(&mut self) -> Result<Option<Frame>, Error> {
        self.done = true;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(feeder) = self.feeder.take() {
            let _ = feeder.join();
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        match self.wait_exit(remaining) {
            Some(status) if status.success() => Ok(None),
            Some(_) => Err(self.failure("decoding failed")),
            None => {
                self.kill();
                Err(Error::new(
                    ErrorKind::Timeout,
                    "ffmpeg did not exit in time",
                ))
            }
        }
    }
}

impl Decoder for FfmpegDecoder {
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        if self.eof {
            return Err(Error::new(ErrorKind::InvalidData, "packet after the end"));
        }
        self.limits.check_packet(packet)?;
        let pts = packet
            .pts
            .or(packet.dts)
            .or_else(|| self.last_pts.map(|p| p.saturating_add(1)))
            .unwrap_or(0);
        self.last_pts = Some(pts);
        if packet.discard {
            if let Ok(mut discard) = self.discard.lock() {
                *discard.entry(pts).or_insert(0) += 1;
            }
        }
        let timestamp = pts
            .checked_add(mkv::PTS_OFFSET)
            .and_then(|t| u64::try_from(t).ok())
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "pts out of range"))?;
        let cluster = mkv::cluster(timestamp, packet.keyframe, packet.data);
        // Packets FFmpeg has not read yet stay within the limit rather than
        // growing memory while it is slow or stalled.
        let bytes = u64::try_from(cluster.len()).unwrap_or(u64::MAX);
        let waiting = self
            .waiting
            .fetch_add(bytes, Ordering::Relaxed)
            .saturating_add(bytes);
        if waiting > self.limits.max_pending_bytes {
            self.kill();
            return Err(Error::new(
                ErrorKind::Limit,
                "packets waiting to be decoded exceed the limit",
            ));
        }
        let sent = self.input.as_ref().is_some_and(|i| i.send(cluster).is_ok());
        if !sent {
            return Err(self.failure("ffmpeg stopped reading"));
        }
        Ok(())
    }

    fn send_eof(&mut self) -> Result<(), Error> {
        self.eof = true;
        // Closing the channel ends the feeder once it has written what is
        // queued, which closes FFmpeg's input. It is not waited for here:
        // FFmpeg may need its output read first.
        self.input = None;
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        if self.done {
            return Ok(None);
        }
        if !self.eof {
            return match self.frames.try_recv() {
                Ok(Ok(frame)) => Ok(Some(frame)),
                Ok(Err(e)) => Err(self.reader_failed(e)),
                Err(TryRecvError::Empty) => {
                    if Instant::now() > self.deadline {
                        self.kill();
                        return Err(Error::new(ErrorKind::Timeout, "decoding took too long"));
                    }
                    Ok(None)
                }
                Err(TryRecvError::Disconnected) => {
                    self.done = true;
                    Err(self.failure("ffmpeg ended before the end of the packets"))
                }
            };
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        match self.frames.recv_timeout(remaining) {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(e)) => {
                self.done = true;
                Err(self.reader_failed(e))
            }
            Err(RecvTimeoutError::Timeout) => {
                self.done = true;
                self.kill();
                Err(Error::new(ErrorKind::Timeout, "decoding took too long"))
            }
            Err(RecvTimeoutError::Disconnected) => self.finish(),
        }
    }

    fn info(&self) -> &DecoderInfo {
        &self.info
    }
}

impl Drop for FfmpegDecoder {
    fn drop(&mut self) {
        self.input = None;
        if self.child.try_wait().ok().flatten().is_none() {
            self.kill();
        }
    }
}

/// Starts a thread, or reports why it could not.
fn spawn_thread(name: &str, f: impl FnOnce() + Send + 'static) -> Result<JoinHandle<()>, Error> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map_err(|e| Error::new(ErrorKind::Backend, format!("cannot start a thread: {e}")))
}

fn frame_bytes(frame: &Frame) -> u64 {
    match &frame.pixels {
        Pixels::Cpu(p) => u64::try_from(p.packed_len()).unwrap_or(u64::MAX),
        Pixels::Cuda(_) | Pixels::Shared(_) => 0,
    }
}

fn feed(mut stdin: std::process::ChildStdin, chunks: &Receiver<Vec<u8>>, waiting: &AtomicU64) {
    for chunk in chunks {
        if stdin.write_all(&chunk).is_err() {
            return;
        }
        let bytes = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        waiting.fetch_sub(bytes, Ordering::Relaxed);
    }
    let _ = stdin.flush();
}

/// Reads FFmpeg's standard error: frame lines go to `infos`, the last few
/// other lines are kept for error reports.
fn read_messages(
    mut stderr: impl Read,
    infos: &Sender<Result<FrameInfo, String>>,
    kept: &Mutex<VecDeque<String>>,
    out_of_memory: &AtomicBool,
) {
    let mut buf = [0u8; 8192];
    let mut line: Vec<u8> = Vec::with_capacity(MAX_LINE);
    let mut overflow = false;
    loop {
        let n = match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in buf.get(..n).unwrap_or(&[]) {
            if b == b'\n' || b == b'\r' {
                if !line.is_empty() {
                    handle_line(&line, overflow, infos, kept, out_of_memory);
                }
                line.clear();
                overflow = false;
            } else if line.len() < MAX_LINE {
                line.push(b);
            } else {
                overflow = true;
            }
        }
    }
    if !line.is_empty() {
        handle_line(&line, overflow, infos, kept, out_of_memory);
    }
}

fn handle_line(
    line: &[u8],
    overflow: bool,
    infos: &Sender<Result<FrameInfo, String>>,
    kept: &Mutex<VecDeque<String>>,
    out_of_memory: &AtomicBool,
) {
    let text = String::from_utf8_lossy(line);
    if says_out_of_memory(&text) {
        out_of_memory.store(true, Ordering::Relaxed);
    }
    if !overflow {
        if let Some(parsed) = showinfo::parse_line(&text) {
            let _ = infos.send(parsed);
            return;
        }
    }
    if text.starts_with("[Parsed_showinfo_0") {
        // Side data and colour lines of a frame.
        return;
    }
    if let Ok(mut kept) = kept.lock() {
        if kept.len() == KEPT_LINES {
            kept.pop_front();
        }
        kept.push_back(truncate(text.trim(), KEPT_LINE).to_owned());
    }
}

/// Where the frame reader sends frames.
struct Output<'a> {
    frames: &'a SyncSender<Result<Frame, Error>>,
    discard: &'a Mutex<HashMap<i64, u32>>,
}

impl Output<'_> {
    /// Whether the frame of `pts` (FFmpeg's number) is one the caller
    /// discards; counts it off.
    fn discards(&self, pts: i64) -> bool {
        let pts = pts.saturating_sub(mkv::PTS_OFFSET);
        let Ok(mut discard) = self.discard.lock() else {
            return false;
        };
        match discard.get_mut(&pts) {
            Some(count) if *count > 1 => {
                *count -= 1;
                true
            }
            Some(_) => {
                discard.remove(&pts);
                true
            }
            None => false,
        }
    }
}

/// Reads one raw frame per `showinfo` line from FFmpeg's standard output.
fn read_frames(
    mut stdout: ChildStdout,
    infos: &Receiver<Result<FrameInfo, String>>,
    output: &Output<'_>,
    limits: &Limits,
    color: ColorInfo,
) {
    let frames = output.frames;
    let mut expected = 0u64;
    let mut decoded = 0u64;
    let mut scratch = Vec::new();
    for info in infos {
        let result = info
            .map_err(|m| Error::new(ErrorKind::Backend, format!("ffmpeg: {m}")))
            .and_then(|info| {
                // Numbers count from 0 again when FFmpeg rebuilds its
                // filters, at a change of frame size or format.
                if info.n != expected && info.n != 0 {
                    return Err(Error::new(
                        ErrorKind::Backend,
                        "ffmpeg frame numbers out of order",
                    ));
                }
                expected = info.n + 1;
                if output.discards(info.pts) {
                    skip_frame(&mut stdout, &info, limits, &mut scratch)?;
                    return Ok(None);
                }
                let frame = read_frame(&mut stdout, &info, limits, color)?;
                let bytes = frame_bytes(&frame);
                decoded = decoded.saturating_add(bytes);
                if decoded > limits.max_decoded_bytes {
                    return Err(Error::new(
                        ErrorKind::Limit,
                        "decoded bytes exceed the limit",
                    ));
                }
                Ok(Some(frame))
            });
        let failed = result.is_err();
        let sent = match result.transpose() {
            None => true,
            Some(r) => frames.send(r).is_ok(),
        };
        if !sent || failed {
            return;
        }
    }
    // Every frame line has its frame; anything more is not described.
    let mut extra = [0u8; 1];
    if matches!(stdout.read(&mut extra), Ok(n) if n > 0) {
        let _ = frames.send(Err(Error::new(
            ErrorKind::Backend,
            "ffmpeg wrote frame data without a frame line",
        )));
    }
}

/// Reads a frame the caller discards into `scratch`, reused from frame to
/// frame.
fn skip_frame(
    stdout: &mut ChildStdout,
    info: &FrameInfo,
    limits: &Limits,
    scratch: &mut Vec<u8>,
) -> Result<(), Error> {
    let sizes = info
        .format
        .plane_bytes(info.width, info.height)
        .ok_or_else(|| Error::new(ErrorKind::Backend, "frame size overflows"))?;
    let total = sizes
        .iter()
        .try_fold(0usize, |sum, s| sum.checked_add(*s))
        .ok_or_else(|| Error::new(ErrorKind::Backend, "frame size overflows"))?;
    limits.check_frame(
        info.width,
        info.height,
        u64::try_from(total).unwrap_or(u64::MAX),
    )?;
    if scratch.len() < total {
        scratch
            .try_reserve(total - scratch.len())
            .map_err(|_| Error::new(ErrorKind::Limit, "out of memory for a frame"))?;
        scratch.resize(total, 0);
    }
    let target = scratch
        .get_mut(..total)
        .ok_or_else(|| Error::new(ErrorKind::Backend, "scratch"))?;
    stdout
        .read_exact(target)
        .map_err(|e| Error::new(ErrorKind::Backend, format!("short frame from ffmpeg: {e}")))
}

fn read_frame(
    stdout: &mut ChildStdout,
    info: &FrameInfo,
    limits: &Limits,
    color: ColorInfo,
) -> Result<Frame, Error> {
    let raw: RawFormat = info.format;
    let sizes = raw
        .plane_bytes(info.width, info.height)
        .ok_or_else(|| Error::new(ErrorKind::Backend, "frame size overflows"))?;
    let total = sizes
        .iter()
        .try_fold(0u64, |sum, s| sum.checked_add(u64::try_from(*s).ok()?))
        .ok_or_else(|| Error::new(ErrorKind::Backend, "frame size overflows"))?;
    limits.check_frame(info.width, info.height, total)?;
    let mut planes = Vec::with_capacity(sizes.len());
    for (index, size) in sizes.iter().enumerate() {
        let mut data = Vec::new();
        data.try_reserve_exact(*size)
            .map_err(|_| Error::new(ErrorKind::Limit, "out of memory for a frame"))?;
        data.resize(*size, 0);
        stdout
            .read_exact(&mut data)
            .map_err(|e| Error::new(ErrorKind::Backend, format!("short frame from ffmpeg: {e}")))?;
        if index < raw.format.plane_count() {
            let stride = raw
                .format
                .row_bytes(index, info.width, info.height)
                .ok_or_else(|| Error::new(ErrorKind::Backend, "bad row size"))?;
            planes.push(Plane::new(data, stride));
        }
    }
    let picture = Picture::new(info.width, info.height, raw.format, planes)?;
    let mut color = color;
    if raw.full_range {
        color.full_range = Some(true);
    }
    color.chroma_location = info.chroma_location.or(color.chroma_location);
    Ok(Frame {
        pts: Some(info.pts.saturating_sub(mkv::PTS_OFFSET)),
        color,
        pixels: Pixels::Cpu(picture),
    })
}

/// Reads a whole stream from standard input and returns nothing: lets a
/// test drive [`read_messages`] without a process.
#[doc(hidden)]
pub fn parse_messages_for_test(input: &[u8]) -> (Vec<Result<FrameInfo, String>>, Vec<String>) {
    let (tx, rx) = mpsc::channel();
    let kept = Mutex::new(VecDeque::new());
    read_messages(io::Cursor::new(input), &tx, &kept, &AtomicBool::new(false));
    drop(tx);
    let infos = rx.into_iter().collect();
    let kept = kept.into_inner().map(Vec::from).unwrap_or_default();
    (infos, kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_fit_the_memory_budget() {
        let sized = |codec, config: Option<&[u8]>, w, h| StreamConfig {
            config: config.map(|d| coreav_decode::CodecConfig {
                kind: coreav_decode::ConfigKind::ProResFourcc,
                data: d.to_vec(),
            }),
            width: Some(w),
            height: Some(h),
            ..StreamConfig::new(codec)
        };
        let limits = Limits::default();
        let prores = |w, h| sized(Codec::ProRes, Some(b"ap4h"), w, h);
        // Small frames: the request stands, or FFmpeg's own choice (one
        // thread per CPU and one more, at most 16), given explicitly; 0 is
        // FFmpeg's own choice.
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        let own = Some(u32::try_from(cpus + 1).unwrap_or(16).min(16));
        assert_eq!(
            threads_within(None, &prores(1920, 1080), &limits).ok(),
            Some(own)
        );
        assert_eq!(
            threads_within(Some(0), &prores(1920, 1080), &limits).ok(),
            Some(own)
        );
        // A tight budget leaves one thread rather than refusing.
        let tight = Limits {
            max_memory_bytes: 1 << 30,
            ..Limits::default()
        };
        assert_eq!(
            threads_within(None, &prores(1920, 1080), &tight).ok(),
            Some(Some(1))
        );
        assert_eq!(
            threads_within(Some(3), &prores(1920, 1080), &limits).ok(),
            Some(Some(3))
        );
        // 8809x6314 ProRes 4444 (445 MB a frame) within 8 GiB: lowered to 4,
        // never raised.
        assert_eq!(
            threads_within(Some(16), &prores(8809, 6314), &limits).ok(),
            Some(Some(4))
        );
        assert_eq!(
            threads_within(Some(2), &prores(8809, 6314), &limits).ok(),
            Some(Some(2))
        );
        let frame = decoded_frame_bytes(&prores(8809, 6314), 8809, 6314);
        assert_eq!(frame, 8809 * 6314 * 8);
        assert!(ffmpeg_memory(frame, 4, 0) <= limits.max_memory_bytes);
        assert!(ffmpeg_memory(frame, 5, 0) > limits.max_memory_bytes);
        // 16384x8192 ProRes 4444 (2^27 pixels): its frames alone do not
        // fit, refused...
        let refused = threads_within(None, &prores(16_384, 8_192), &limits);
        assert_eq!(refused.err().map(|e| e.kind), Some(ErrorKind::Limit));
        // ... nor at an unknown format (8 bytes a pixel) with references;
        // 8-bit 4:2:0 H.264 of that size does.
        let unknown = sized(Codec::H264, None, 11_000, 11_000);
        assert!(threads_within(None, &unknown, &limits).is_err());
        let avc = StreamConfig {
            config: Some(coreav_decode::CodecConfig {
                kind: coreav_decode::ConfigKind::Avcc,
                data: vec![1, 77, 0, 40, 0xff, 0xe0, 0],
            }),
            ..unknown
        };
        assert!(threads_within(None, &avc, &limits).is_ok());
        // Without a declared size, nothing to fit to.
        let unsized_stream = StreamConfig::new(Codec::ProRes);
        assert_eq!(
            threads_within(Some(2), &unsized_stream, &limits).ok(),
            Some(Some(2))
        );
    }

    #[test]
    fn memory_failures_are_limits_only_on_a_clean_exit() {
        assert!(says_out_of_memory(
            "Task finished with error code: -12 (Cannot allocate memory)"
        ));
        assert!(says_out_of_memory(
            "pthread_create() failed: Resource temporarily unavailable"
        ));
        assert!(!says_out_of_memory("[h264] get_buffer() failed"));
        assert!(status_is_memory(Some(244), true));
        // A crash (a signal: no exit code) is not a limit, whatever it said.
        assert!(!status_is_memory(None, true));
        assert!(!status_is_memory(Some(1), false));
    }

    #[test]
    fn long_lines_are_cut_and_frame_lines_found() {
        let mut input = Vec::new();
        input.extend_from_slice(b"Input #0, matroska\n");
        input.extend_from_slice(b"[Parsed_showinfo_0 @ 0x1] n:   0 pts: 5 fmt:yuv420p s:4x2\n");
        input.extend_from_slice(b"[Parsed_showinfo_0 @ 0x1] User Data=");
        input.extend(std::iter::repeat_n(b'a', 100_000));
        input.extend_from_slice(b"\r\nerror: something\n");
        let (infos, kept) = parse_messages_for_test(&input);
        assert_eq!(infos.len(), 1);
        assert_eq!(kept, vec!["Input #0, matroska", "error: something"]);
    }

    #[test]
    fn arguments_name_the_decoder() {
        let options = FfmpegOptions {
            program: "/usr/bin/ffmpeg".into(),
            threads: Some(2),
        };
        let args = arguments(&options, Codec::Hevc, &Limits::default());
        let joined = args.join(" ");
        assert!(joined.contains("-f matroska -c:v hevc -i pipe:0"));
        assert!(joined.contains("-threads 2"));
        assert!(joined.ends_with("-f rawvideo pipe:1"));
    }
}
