//! The worker process: reads the parent's messages, hosts one decoder
//! backend, and writes its frames into shared memory slots (or inline),
//! one descriptor per frame (`docs/design/decode.md`, "The sandboxed
//! worker").

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

use coreav_decode::{Decoder, Error, ErrorKind, Packet};
use coreav_frame::Frame;

#[cfg(target_os = "linux")]
use crate::protocol::Transfer;
use crate::protocol::{
    FrameDescriptor, INLINE, Incoming, Open, ParentMessage, PlaneDescriptor, ProtocolError,
    WorkerBackend, WorkerMessage, format_code, read_message,
};
use crate::sandbox;

/// Largest `Open` message.
const MAX_OPEN: usize = 2 << 20;

/// How long the worker waits for a message before it hands on the frames
/// its backend has ready (`docs/design/decode.md`, "Threads and
/// backpressure").
const IDLE_DRAIN: Duration = Duration::from_millis(2);

/// Runs the worker on standard input and output until the parent closes
/// the session. Call it from a program's `main` when the parent started it
/// as a worker (the `coreav` binary does with a hidden argument; the
/// `coreav-worker` binary always does). `allow` says whether this program
/// hosts a backend: an `Open` of one it refuses gets an error with its
/// message, before the worker does anything else.
pub fn worker_main(allow: impl Fn(WorkerBackend) -> Result<(), String>) -> ExitCode {
    let mut out = io::BufWriter::new(io::stdout());
    match run(&mut out, &allow) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = WorkerMessage::Error(e.kind, e.message).write(&mut out);
            ExitCode::from(1)
        }
    }
}

enum Command {
    Packet {
        pts: Option<i64>,
        dts: Option<i64>,
        keyframe: bool,
        discard: bool,
        data: Vec<u8>,
    },
    Eof,
    Close,
    /// The packets waiting to be decoded passed the limit.
    Overflow,
    /// A message from the parent did not fit the memory limit.
    OutOfMemory,
}

