//! The parent side: starts a worker, sends it packets, and validates every
//! frame it returns, then copies it out or lends it in its slot
//! (`docs/design/decode.md`, "The sandboxed worker"). The worker is
//! untrusted and possibly compromised.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use coreav_decode::{
    Backend, BackendKind, Decoder, DecoderInfo, Error, ErrorKind, Isolation, Limits, Packet,
    StreamConfig, StreamTraits, Support,
};
use coreav_frame::{ColorInfo, Frame, Pixels, SharedMemory, SharedPicture};

use crate::descriptor::{Checked, SlotMemory, Slots, accept, check};
use crate::protocol::{
    INLINE, Incoming, Open, ParentMessage, SandboxLimits, Transfer, WorkerBackend, WorkerMessage,
    read_message,
};

/// How to start a worker: a program and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerCommand {
    /// The program.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
}

impl WorkerCommand {
    /// This program, run again with `arg`: for a program whose `main`
    /// calls [`crate::worker_main`] when it sees `arg`, as `coreav` does.
    pub fn current_exe(arg: &str) -> io::Result<Self> {
        Ok(Self {
            program: std::env::current_exe()?,
            args: vec![arg.into()],
        })
    }

    /// The `coreav-worker` binary next to this program, else on `PATH`.
    pub fn installed() -> Option<Self> {
        let name = "coreav-worker";
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.join(name)))
            .filter(|p| p.is_file());
        sibling.or_else(|| find_on_path(name)).map(|program| Self {
            program,
            args: Vec::new(),
        })
    }
}

/// `name` in a directory of `PATH`.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// The `ffmpeg` binary for the fallback: `COREAV_FFMPEG` when it names a
/// file, else `ffmpeg` on `PATH`.
pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("COREAV_FFMPEG") {
        let path = PathBuf::from(named);
        return path.is_file().then_some(path);
    }
    find_on_path("ffmpeg")
}

/// How frames travel from the worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferMode {
    /// Shared memory slots where the platform has them (Linux), the pipe
    /// otherwise and for frames larger than a slot.
    #[default]
    Shared,
    /// Every frame through the pipe: the differential reference.
    Pipe,
}

/// How to run workers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerOptions {
    /// The worker program.
    pub command: WorkerCommand,
    /// Frame transfer.
    pub transfer: TransferMode,
    /// Shared memory slots.
    pub slots: u32,
    /// Hand each frame that comes in a slot to the consumer in place
    /// ([`Pixels::Shared`]) instead of copying it out: the slot stays the
    /// consumer's until the frame and its clones drop, so a consumer that
    /// keeps `slots` frames stops the worker until its deadline. For a
    /// consumer that passes each frame on and drops it (the transcode).
    pub lend_frames: bool,
    /// A cap on the worker's address space in bytes: the limit in force is
    /// the smaller of it and the decoder's `Limits::max_memory_bytes`
    /// (`None`: the budget alone).
    pub address_space: Option<u64>,
    /// The worker's open file limit.
    pub open_files: Option<u64>,
    /// Decoder threads; 0 for the backend's default.
    pub threads: u32,
    /// The `ffmpeg` binary, for the fallback.
    pub ffmpeg: Option<PathBuf>,
}

impl WorkerOptions {
    /// Defaults for `command`: shared memory with 2 slots, frames copied
    /// out, address space capped at 16 GiB (the decoder's memory budget
    /// applies below it), 256 open files, the backend's threads, and the
    /// `ffmpeg` that [`find_ffmpeg`] finds.
    pub fn new(command: WorkerCommand) -> Self {
        Self {
            command,
            transfer: TransferMode::Shared,
            slots: 2,
            lend_frames: false,
            address_space: Some(16 << 30),
            open_files: Some(256),
            threads: 0,
            ffmpeg: find_ffmpeg(),
        }
    }
}

/// A backend that runs in a worker: the FFmpeg fallback, or dav1d or
/// libvpx sandboxed.
#[derive(Clone, Debug)]
pub struct SandboxedBackend {
    /// The backend the worker hosts.
    pub backend: WorkerBackend,
    /// How to run the worker.
    pub options: WorkerOptions,
}