fn run(
    out: &mut impl Write,
    allow: &impl Fn(WorkerBackend) -> Result<(), String>,
) -> Result<(), Error> {
    let mut stdin = io::stdin();
    let mut open = match read_message(&mut stdin, |_| MAX_OPEN) {
        Ok(Incoming::Message(kind, payload)) => match ParentMessage::decode(kind, &payload) {
            Ok(ParentMessage::Open(open)) => open,
            _ => return Err(Error::new(ErrorKind::Backend, "expected Open")),
        },
        _ => return Err(Error::new(ErrorKind::Backend, "expected Open")),
    };
    allow(open.backend).map_err(|message| Error::new(ErrorKind::Unsupported, message))?;
    sandbox::apply(&open.sandbox)
        .map_err(|e| Error::new(ErrorKind::Backend, format!("cannot limit the worker: {e}")))?;
    // The backend fits itself to the address-space limit in force, which an
    // inherited lower limit can make smaller than the budget asked for.
    if let Some(space) = sandbox::address_space() {
        open.limits.max_memory_bytes = open.limits.max_memory_bytes.min(space);
    }
    let mut output = Output {
        #[cfg(target_os = "linux")]
        shared: Shared::open(&open)?,
        acks: None,
        reader_out_of_memory: Arc::new(AtomicBool::new(false)),
        seq: 0,
    };
    let mut decoder = make_decoder(&open)?;
    WorkerMessage::Ready(decoder.info().decoder.clone())
        .write(out)
        .map_err(io_error)?;
    let waiting = Arc::new(AtomicU64::new(0));
    let out_of_memory = Arc::new(AtomicBool::new(false));
    let (commands, acks) = spawn_reader(
        stdin,
        &open.limits,
        Arc::clone(&waiting),
        Arc::clone(&out_of_memory),
    )?;
    output.acks = Some(acks);
    output.reader_out_of_memory = out_of_memory;
    let mut ended = false;
    loop {
        // A backend that decodes on threads of its own (FFmpeg) has frames
        // ready while no packet comes: a parent that waits for frames
        // before it sends more (the transcode's pacing) gets them. After
        // the end of input every frame has gone, so the worker just waits.
        let received = if ended {
            commands.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            commands.recv_timeout(IDLE_DRAIN)
        };
        let command = match received {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => {
                drain(decoder.as_mut(), &mut output, out)?;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match command {
            Command::Packet {
                pts,
                dts,
                keyframe,
                discard,
                data,
            } => {
                decoder.send_packet(&Packet {
                    data: &data,
                    pts,
                    dts,
                    keyframe,
                    discard,
                })?;
                waiting.fetch_sub(
                    u64::try_from(data.len()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                drain(decoder.as_mut(), &mut output, out)?;
            }
            Command::Eof => {
                decoder.send_eof()?;
                drain(decoder.as_mut(), &mut output, out)?;
                WorkerMessage::Done.write(out).map_err(io_error)?;
                ended = true;
            }
            Command::Close => break,
            Command::Overflow => {
                return Err(Error::new(
                    ErrorKind::Limit,
                    "packets waiting to be decoded exceed the limit",
                ));
            }
            Command::OutOfMemory => {
                return Err(Error::new(
                    ErrorKind::Limit,
                    "a packet does not fit the memory limit",
                ));
            }
        }
    }
    Ok(())
}

fn io_error(e: io::Error) -> Error {
    Error::new(ErrorKind::Io, format!("writing to the parent: {e}"))
}

/// Reads the parent's messages on a thread of its own, so that waiting for
/// a free slot never stops the worker from seeing acknowledgements. The
/// packets it has read and the decoder has not yet taken (`waiting`, in
/// bytes) stay within `Limits::max_pending_bytes`: past it, the reader
/// queues `Overflow`, which fails the worker with `Limit`, and drops every
/// later packet while it keeps passing acknowledgements on, so that the
/// worker reaches the `Overflow`.
fn spawn_reader(
    mut stdin: io::Stdin,
    limits: &coreav_decode::Limits,
    waiting: Arc<AtomicU64>,
    out_of_memory: Arc<AtomicBool>,
) -> Result<(Receiver<Command>, Receiver<u32>), Error> {
    let (command_tx, command_rx) = mpsc::channel();
    let (ack_tx, ack_rx) = mpsc::channel();
    let max = usize::try_from(limits.max_packet_bytes)
        .unwrap_or(usize::MAX)
        .saturating_add(64);
    let max_waiting = limits.max_pending_bytes;
    let read = move || {
        let mut overflowed = false;
        loop {
            let (kind, payload) = match read_message(&mut stdin, |_| max) {
                Ok(Incoming::Message(kind, payload)) => (kind, payload),
                Err(ProtocolError(what)) if what.contains("out of memory") => {
                    // Flagged before the acknowledgement channel closes,
                    // so a wait for a slot ends with this, not with "parent
                    // gone".
                    out_of_memory.store(true, Ordering::SeqCst);
                    let _ = command_tx.send(Command::OutOfMemory);
                    return;
                }
                _ => break,
            };
            let message = ParentMessage::decode(kind, &payload);
            let command = match message {
                Ok(ParentMessage::Ack(slot)) => {
                    let _ = ack_tx.send(slot);
                    continue;
                }
                Ok(ParentMessage::Packet {
                    pts,
                    dts,
                    keyframe,
                    discard,
                    data,
                }) => {
                    if overflowed {
                        continue;
                    }
                    let bytes = u64::try_from(data.len()).unwrap_or(u64::MAX);
                    let total = waiting
                        .fetch_add(bytes, Ordering::Relaxed)
                        .saturating_add(bytes);
                    if total > max_waiting {
                        overflowed = true;
                        Command::Overflow
                    } else {
                        Command::Packet {
                            pts,
                            dts,
                            keyframe,
                            discard,
                            data,
                        }
                    }
                }
                Ok(ParentMessage::Eof) if overflowed => continue,
                Ok(ParentMessage::Eof) => Command::Eof,
                _ => break,
            };
            if command_tx.send(command).is_err() {
                return;
            }
        }
        let _ = command_tx.send(Command::Close);
    };
    std::thread::Builder::new()
        .name("coreav-worker-input".into())
        .spawn(read)
        .map_err(|e| Error::new(ErrorKind::Backend, format!("cannot start a thread: {e}")))?;
    Ok((command_rx, ack_rx))
}

fn drain(
    decoder: &mut dyn Decoder,
    output: &mut Output,
    out: &mut impl Write,
) -> Result<(), Error> {
    while let Some(frame) = decoder.receive_frame()? {
        output.emit(&frame, out)?;
    }
    Ok(())
}

/// Where frames go: slots of the shared memory, or inline.
struct Output {
    /// `None` sends every frame inline.
    #[cfg(target_os = "linux")]
    shared: Option<Shared>,
    acks: Option<Receiver<u32>>,
    /// The input reader stopped because a message did not fit the memory
    /// limit.
    reader_out_of_memory: Arc<AtomicBool>,
    seq: u64,
}

/// The parent's memory, mapped through `/proc`, and which of its slots are
/// free.
#[cfg(target_os = "linux")]
struct Shared {
    memory: crate::shm::WorkerMemory,
    slots: u32,
    slot_size: u64,
    free: VecDeque<u32>,
    deadline: Instant,
}

#[cfg(target_os = "linux")]
impl Shared {
    /// The slots `open` offers, or `None` for a pipe transfer and where
    /// the memory cannot be mapped (a sandbox without `/proc`): every frame
    /// then goes inline.
    fn open(open: &Open) -> Result<Option<Self>, Error> {
        let deadline = open.limits.deadline();
        let Transfer::Shared {
            path,
            slots,
            slot_size,
        } = &open.transfer
        else {
            return Ok(None);
        };
        let len = u64::from(*slots)
            .checked_mul(*slot_size)
            .ok_or_else(|| Error::new(ErrorKind::Backend, "memory size overflows"))?;
        let Ok(memory) = crate::shm::WorkerMemory::open(path, len) else {
            return Ok(None);
        };
        Ok(Some(Self {
            memory,
            slots: *slots,
            slot_size: *slot_size,
            free: (0..*slots).collect(),
            deadline,
        }))
    }
}

impl Output {
    /// A free slot, waiting for the parent's acknowledgement if none is.
    #[cfg(target_os = "linux")]
    fn take_slot(&mut self) -> Result<u32, Error> {
        let shared = self
            .shared
            .as_mut()
            .ok_or_else(|| Error::new(ErrorKind::Backend, "no memory"))?;
        loop {
            if let Some(slot) = shared.free.pop_front() {
                return Ok(slot);
            }
            let acks = self
                .acks
                .as_ref()
                .ok_or_else(|| Error::new(ErrorKind::Backend, "no acknowledgements"))?;
            let remaining = shared.deadline.saturating_duration_since(Instant::now());
            match acks.recv_timeout(remaining) {
                Ok(slot) if slot < shared.slots && !shared.free.contains(&slot) => {
                    shared.free.push_back(slot);
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err(Error::new(ErrorKind::Timeout, "no free frame slot"));
                }
                Err(RecvTimeoutError::Disconnected)
                    if self.reader_out_of_memory.load(Ordering::SeqCst) =>
                {
                    return Err(Error::new(
                        ErrorKind::Limit,
                        "a packet does not fit the memory limit",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::new(ErrorKind::Backend, "parent gone"));
                }
            }
        }
    }

    fn emit(&mut self, frame: &Frame, out: &mut impl Write) -> Result<(), Error> {
        let picture = frame.to_cpu()?;
        let format = picture.format();
        let mut planes = Vec::new();
        let mut offset = 0u64;
        for plane in 0..format.plane_count() {
            let (rows, row_bytes) = picture
                .plane_rows(plane)
                .ok_or_else(|| Error::new(ErrorKind::Backend, "frame without a plane"))?;
            let len = u64::try_from(rows.saturating_mul(row_bytes)).unwrap_or(u64::MAX);
            planes.push(PlaneDescriptor {
                offset,
                stride: u64::try_from(row_bytes).unwrap_or(u64::MAX),
                len,
            });
            offset = offset.saturating_add(len);
        }
        #[cfg(target_os = "linux")]
        let total = offset;
        let descriptor = FrameDescriptor {
            seq: self.seq,
            slot: INLINE,
            pts: frame.pts,
            width: picture.width(),
            height: picture.height(),
            format: format_code(format),
            color: frame.color,
            planes,
        };
        self.seq += 1;
        #[cfg(target_os = "linux")]
        if self
            .shared
            .as_ref()
            .is_some_and(|shared| total <= shared.slot_size)
        {
            let slot = self.take_slot()?;
            let shared = self
                .shared
                .as_mut()
                .ok_or_else(|| Error::new(ErrorKind::Backend, "no memory"))?;
            let base = u64::from(slot) * shared.slot_size;
            let start =
                usize::try_from(base).map_err(|_| Error::new(ErrorKind::Backend, "slot offset"))?;
            let len =
                usize::try_from(total).map_err(|_| Error::new(ErrorKind::Backend, "frame size"))?;
            let target = shared
                .memory
                .bytes_mut()
                .get_mut(start..start + len)
                .ok_or_else(|| Error::new(ErrorKind::Backend, "slot outside the memory"))?;
            let mut written = 0;
            for plane in 0..format.plane_count() {
                for row in picture.rows(plane) {
                    if let Some(dst) = target.get_mut(written..written + row.len()) {
                        dst.copy_from_slice(row);
                    }
                    written += row.len();
                }
            }
            return WorkerMessage::Frame(FrameDescriptor { slot, ..descriptor }, Vec::new())
                .write(out)
                .map_err(io_error);
        }
        let mut data = Vec::new();
        picture.write_packed(&mut data)?;
        WorkerMessage::Frame(descriptor, data)
            .write(out)
            .map_err(io_error)
    }
}

fn make_decoder(open: &Open) -> Result<Box<dyn Decoder>, Error> {
    match open.backend {
        WorkerBackend::Ffmpeg => ffmpeg_decoder(open),
        WorkerBackend::Dav1d => dav1d_decoder(open),
        WorkerBackend::Libvpx => libvpx_decoder(open),
    }
}

#[cfg(feature = "ffmpeg")]
fn ffmpeg_decoder(open: &Open) -> Result<Box<dyn Decoder>, Error> {
    use coreav_backend_ffmpeg::{FfmpegDecoder, FfmpegOptions};
    let program = open
        .ffmpeg
        .clone()
        .ok_or_else(|| Error::new(ErrorKind::Unsupported, "no ffmpeg binary given"))?;
    let options = FfmpegOptions {
        program: program.into(),
        threads: (open.threads > 0).then_some(open.threads),
    };
    Ok(Box::new(FfmpegDecoder::open(
        &options,
        &open.stream,
        &open.limits,
    )?))
}

#[cfg(not(feature = "ffmpeg"))]
fn ffmpeg_decoder(_: &Open) -> Result<Box<dyn Decoder>, Error> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "this worker was built without the FFmpeg fallback",
    ))
}

#[cfg(feature = "dav1d")]
fn dav1d_decoder(open: &Open) -> Result<Box<dyn Decoder>, Error> {
    use coreav_backend_dav1d::{Dav1dDecoder, Dav1dOptions};
    let options = Dav1dOptions {
        threads: open.threads,
    };
    Ok(Box::new(Dav1dDecoder::open(
        &options,
        &open.stream,
        &open.limits,
    )?))
}

#[cfg(not(feature = "dav1d"))]
fn dav1d_decoder(_: &Open) -> Result<Box<dyn Decoder>, Error> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "this worker was built without dav1d",
    ))
}

#[cfg(feature = "libvpx")]
fn libvpx_decoder(open: &Open) -> Result<Box<dyn Decoder>, Error> {
    use coreav_backend_libvpx::{LibvpxDecoder, LibvpxOptions};
    let options = LibvpxOptions {
        threads: open.threads,
    };
    Ok(Box::new(LibvpxDecoder::open(
        &options,
        &open.stream,
        &open.limits,
    )?))
}

#[cfg(not(feature = "libvpx"))]
fn libvpx_decoder(_: &Open) -> Result<Box<dyn Decoder>, Error> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "this worker was built without libvpx",
    ))
}