impl Backend for SandboxedBackend {
    fn kind(&self) -> BackendKind {
        match self.backend {
            WorkerBackend::Ffmpeg => BackendKind::Fallback,
            WorkerBackend::Dav1d => BackendKind::Dav1d,
            WorkerBackend::Libvpx => BackendKind::Libvpx,
        }
    }

    fn supports(&self, stream: &StreamConfig, _traits: &StreamTraits) -> Support {
        match self.backend {
            WorkerBackend::Ffmpeg if self.options.ffmpeg.is_none() => {
                Support::No("no ffmpeg binary found (set COREAV_FFMPEG)".into())
            }
            WorkerBackend::Ffmpeg => Support::Yes,
            WorkerBackend::Dav1d if stream.codec == coreav_decode::Codec::Av1 => Support::Yes,
            WorkerBackend::Dav1d => Support::No("dav1d decodes AV1 only".into()),
            WorkerBackend::Libvpx
                if matches!(
                    stream.codec,
                    coreav_decode::Codec::Vp8 | coreav_decode::Codec::Vp9
                ) =>
            {
                Support::Yes
            }
            WorkerBackend::Libvpx => Support::No("libvpx here decodes VP8 and VP9 only".into()),
        }
    }

    fn open(&self, stream: &StreamConfig, limits: &Limits) -> Result<Box<dyn Decoder>, Error> {
        Ok(Box::new(SandboxedDecoder::open(
            &self.options,
            self.backend,
            stream,
            limits,
        )?))
    }
}

enum Event {
    Ready(String),
    Frame(Frame, u64),
    Done,
    Failed(Error),
    /// The worker's output ended without a final message.
    Ended,
}

/// One write to the worker's input, and where to report how it went.
struct WriteJob {
    bytes: Vec<u8>,
    done: Option<Sender<io::Result<()>>>,
    /// The slot this write acknowledges, handed back to the worker just
    /// before the write.
    release: Option<u32>,
}

/// Which shared-memory slots the parent holds: from the frame the worker
/// put in one until the parent's acknowledgement of it is written. A frame
/// in a slot the parent holds is a protocol violation, so acknowledgements
/// waiting for the writer never outnumber the slots, even from a worker
/// that sends frames and never reads its input.
type HeldSlots = Arc<Vec<AtomicBool>>;

/// Writes to the worker's input on a thread of its own, so that no caller
/// waits on the pipe past its deadline: a worker that stops reading, even
/// one that left its process group and so outlives the keeper's kill,
/// holds only this thread. Ends at the first failed write, or once every
/// sender is gone, which closes the worker's input.
fn write_worker(mut stdin: ChildStdin, jobs: &Receiver<WriteJob>, held: &HeldSlots) {
    for job in jobs {
        // Before the write, not after: the worker may reuse the slot as soon
        // as it reads the acknowledgement.
        if let Some(flag) = job.release.and_then(|s| held.get(usize::try_from(s).ok()?)) {
            flag.store(false, Ordering::SeqCst);
        }
        let result = stdin.write_all(&job.bytes).and_then(|()| stdin.flush());
        let failed = result.is_err();
        if let Some(done) = job.done {
            let _ = done.send(result);
        }
        if failed {
            return;
        }
    }
}

/// How long a failed write waits for the worker's own report of why.
const REPORT_PATIENCE: Duration = Duration::from_secs(1);

/// A decoder whose backend runs in a worker process.
pub struct SandboxedDecoder {
    /// Orders to the thread that owns the worker process ([`keep`]).
    keeper: Option<Sender<Order>>,
    /// Writes to the worker's input ([`write_worker`]); `None` once closed.
    writer: Option<Sender<WriteJob>>,
    events: Receiver<Event>,
    info: DecoderInfo,
    queue: Arc<Queue>,
    stderr: Arc<Mutex<Vec<u8>>>,
    limits: Limits,
    deadline: Instant,
    eof: bool,
    finished: bool,
}

/// Frames handed to the consumer but not yet received, at most.
const MAX_QUEUED_FRAMES: usize = 4;

/// Frames handed to the consumer and not yet received. The reader waits for
/// room before it hands over another frame and frees its slot, so a slow
/// consumer slows the worker instead of growing memory
/// (`docs/design/decode.md`, "Threads").
#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    room: Condvar,
}

#[derive(Default)]
struct QueueState {
    frames: usize,
    bytes: u64,
    closed: bool,
}

impl Queue {
    /// Waits until a frame of `bytes` fits, then counts it; `false` once the
    /// decoder is gone.
    fn wait_for_room(&self, bytes: u64, limits: &Limits) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        while !state.closed
            && state.frames > 0
            && (state.frames >= MAX_QUEUED_FRAMES
                || state.bytes.saturating_add(bytes) > limits.max_pending_bytes)
        {
            match self.room.wait_timeout(state, Duration::from_millis(50)) {
                Ok((guard, _)) => state = guard,
                Err(_) => return false,
            }
        }
        if state.closed {
            return false;
        }
        state.frames += 1;
        state.bytes = state.bytes.saturating_add(bytes);
        true
    }

    fn taken(&self, bytes: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.frames = state.frames.saturating_sub(1);
            state.bytes = state.bytes.saturating_sub(bytes);
        }
        self.room.notify_all();
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
        self.room.notify_all();
    }
}

/// How long [`SandboxedDecoder`] waits for the keeper to report a killed
/// worker's status.
const KILL_PATIENCE: Duration = Duration::from_secs(2);

/// What the keeper of a worker is asked to do.
enum Order {
    /// Kill the worker now and report whether it had already exited, and
    /// how.
    Kill(Sender<Option<ExitStatus>>),
    /// The decoder is gone: kill the worker.
    Close,
}

/// Owns the worker process for its whole life (`docs/design/decode.md`,
/// "Time"). It kills the worker's process group, the backend's own
/// children (FFmpeg) with it, at the decoder's deadline or when ordered,
/// and only then reaps the worker, so the group's id cannot have been
/// reused by then. Killing it also ends any write of the parent blocked on
/// a worker that stopped reading: the write fails.
fn keep(mut child: Child, deadline: Instant, orders: &Receiver<Order>) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let reply = match orders.recv_timeout(remaining) {
        Ok(Order::Kill(reply)) => Some(reply),
        Ok(Order::Close) | Err(_) => None,
    };
    let exited = has_exited(&mut child);
    kill_group(&child);
    let _ = child.kill();
    let status = child.wait().ok();
    if let Some(reply) = reply {
        let _ = reply.send(status.filter(|_| exited));
    }
}

/// Whether the worker has exited, without reaping it (Linux's
/// `waitid(WNOWAIT)`). Elsewhere it says no, and its status goes
/// unreported: reaping it before the group is killed could let the
/// group's id be reused.
#[cfg(target_os = "linux")]
fn has_exited(child: &mut Child) -> bool {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    matches!(
        waitid(WaitId::Pid(Pid::from_child(child)), options),
        Ok(Some(_))
    )
}

#[cfg(all(unix, not(target_os = "linux")))]
fn has_exited(_: &mut Child) -> bool {
    false
}

#[cfg(not(unix))]
fn has_exited(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(Some(_)))
}

/// Kills the worker's process group (the worker leads it).
#[cfg(unix)]
fn kill_group(child: &Child) {
    use rustix::process::{Pid, Signal, kill_process_group};
    let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
}

/// Without process groups (not unix), only the worker is killed: a
/// backend's own children are not (`docs/design/decode.md`, "Time").
#[cfg(not(unix))]
fn kill_group(_: &Child) {}

/// Starts a thread, or reports why it could not.
fn spawn_thread(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), Error> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map(|_| ())
        .map_err(|e| Error::new(ErrorKind::Backend, format!("cannot start a thread: {e}")))
}

/// Bytes of the worker's standard error kept for error reports.
const KEPT_STDERR: usize = 2048;

impl SandboxedDecoder {
    /// Starts a worker hosting `backend` for `stream` and waits until it is
    /// ready.
    pub fn open(
        options: &WorkerOptions,
        backend: WorkerBackend,
        stream: &StreamConfig,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let deadline = limits.deadline();
        let memory = shared_memory(options, stream, limits);
        let transfer = match &memory {
            Some((memory, slot_size)) => Transfer::Shared {
                path: memory.path(),
                slots: options.slots,
                slot_size: *slot_size,
            },
            None => Transfer::Pipe,
        };
        let parallelism = std::thread::available_parallelism().map_or(1, |n| n.get());
        let cpu_seconds = limits
            .timeout
            .as_secs()
            .saturating_add(1)
            .saturating_mul(u64::try_from(parallelism).unwrap_or(1))
            .saturating_add(10);
        // The decoder's memory budget, within the worker options' cap: the
        // worker's address-space limit, and what its backend fits itself to.
        let memory_budget = options
            .address_space
            .map_or(limits.max_memory_bytes, |cap| {
                cap.min(limits.max_memory_bytes)
            });
        let open = Open {
            backend,
            stream: stream.clone(),
            limits: Limits {
                max_memory_bytes: memory_budget,
                ..limits.clone()
            },
            transfer,
            sandbox: SandboxLimits {
                address_space: Some(memory_budget),
                cpu_seconds: Some(cpu_seconds),
                open_files: options.open_files,
            },
            ffmpeg: options
                .ffmpeg
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            threads: options.threads,
        };
        let mut command = Command::new(&options.command.program);
        command
            .args(&options.command.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::sandbox::configure_allocator(&mut command);
        // Its own process group, so that killing the group ends the
        // backend's children too.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().map_err(|e| {
            Error::new(
                ErrorKind::Backend,
                format!(
                    "cannot start the worker {}: {e}",
                    options.command.program.display()
                ),
            )
        })?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        // From here on the keeper owns the worker: every path that gives
        // up drops `keeper` or orders a kill.
        let (keeper, orders) = mpsc::channel();
        // Shared so that the worker can still be killed here when the
        // keeper's thread cannot start (the closure is dropped then).
        let keeper_child = Arc::new(Mutex::new(Some(child)));
        {
            let child = Arc::clone(&keeper_child);
            let started = spawn_thread("coreav-worker-keeper", move || {
                if let Some(child) = child.lock().ok().and_then(|mut c| c.take()) {
                    keep(child, deadline, &orders);
                }
            });
            if let Err(e) = started {
                if let Some(mut child) = keeper_child.lock().ok().and_then(|mut c| c.take()) {
                    kill_group(&child);
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err(e);
            }
        }
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            let _ = keeper.send(Order::Close);
            return Err(Error::new(ErrorKind::Backend, "worker pipes missing"));
        };
        let kept = Arc::new(Mutex::new(Vec::new()));
        let (writer, jobs) = mpsc::channel::<WriteJob>();
        let slot_count = if memory.is_some() { options.slots } else { 0 };
        let held: HeldSlots = Arc::new((0..slot_count).map(|_| AtomicBool::new(false)).collect());
        let queue = Arc::new(Queue::default());
        let (events_tx, events) = mpsc::channel();
        let started = {
            let kept = Arc::clone(&kept);
            spawn_thread("coreav-worker-stderr", move || keep_stderr(stderr, &kept))
        }
        .and_then(|()| {
            let held = Arc::clone(&held);
            spawn_thread("coreav-worker-writer", move || {
                write_worker(stdin, &jobs, &held);
            })
        })
        .and_then(|()| {
            let writer = writer.clone();
            let queue = Arc::clone(&queue);
            let limits = limits.clone();
            let slots = memory.map(|(memory, size)| SharedSlots {
                memory,
                count: options.slots,
                size,
                lend: options.lend_frames,
            });
            spawn_thread("coreav-worker-reader", move || {
                read_worker(
                    stdout,
                    &writer,
                    &held,
                    &events_tx,
                    &queue,
                    &limits,
                    slots.as_ref(),
                );
            })
        });
        if let Err(e) = started {
            let _ = keeper.send(Order::Close);
            return Err(e);
        }
        let mut decoder = Self {
            keeper: Some(keeper),
            writer: Some(writer),
            events,
            info: DecoderInfo {
                backend: match backend {
                    WorkerBackend::Ffmpeg => BackendKind::Fallback,
                    WorkerBackend::Dav1d => BackendKind::Dav1d,
                    WorkerBackend::Libvpx => BackendKind::Libvpx,
                },
                isolation: Isolation::Sandboxed,
                decoder: String::new(),
            },
            queue,
            stderr: kept,
            limits: limits.clone(),
            deadline,
            eof: false,
            finished: false,
        };
        decoder.send(&ParentMessage::Open(Box::new(open)))?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        match decoder.events.recv_timeout(remaining) {
            Ok(Event::Ready(name)) => {
                decoder.info.decoder = name;
                Ok(decoder)
            }
            Ok(Event::Failed(e)) => Err(decoder.fail(e)),
            Ok(_) => Err(decoder.fail(Error::new(ErrorKind::Backend, "worker not ready"))),
            Err(RecvTimeoutError::Timeout) => Err(decoder.fail(Error::new(
                ErrorKind::Timeout,
                "worker did not start in time",
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(decoder.fail(Error::new(
                ErrorKind::Backend,
                "worker exited before it was ready",
            ))),
        }
    }

    fn fail(&mut self, error: Error) -> Error {
        self.finished = true;
        // How the worker ended, if it had: a signal points at a resource
        // limit or a crash in the backend.
        let status = self.kill().map(|s| format!("worker {s}"));
        let stderr = self
            .stderr
            .lock()
            .map(|b| String::from_utf8_lossy(&b).trim().to_owned())
            .unwrap_or_default();
        if error.kind != ErrorKind::Backend {
            return error;
        }
        let details: Vec<String> = status
            .into_iter()
            .chain((!stderr.is_empty()).then_some(stderr))
            .collect();
        if details.is_empty() {
            error
        } else {
            Error::new(
                error.kind,
                format!("{} ({})", error.message, details.join("; ")),
            )
        }
    }

    /// Kills the worker and returns how it had ended, if it had before.
    fn kill(&mut self) -> Option<ExitStatus> {
        self.queue.close();
        let keeper = self.keeper.take()?;
        let (reply, status) = mpsc::channel();
        keeper.send(Order::Kill(reply)).ok()?;
        status.recv_timeout(KILL_PATIENCE).ok().flatten()
    }

    /// The error for a worker whose output ended or that stopped reading:
    /// a timeout when the keeper killed it at the deadline.
    fn ended(&self, what: &str) -> Error {
        if Instant::now() >= self.deadline {
            Error::new(ErrorKind::Timeout, "decoding took too long")
        } else {
            Error::new(ErrorKind::Backend, what)
        }
    }

    /// Writes one message, waiting at most until the deadline.
    fn send(&mut self, message: &ParentMessage) -> Result<(), Error> {
        let mut bytes = Vec::new();
        message
            .write(&mut bytes)
            .map_err(|e| Error::new(ErrorKind::InvalidData, format!("message too large: {e}")))?;
        let (done, written) = mpsc::channel();
        let queued = self.writer.as_ref().is_some_and(|w| {
            w.send(WriteJob {
                bytes,
                done: Some(done),
                release: None,
            })
            .is_ok()
        });
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let outcome = if queued {
            written.recv_timeout(remaining)
        } else {
            Err(RecvTimeoutError::Disconnected)
        };
        match outcome {
            Ok(Ok(())) => Ok(()),
            Err(RecvTimeoutError::Timeout) => {
                Err(self.fail(Error::new(ErrorKind::Timeout, "decoding took too long")))
            }
            Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => {
                // The worker stopped reading: report why, if it said; its
                // report follows any frames it sent before.
                let remaining = self.deadline.saturating_duration_since(Instant::now());
                let until = Instant::now() + REPORT_PATIENCE.min(remaining);
                let error = loop {
                    let wait = until.saturating_duration_since(Instant::now());
                    match self.events.recv_timeout(wait) {
                        Ok(Event::Failed(e)) => break e,
                        Ok(Event::Frame(_, bytes)) => self.queue.taken(bytes),
                        Ok(_) | Err(_) => break self.ended("worker stopped reading"),
                    }
                };
                Err(self.fail(error))
            }
        }
    }

    fn take(&mut self, event: Result<Event, Error>) -> Result<Option<Frame>, Error> {
        match event {
            Ok(Event::Frame(frame, bytes)) => {
                self.queue.taken(bytes);
                Ok(Some(frame))
            }
            Ok(Event::Done) => {
                self.finished = true;
                Ok(None)
            }
            Ok(Event::Ready(_)) => Err(self.fail(Error::new(
                ErrorKind::Backend,
                "worker protocol violation: Ready twice",
            ))),
            Ok(Event::Ended) => {
                let error = self.ended("worker exited");
                Err(self.fail(error))
            }
            Ok(Event::Failed(e)) | Err(e) => Err(self.fail(e)),
        }
    }
}

impl Decoder for SandboxedDecoder {
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        if self.eof || self.finished {
            return Err(Error::new(ErrorKind::InvalidData, "packet after the end"));
        }
        self.limits.check_packet(packet)?;
        self.send(&ParentMessage::Packet {
            pts: packet.pts,
            dts: packet.dts,
            keyframe: packet.keyframe,
            discard: packet.discard,
            data: packet.data.to_vec(),
        })
    }

    fn send_eof(&mut self) -> Result<(), Error> {
        if self.eof || self.finished {
            return Ok(());
        }
        self.eof = true;
        self.send(&ParentMessage::Eof)
    }

    fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        if self.finished {
            return Ok(None);
        }
        if !self.eof {
            return match self.events.try_recv() {
                Ok(event) => self.take(Ok(event)),
                Err(TryRecvError::Empty) if Instant::now() > self.deadline => {
                    Err(self.fail(Error::new(ErrorKind::Timeout, "decoding took too long")))
                }
                Err(TryRecvError::Empty) => Ok(None),
                Err(TryRecvError::Disconnected) => {
                    let error = self.ended("worker exited");
                    Err(self.fail(error))
                }
            };
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let event = match self.events.recv_timeout(remaining) {
            Ok(event) => Ok(event),
            Err(RecvTimeoutError::Timeout) => {
                Err(Error::new(ErrorKind::Timeout, "decoding took too long"))
            }
            Err(RecvTimeoutError::Disconnected) => Ok(Event::Ended),
        };
        self.take(event)
    }

    fn info(&self) -> &DecoderInfo {
        &self.info
    }
}

impl Drop for SandboxedDecoder {
    /// Hands the worker to its keeper to kill, and lets go of its input;
    /// never blocks.
    fn drop(&mut self) {
        self.queue.close();
        self.writer = None;
        if let Some(keeper) = self.keeper.take() {
            let _ = keeper.send(Order::Close);
        }
    }
}

/// The parent's memory file and its slot size, when frames go through
/// shared memory: on Linux, for a stream whose size is declared. A slot
/// holds a frame of the declared size at 4:4:4 and 16 bits.
#[cfg(target_os = "linux")]
fn shared_memory(
    options: &WorkerOptions,
    stream: &StreamConfig,
    limits: &Limits,
) -> Option<(Memory, u64)> {
    if options.transfer != TransferMode::Shared || options.slots == 0 {
        return None;
    }
    let (w, h) = (stream.width?, stream.height?);
    let slot = u64::from(w)
        .checked_mul(u64::from(h))?
        .checked_mul(6)?
        .min(limits.max_frame_bytes)
        .checked_add(4095)?
        & !4095;
    let total = slot.checked_mul(u64::from(options.slots))?;
    let memory = crate::shm::ParentMemory::create(total).ok()?;
    Some((Memory(Arc::new(memory)), slot))
}

#[cfg(not(target_os = "linux"))]
fn shared_memory(_: &WorkerOptions, _: &StreamConfig, _: &Limits) -> Option<(Memory, u64)> {
    None
}

/// The parent's mapping, shared with the reader thread.
#[derive(Clone)]
struct Memory(#[cfg(target_os = "linux")] Arc<crate::shm::ParentMemory>);

impl Memory {
    fn path(&self) -> String {
        #[cfg(target_os = "linux")]
        return self.0.path_for_worker();
        #[cfg(not(target_os = "linux"))]
        String::new()
    }
}

impl SlotMemory for Memory {
    fn len(&self) -> usize {
        #[cfg(target_os = "linux")]
        return self.0.len();
        #[cfg(not(target_os = "linux"))]
        0
    }

    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
    fn copy_out(&self, start: usize, out: &mut [u8]) -> Option<()> {
        #[cfg(target_os = "linux")]
        return self.0.copy_out(start, out);
        #[cfg(not(target_os = "linux"))]
        None
    }
}

fn keep_stderr(mut stderr: impl Read, kept: &Mutex<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    while let Ok(n) = stderr.read(&mut buf) {
        if n == 0 {
            break;
        }
        if let (Ok(mut kept), Some(chunk)) = (kept.lock(), buf.get(..n)) {
            let room = KEPT_STDERR.saturating_sub(kept.len());
            kept.extend(chunk.iter().take(room));
        }
    }
}

/// A frame lent in its slot ([`WorkerOptions::lend_frames`]): read only by
/// copying out of the ranges the descriptor's check gave, and the slot
/// acknowledged when the last clone of its picture drops.
struct Lent {
    memory: Memory,
    /// The planes' bytes in the mapping in packed order: a range per plane
    /// whose rows are packed (adjacent ones joined), else a range per row.
    segments: Vec<Range<usize>>,
    /// The acknowledgement's way to the worker, and the slot.
    ack: (Sender<WriteJob>, u32),
}

impl Lent {
    fn frame(
        checked: &Checked,
        pts: Option<i64>,
        color: ColorInfo,
        memory: &Memory,
        writer: &Sender<WriteJob>,
    ) -> Option<Frame> {
        let slot = checked.slot?;
        let mut segments: Vec<Range<usize>> = Vec::new();
        for (index, (range, stride)) in checked.planes.iter().enumerate() {
            let row = checked
                .format
                .row_bytes(index, checked.width, checked.height)?;
            let rows: Vec<Range<usize>> = if *stride == row {
                vec![range.clone()]
            } else {
                range
                    .clone()
                    .step_by((*stride).max(1))
                    .map(|start| start..start.saturating_add(row).min(range.end))
                    .collect()
            };
            for segment in rows {
                match segments.last_mut() {
                    Some(last) if last.end == segment.start => last.end = segment.end,
                    _ => segments.push(segment),
                }
            }
        }
        let lent = Self {
            memory: memory.clone(),
            segments,
            ack: (writer.clone(), slot),
        };
        Some(Frame {
            pts,
            color,
            pixels: Pixels::Shared(SharedPicture {
                width: checked.width,
                height: checked.height,
                format: checked.format,
                memory: Arc::new(lent),
            }),
        })
    }
}

impl SharedMemory for Lent {
    fn read_packed(&self, mut at: usize, out: &mut [u8]) -> Option<usize> {
        let mut filled = 0;
        for segment in &self.segments {
            if filled == out.len() {
                break;
            }
            let len = segment.len();
            if at >= len {
                at -= len;
                continue;
            }
            let n = (len - at).min(out.len() - filled);
            let target = out.get_mut(filled..filled + n)?;
            self.memory
                .copy_out(segment.start.checked_add(at)?, target)?;
            filled += n;
            at = 0;
        }
        Some(filled)
    }
}

impl Drop for Lent {
    /// Gives the slot back: the writer clears its flag before the worker
    /// can read the acknowledgement. A worker gone or no longer reading
    /// gets none, which nothing waits for.
    fn drop(&mut self) {
        let (writer, slot) = &self.ack;
        let mut bytes = Vec::new();
        if ParentMessage::Ack(*slot).write(&mut bytes).is_ok() {
            let _ = writer.send(WriteJob {
                bytes,
                done: None,
                release: Some(*slot),
            });
        }
    }
}

/// The shared memory as the reader uses it.
struct SharedSlots {
    memory: Memory,
    count: u32,
    /// Bytes per slot.
    size: u64,
    /// Whether frames in slots are lent ([`WorkerOptions::lend_frames`]).
    lend: bool,
}

/// Reads the worker's messages: validates each frame and queues it for
/// [`SandboxedDecoder`], copied out with its slot acknowledged, or lent in
/// its slot.
fn read_worker(
    mut stdout: ChildStdout,
    writer: &Sender<WriteJob>,
    held: &HeldSlots,
    events: &Sender<Event>,
    queue: &Queue,
    limits: &Limits,
    shared: Option<&SharedSlots>,
) {
    let violation = |what: &str| {
        Event::Failed(Error::new(
            ErrorKind::Backend,
            format!("worker protocol violation: {what}"),
        ))
    };
    let mut ready = false;
    let mut stopped_reading = false;
    let mut seq = 0u64;
    let mut decoded = 0u64;
    loop {
        let message = match read_message(&mut stdout, |k| {
            WorkerMessage::max_len(k, limits.max_frame_bytes)
        }) {
            Ok(Incoming::Message(kind, payload)) => WorkerMessage::decode(kind, &payload),
            Ok(Incoming::End) => {
                let _ = events.send(if stopped_reading {
                    violation("worker stopped reading")
                } else {
                    Event::Ended
                });
                return;
            }
            Err(e) => {
                let _ = events.send(violation(e.0));
                return;
            }
        };
        let event = match message {
            Err(e) => violation(e.0),
            Ok(WorkerMessage::Ready(_)) if ready => violation("Ready twice"),
            Ok(WorkerMessage::Ready(name)) => {
                ready = true;
                Event::Ready(name)
            }
            // A worker that cannot open its decoder says why instead of
            // Ready.
            Ok(WorkerMessage::Error(kind, text)) if !ready => {
                Event::Failed(Error::new(kind, format!("worker: {text}")))
            }
            Ok(_) if !ready => violation("message before Ready"),
            // Before any byte of the slot is read: a slot the parent still
            // holds is not the worker's to fill.
            Ok(WorkerMessage::Frame(descriptor, _))
                if descriptor.slot != INLINE
                    && usize::try_from(descriptor.slot)
                        .ok()
                        .and_then(|s| held.get(s))
                        .is_none_or(|f| f.swap(true, Ordering::SeqCst)) =>
            {
                violation(if held.is_empty() {
                    "slot without shared memory"
                } else {
                    "a frame in a slot the parent holds"
                })
            }
            Ok(WorkerMessage::Frame(descriptor, inline)) => {
                let slots = shared.map(|s| Slots {
                    memory: &s.memory,
                    count: s.count,
                    size: s.size,
                });
                let received = match shared {
                    Some(SharedSlots { memory, lend, .. })
                        if *lend && descriptor.slot != INLINE =>
                    {
                        check(&descriptor, inline.len(), seq, slots, limits).and_then(|checked| {
                            let frame = Lent::frame(
                                &checked,
                                descriptor.pts,
                                descriptor.color,
                                memory,
                                writer,
                            )
                            .ok_or_else(|| {
                                Error::new(ErrorKind::Backend, "a lent frame without its slot")
                            })?;
                            Ok(crate::descriptor::Accepted {
                                frame,
                                slot: None,
                                bytes: checked.bytes,
                            })
                        })
                    }
                    _ => accept(&descriptor, &inline, seq, slots, limits),
                };
                match received {
                    Err(e) => Event::Failed(e),
                    Ok(accepted) => {
                        seq += 1;
                        decoded = decoded.saturating_add(accepted.bytes);
                        if decoded > limits.max_decoded_bytes {
                            Event::Failed(Error::new(
                                ErrorKind::Limit,
                                "decoded bytes exceed the limit",
                            ))
                        } else {
                            // Backpressure: hand the frame over only when the
                            // consumer has room, and free its slot after.
                            if !queue.wait_for_room(accepted.bytes, limits) {
                                return;
                            }
                            let bytes = accepted.bytes;
                            if events.send(Event::Frame(accepted.frame, bytes)).is_err() {
                                return;
                            }
                            if let (Some(slot), false) = (accepted.slot, stopped_reading) {
                                // A worker that stopped reading has usually
                                // failed: keep reading for its error.
                                let mut ack = Vec::new();
                                stopped_reading = ParentMessage::Ack(slot).write(&mut ack).is_err()
                                    || writer
                                        .send(WriteJob {
                                            bytes: ack,
                                            done: None,
                                            release: Some(slot),
                                        })
                                        .is_err();
                            }
                            continue;
                        }
                    }
                }
            }
            Ok(WorkerMessage::Done) => {
                let _ = events.send(Event::Done);
                // Nothing may follow; the session is over.
                return;
            }
            Ok(WorkerMessage::Error(kind, text)) => {
                Event::Failed(Error::new(kind, format!("worker: {text}")))
            }
        };
        let stop = matches!(event, Event::Failed(_));
        if events.send(event).is_err() || stop {
            return;
        }
    }
}
