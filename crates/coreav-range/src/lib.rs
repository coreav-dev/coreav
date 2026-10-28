//! The read plan for a media object read by byte ranges (`docs/design/remote.md`).
//!
//! A parser reads its input as scattered small reads (headers, sample
//! tables) and long forward passes (a writer copying samples). Over a
//! network each request costs a round trip, so the plan decides which
//! requests to make, when, and how far ahead to read:
//!
//! - **Cursors.** A read at a new position opens a response of
//!   [`Settings::first_span`] bytes, read front to back. A read up to
//!   [`Settings::max_skip`] bytes ahead of an open response is served by
//!   reading through the gap. Up to [`Settings::open_bodies`] responses stay
//!   open at their positions, so a file whose movie box is at its end is
//!   read at its start, at its end and at its start again without a new
//!   request.
//! - **Pipelines.** Reading on past the end of a response, or advice that a
//!   copy pass follows ([`Reader::will_read`]), starts a pipeline: up to
//!   [`Settings::parallel`] segment requests of [`Settings::segment_bytes`]
//!   in flight, each read ahead of the reader by at most its share of
//!   [`Settings::buffer_bytes`] and kept [`Settings::keep_behind`] bytes
//!   behind it. A read more than `max_skip` past everything requested
//!   restarts the pipeline. Up to [`Settings::max_pipelines`] run at once,
//!   since a clip's tracks may be stored far apart.
//! - **A cache** of the last [`Settings::cache_bytes`] received outside
//!   pipelines serves reads that go back, such as the passes over sample
//!   tables.
//! - **Pinning, caps and retries.** The first response fixes the object's
//!   size and, if the host gives one, its validator (a strong ETag); a later
//!   response that differs fails with [`Error::Changed`]. One reader makes
//!   at most [`Settings::max_requests`] requests and receives at most
//!   [`Settings::max_bytes`]. A read that fails for a reason a new request
//!   can fix is tried again from its offset, up to [`Settings::retries`]
//!   times.
//!
//! The crate does no I/O: a [`Host`] starts requests and moves their bytes,
//! blocking or not, and may run several requests at once. The plan is the
//! one of the range reader of the JavaScript package around the WebAssembly
//! module (issue #87), with the numbers sized for that host as parameters
//! ([`Settings::WORKERS`]) and a preset for native hosts
//! ([`Settings::NATIVE`]).

use std::collections::VecDeque;

const MIB: u64 = 1 << 20;

/// Bytes a host is asked to read per call, at most.
const CHUNK: u64 = 256 << 10;

/// Unread bytes a response may still hold for a read past its end to count
/// as reading on.
const ONWARD: u64 = MIB;

/// Bytes a host may hold ahead of the reader for an open response outside a
/// pipeline: enough to keep a connection busy, little enough that bytes
/// never read are not transferred.
const CURSOR_READ_AHEAD: u64 = MIB;

/// The plan's numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Bytes a response opened at a new position asks for.
    pub first_span: u64,
    /// How far ahead of an open response a read may start and still be
    /// served by reading through the gap.
    pub max_skip: u64,
    /// Bytes received outside pipelines that are kept for reads that go
    /// back.
    pub cache_bytes: u64,
    /// Segment requests a pipeline keeps in flight (shared by the running
    /// pipelines).
    pub parallel: usize,
    /// Bytes of one pipeline segment (larger when the object would
    /// otherwise need more requests than [`Self::max_requests`]).
    pub segment_bytes: u64,
    /// Bytes the pipelines may hold ahead of their readers, together.
    pub buffer_bytes: u64,
    /// Bytes a pipeline keeps behind its reader (copy windows refill from a
    /// sample's start).
    pub keep_behind: u64,
    /// Pipelines that run at once.
    pub max_pipelines: usize,
    /// Responses kept open outside pipelines (one while a pipeline runs).
    pub open_bodies: usize,
    /// Requests one reader may make.
    pub max_requests: u32,
    /// Bytes one reader may receive.
    pub max_bytes: u64,
    /// Times a failed read is tried again.
    pub retries: u32,
    /// Bytes at the object's end requested together with the first request
    /// (a suffix range, which needs no size), so that an index stored at the
    /// end (an MP4 movie box written last, Matroska cues) is read without a
    /// second round trip; 0 for none.
    pub tail_bytes: u64,
    /// Gaps between advised regions ([`Reader::will_read_regions`]) shorter
    /// than this are read through rather than skipped; `None` sets it per
    /// reader ([`Reader::sparse_gap`]).
    pub sparse_gap: Option<u64>,
    /// The bandwidth, in bytes per second, assumed for the requests in
    /// flight together when [`Self::sparse_gap`] is set per reader.
    pub bandwidth: u64,
}

impl Settings {
    /// The JavaScript package's numbers: six connections per request (four
    /// segments and one response), 128 MB of memory, at most 1,000
    /// requests.
    pub const WORKERS: Self = Self {
        first_span: 4 * MIB,
        max_skip: 8 * MIB,
        cache_bytes: 8 * MIB,
        parallel: 4,
        segment_bytes: 16 * MIB,
        buffer_bytes: 24 * MIB,
        keep_behind: 8 * MIB,
        max_pipelines: 2,
        open_bodies: 3,
        max_requests: 1000,
        max_bytes: u64::MAX,
        retries: 2,
        tail_bytes: 0,
        sparse_gap: None,
        bandwidth: 60_000_000,
    };

    /// A native process's numbers: the general ones of [`Self::WORKERS`],
    /// with more memory for read-ahead and the cache, a higher request cap,
    /// the tail, and a first span of 32 MiB (a native host cancels a
    /// response by closing its connection, so a long span transfers only
    /// what is in flight, and a scan from the start reaches its pipeline one
    /// round trip later).
    pub const NATIVE: Self = Self {
        first_span: 32 * MIB,
        buffer_bytes: 64 * MIB,
        cache_bytes: 16 * MIB,
        max_requests: 10_000,
        tail_bytes: MIB,
        ..Self::WORKERS
    };

    /// Why the settings cannot work, if they cannot.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.first_span == 0 || self.segment_bytes == 0 || self.parallel == 0 {
            return Err("first_span, segment_bytes and parallel must be at least 1");
        }
        if self.max_pipelines == 0 || self.open_bodies == 0 || self.max_requests == 0 {
            return Err("max_pipelines, open_bodies and max_requests must be at least 1");
        }
        Ok(())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::NATIVE
    }
}

/// A request the plan asks the host to make: the bytes
/// `[offset, offset + length)` of the object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    /// The first byte.
    pub offset: u64,
    /// How many bytes, at most; the response may hold fewer.
    pub length: u64,
    /// The last `length` bytes of the object instead (`offset` is 0): a
    /// suffix range (`Range: bytes=-length`).
    pub suffix: bool,
    /// The validator of the object the first response came from; the host
    /// must fail with [`HostError::Changed`] if the object no longer has it
    /// (`If-Match`).
    pub validator: Option<&'a str>,
    /// How many bytes the host may receive ahead of what the plan has read
    /// from this response. Bytes beyond it wait in the connection.
    pub read_ahead: u64,
}

/// The head of a response, as the host received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// The first byte the response holds; must be the requested offset.
    pub offset: u64,
    /// The offset just past the last byte it holds, if the host knows it
    /// (`Content-Range`); a response may hold less than asked, never more.
    pub end: Option<u64>,
    /// The object's size.
    pub size: u64,
    /// The object's validator, if the host has a strong one.
    pub validator: Option<String>,
    /// The host received the whole object from byte 0 (a server that
    /// ignores ranges).
    pub whole: bool,
    /// How long the host waited for this head after starting the request,
    /// if it measured it.
    pub wait: Option<core::time::Duration>,
}

/// Why a host's request or read failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    /// A failure a new request may fix (a reset connection, a timeout, a
    /// server error).
    Retry(String),
    /// The object is not the one the reader pinned.
    Changed(String),
    /// The object does not exist.
    NotFound(String),
    /// The response breaks the protocol or the request cannot succeed (a
    /// client error, a refused redirect, a refused address).
    Refused(String),
}

/// Why a read failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The object changed: its size or validator differs from the first
    /// response's.
    Changed(String),
    /// The object does not exist.
    NotFound(String),
    /// A response broke the protocol, or a request cannot succeed.
    Refused(String),
    /// The reader made [`Settings::max_requests`] requests.
    RequestLimit(u32),
    /// The reader received [`Settings::max_bytes`] bytes.
    ByteLimit(u64),
    /// A read failed after its retries.
    Failed(String),
    /// A read beyond the object's end.
    OutOfRange {
        /// The read's offset.
        offset: u64,
        /// The read's length.
        length: u64,
        /// The object's size.
        size: u64,
    },
}

impl Error {
    /// Whether a new request cannot fix it.
    fn fatal(&self) -> bool {
        !matches!(self, Self::Failed(_))
    }
}

impl From<HostError> for Error {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Retry(message) => Self::Failed(message),
            HostError::Changed(message) => Self::Changed(message),
            HostError::NotFound(message) => Self::NotFound(message),
            HostError::Refused(message) => Self::Refused(message),
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Changed(m) => write!(f, "the object changed while it was read: {m}"),
            Self::NotFound(m) => write!(f, "no such object: {m}"),
            Self::Refused(m) | Self::Failed(m) => f.write_str(m),
            Self::RequestLimit(n) => write!(f, "more than {n} requests for one object"),
            Self::ByteLimit(n) => write!(f, "more than {n} bytes received for one object"),
            Self::OutOfRange {
                offset,
                length,
                size,
            } => write!(
                f,
                "a read of {length} bytes at {offset} past the object's end ({size} bytes)"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// What a reader did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Requests started.
    pub requests: u32,
    /// Pipelines started.
    pub pipelines: u32,
    /// Bytes the requests asked for.
    pub bytes_requested: u64,
    /// Bytes read from responses.
    pub bytes_received: u64,
}

/// What makes the requests.
///
/// A host may run requests at once: [`Host::start`] returns at once, and
/// the plan asks for a response's head ([`Host::response`]) and bytes
/// ([`Host::read`]) when it needs them, in any order across handles. A host
/// that blocks simply waits in those calls.
pub trait Host {
    /// One request in flight.
    type Handle;

    /// Starts a request.
    fn start(&mut self, request: &Request<'_>) -> Self::Handle;

    /// The head of the response, waiting for it if needed.
    fn response(&mut self, handle: &mut Self::Handle) -> Result<Response, HostError>;

    /// Reads the next bytes of the response's body into `buf` (at most
    /// `buf.len()`), waiting for at least one; `Ok(0)` at its end.
    fn read(&mut self, handle: &mut Self::Handle, buf: &mut [u8]) -> Result<usize, HostError>;

    /// Ends a request: nothing more is read from it.
    fn cancel(&mut self, handle: Self::Handle);

    /// Every byte the plan receives, with its offset, for a host that keeps
    /// them (a native host spools them to a temporary file, so that a
    /// second pass over the object is not fetched again). Does nothing by
    /// default.
    fn keep(&mut self, offset: u64, bytes: &[u8]) {
        let _ = (offset, bytes);
    }

    /// Fills `buf` with the bytes from `offset` if the host kept them all;
    /// asked before every read. None by default.
    fn kept(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        let _ = (offset, buf);
        false
    }
}

/// An open response at a position in the object, read front to back; or,
/// without a handle, the place where a used-up response ended.
struct Body<H> {
    handle: Option<H>,
    start: u64,
    pos: u64,
    end: u64,
    whole: bool,
}

/// Adds `bytes` received at `at` to `pieces`: onto the last piece when they
/// follow it and it is under [`CHUNK`], else as a new piece of their exact
/// size. A server that sends a byte at a time thus costs memory in
/// proportion to the bytes held, not one buffer per read.
fn append(pieces: &mut VecDeque<(u64, Vec<u8>)>, at: u64, bytes: &[u8]) {
    if let Some((start, last)) = pieces.back_mut() {
        if start.saturating_add(last.len() as u64) == at && (last.len() as u64) < CHUNK {
            last.extend_from_slice(bytes);
            return;
        }
    }
    pieces.push_back((at, bytes.to_vec()));
}

/// The memory `pieces` hold, by capacity.
fn held(pieces: &VecDeque<(u64, Vec<u8>)>) -> u64 {
    pieces.iter().map(|(_, b)| b.capacity() as u64).sum()
}

/// Recently received bytes, kept up to a byte bound, oldest dropped first.
struct Recent {
    limit: u64,
    pieces: VecDeque<(u64, Vec<u8>)>,
    bytes: u64,
}

impl Recent {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            pieces: VecDeque::new(),
            bytes: 0,
        }
    }

    fn keep(&mut self, start: u64, bytes: &[u8]) {
        if self.limit == 0 || bytes.is_empty() {
            return;
        }
        append(&mut self.pieces, start, bytes);
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        while self.bytes > self.limit {
            match self.pieces.pop_front() {
                Some((_, dropped)) => self.bytes = self.bytes.saturating_sub(dropped.len() as u64),
                None => break,
            }
        }
    }

    /// The newest kept piece holding byte `at`.
    fn piece_at(&self, at: u64) -> Option<&(u64, Vec<u8>)> {
        self.pieces
            .iter()
            .rev()
            .find(|(start, bytes)| *start <= at && at < start.saturating_add(bytes.len() as u64))
    }

    /// Copies the kept bytes at the start of `[offset, offset + out.len())`
    /// into `out`, as far as they reach without a gap; returns the count.
    fn copy_prefix(&self, offset: u64, out: &mut [u8]) -> usize {
        let mut done = 0usize;
        while done < out.len() {
            let at = offset.saturating_add(done as u64);
            let Some((start, bytes)) = self.piece_at(at) else {
                break;
            };
            let from = usize::try_from(at - start).unwrap_or(usize::MAX);
            let Some(available) = bytes.get(from..) else {
                break;
            };
            let Some(target) = out.get_mut(done..) else {
                break;
            };
            let n = available.len().min(target.len());
            if let (Some(t), Some(s)) = (target.get_mut(..n), available.get(..n)) {
                t.copy_from_slice(s);
            }
            done = done.saturating_add(n);
            if n == 0 {
                break;
            }
        }
        done
    }
}

/// One segment of a pipeline: the bytes `[start, end)`, requested when the
/// segment is created and read when the reader reaches them. A response
/// that holds less than asked is followed by a request for the rest.
struct Segment<H> {
    start: u64,
    end: u64,
    handle: Option<H>,
    /// Whether the current handle's response head has been received.
    opened: bool,
    /// The end of the current response.
    body_end: u64,
    /// Bytes read from the host, from `start`.
    received: u64,
    /// Read bytes still kept, in order.
    pieces: VecDeque<(u64, Vec<u8>)>,
}

impl<H> Segment<H> {
    fn first(&self) -> u64 {
        self.pieces
            .front()
            .map_or(self.start.saturating_add(self.received), |(at, _)| *at)
    }

    fn arrived(&self) -> u64 {
        self.start.saturating_add(self.received)
    }

    fn trim(&mut self, before: u64) {
        while let Some((at, bytes)) = self.pieces.front() {
            if at.saturating_add(bytes.len() as u64) <= before {
                self.pieces.pop_front();
            } else {
                break;
            }
        }
    }

    /// Copies the kept bytes of `[at, to)` into `out` at `at - base`;
    /// returns where it stopped.
    fn copy(&self, mut at: u64, to: u64, out: &mut [u8], base: u64) -> u64 {
        for (start, bytes) in &self.pieces {
            let end = start.saturating_add(bytes.len() as u64);
            if end <= at {
                continue;
            }
            if *start > at || at >= to {
                break;
            }
            let till = to.min(end);
            let (Ok(from), Ok(n), Ok(into)) = (
                usize::try_from(at - start),
                usize::try_from(till - at),
                usize::try_from(at - base),
            ) else {
                break;
            };
            let (Some(source), Some(target)) = (
                bytes.get(from..from.saturating_add(n)),
                out.get_mut(into..into.saturating_add(n)),
            ) else {
                break;
            };
            target.copy_from_slice(source);
            at = till;
        }
        at
    }
}

/// A forward region `[next.., end)` read by segments in flight, handed out
/// in order.
struct Pipeline<H> {
    end: u64,
    segment_bytes: u64,
    parallel: usize,
    read_ahead: u64,
    segments: VecDeque<Segment<H>>,
    /// The first byte not yet requested.
    next: u64,
    /// The reader's position.
    position: u64,
    /// Where reads count as ahead of the reader: `position`, except over
    /// advised regions before the reader first moves, when it is the first
    /// advised byte (an open response serves the spans before `start`).
    ahead_from: u64,
    /// For advice of regions, the only bytes requested: sorted `[start,
    /// end)` spans apart from each other within the region; empty for a
    /// pipeline over the whole region.
    spans: Vec<(u64, u64)>,
    /// The span `next` lies in, or the first after it.
    span: usize,
}

impl<H> Pipeline<H> {
    fn start(&self) -> u64 {
        self.segments.front().map_or(self.next, Segment::first)
    }

    fn sparse(&self) -> bool {
        !self.spans.is_empty()
    }

    /// Where a read at `at` must stop in this pipeline: its end, or the end
    /// of the span that holds `at`; `None` if `at` lies between spans.
    fn stop(&self, at: u64) -> Option<u64> {
        if self.spans.is_empty() {
            return Some(self.end);
        }
        let i = self.spans.partition_point(|&(_, end)| end <= at);
        self.spans
            .get(i)
            .filter(|&&(start, _)| start <= at)
            .map(|&(_, end)| end)
    }

    /// Moves `next` to a byte the pipeline requests: into the span it lies
    /// in or the first after it, or to `end` past the last.
    fn skip_gaps(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        while let Some(&(start, end)) = self.spans.get(self.span) {
            if self.next < end {
                self.next = self.next.max(start);
                return;
            }
            self.span = self.span.saturating_add(1);
        }
        self.next = self.end;
    }

    fn segment_at(&self, at: u64) -> Option<usize> {
        self.segments
            .iter()
            .position(|s| s.start <= at && at < s.end)
    }
}

/// A reader of one remote object, following the plan; see the crate
/// documentation.
pub struct Reader<H: Host> {
    host: H,
    settings: Settings,
    size: u64,
    /// Whether a response has given the size (and pinned the validator).
    pinned: bool,
    validator: Option<String>,
    /// Open responses, the most recently read first.
    cursors: Vec<Body<H::Handle>>,
    /// Pipelines, the most recently read first.
    forward: Vec<Pipeline<H::Handle>>,
    recent: Recent,
    stats: Stats,
    /// The buffer pipeline segments are read into.
    scratch: Vec<u8>,
    /// How long the first response took to arrive, if the host measured it.
    first_wait: Option<core::time::Duration>,
}

impl<H: Host> Reader<H> {
    /// Opens the object: the first request, at byte 0, which also gives
    /// its size; retried as reads are.
    pub fn open(host: H, settings: Settings) -> Result<Self, Error> {
        settings
            .check()
            .map_err(|reason| Error::Refused(reason.to_owned()))?;
        let mut reader = Self {
            host,
            recent: Recent::new(settings.cache_bytes),
            settings,
            size: 0,
            pinned: false,
            validator: None,
            cursors: Vec::new(),
            forward: Vec::new(),
            stats: Stats::default(),
            scratch: Vec::new(),
            first_wait: None,
        };
        // The end of the object, asked for at once; checked once the first
        // response has pinned the size.
        let tail =
            (reader.settings.tail_bytes > 0 && reader.settings.max_requests > 1).then(|| {
                reader.stats.requests = reader.stats.requests.saturating_add(1);
                let length = reader.settings.tail_bytes;
                reader.stats.bytes_requested = reader.stats.bytes_requested.saturating_add(length);
                reader.host.start(&Request {
                    offset: 0,
                    length,
                    suffix: true,
                    validator: None,
                    read_ahead: length.min(CURSOR_READ_AHEAD),
                })
            });
        let mut retries = reader.settings.retries;
        loop {
            let span = reader.settings.first_span;
            match reader.request(0, span, true) {
                Ok(body) => {
                    reader.cursors.push(body);
                    break;
                }
                Err(error) if error.fatal() || retries == 0 => {
                    if let Some(tail) = tail {
                        reader.host.cancel(tail);
                    }
                    return Err(error);
                }
                Err(_) => retries -= 1,
            }
        }
        if let Some(tail) = tail {
            reader.adopt_tail(tail)?;
        }
        Ok(reader)
    }

    /// Keeps the tail response as an open response at the object's end,
    /// unless the first response already holds the whole object or the
    /// tail is unusable (then a later read makes its own request). A tail
    /// from another version of the object fails the open.
    fn adopt_tail(&mut self, mut tail: H::Handle) -> Result<(), Error> {
        let covered = self.cursors.first().is_some_and(|b| b.end >= self.size);
        if covered {
            self.host.cancel(tail);
            return Ok(());
        }
        let response = match self.host.response(&mut tail) {
            Ok(response) => response,
            Err(_) => {
                self.host.cancel(tail);
                return Ok(());
            }
        };
        if response.size != self.size {
            self.host.cancel(tail);
            return Err(Error::Changed(format!(
                "the object's size changed from {} to {}",
                self.size, response.size
            )));
        }
        if let (Some(pinned), Some(seen)) = (&self.validator, &response.validator) {
            if pinned != seen {
                self.host.cancel(tail);
                return Err(Error::Changed("its validator changed".to_owned()));
            }
        }
        let end = response.end.unwrap_or(response.size).min(self.size);
        if response.whole || response.offset >= end || end != self.size {
            self.host.cancel(tail);
            return Ok(());
        }
        self.cursors.push(Body {
            handle: Some(tail),
            start: response.offset,
            pos: response.offset,
            end,
            whole: false,
        });
        Ok(())
    }

    /// The object's size.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The object's validator (its strong ETag), if it has one.
    pub fn validator(&self) -> Option<&str> {
        self.validator.as_deref()
    }

    /// The memory the reader holds for received bytes (the cache and the
    /// pipelines' pieces), by capacity.
    pub fn held_bytes(&self) -> u64 {
        let pipelines: u64 = self
            .forward
            .iter()
            .flat_map(|p| p.segments.iter())
            .map(|s| held(&s.pieces))
            .sum();
        held(&self.recent.pieces).saturating_add(pipelines)
    }

    /// What the reader did so far.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The host, for example to read what it recorded.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// Fills `buf` with the bytes from `offset`.
    pub fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let length = buf.len() as u64;
        if offset.checked_add(length).is_none_or(|end| end > self.size) {
            return Err(Error::OutOfRange {
                offset,
                length,
                size: self.size,
            });
        }
        if buf.is_empty() {
            return Ok(());
        }
        let mut retries = self.settings.retries;
        loop {
            match self.read_once(offset, buf) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    self.drop_all();
                    if error.fatal() || retries == 0 {
                        return Err(error);
                    }
                    retries -= 1;
                }
            }
        }
    }

    /// Advice that reads of about `length` bytes from `offset` follow (a
    /// writer's copy pass): a pipeline starts there now, over a sixteenth
    /// more (at least one segment) for samples of other tracks between
    /// them, after the part an open response already holds.
    pub fn will_read(&mut self, offset: u64, length: u64) {
        if offset >= self.size
            || self.stats.requests.saturating_add(1) >= self.settings.max_requests
        {
            return;
        }
        let extra = length.div_ceil(16).max(self.settings.segment_bytes);
        let end = self
            .size
            .min(offset.saturating_add(length).saturating_add(extra));
        let mut start = offset;
        let mut keep = None;
        for (i, body) in self.cursors.iter().enumerate() {
            if offset < body.start || offset >= body.end {
                continue;
            }
            if offset.saturating_sub(body.pos) > self.settings.max_skip {
                continue;
            }
            if body.end > start {
                start = body.end;
                keep = Some(i);
            }
        }
        if self
            .pipeline_for(start)
            .is_some_and(|p| self.forward.get(p).is_some_and(|p| p.end >= end))
        {
            return;
        }
        if start < end {
            let keep = keep.or(if self.cursors.is_empty() {
                None
            } else {
                Some(0)
            });
            self.start_pipeline(start, end, keep, true);
        }
    }

    /// Advice that the reads that follow touch only `regions`, offset and
    /// length pairs (a writer's copy pass through the samples of one track,
    /// a scan of each sample's header). One pipeline reads them in order,
    /// joining regions whose gaps are under [`Settings::sparse_gap`] and
    /// skipping the larger gaps, in at most half the requests left; where
    /// they would need more, the gap doubles until they fit, and a single
    /// span that still does not fit gives no advice. A read outside the
    /// spans asks for its own bytes only. An empty list ends the advice: its
    /// pipeline is dropped and reads go anywhere again.
    pub fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        if regions.is_empty() {
            let (sparse, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.forward)
                .into_iter()
                .partition(Pipeline::sparse);
            self.forward = rest;
            for pipeline in sparse {
                self.cancel_segments(pipeline.segments);
            }
            return;
        }
        // A response that holds the whole object (a server that ignores
        // ranges) serves every read; a range request would be refused.
        if self.cursors.iter().any(|body| body.whole) {
            return;
        }
        let mut list = Vec::new();
        if list.try_reserve(regions.len()).is_err() {
            return;
        }
        for &(offset, length) in regions {
            let end = self.size.min(offset.saturating_add(length));
            if offset < end {
                list.push((offset, end));
            }
        }
        list.sort_unstable();
        let budget = u64::from(
            self.settings
                .max_requests
                .saturating_sub(self.stats.requests)
                / 2,
        );
        if budget == 0 {
            return;
        }
        let mut gap = self.sparse_gap().max(1);
        loop {
            let mut spans: Vec<(u64, u64)> = Vec::new();
            if spans.try_reserve(list.len()).is_err() {
                return;
            }
            for &(start, end) in &list {
                match spans.last_mut() {
                    Some(last) if start.saturating_sub(last.1) < gap => last.1 = last.1.max(end),
                    _ => spans.push((start, end)),
                }
            }
            // Segments grow, as for a forward pass, so that the spans' bytes
            // fit the requests left.
            let bytes: u64 = spans.iter().map(|&(start, end)| end - start).sum();
            let segment = self.settings.segment_bytes.max(bytes.div_ceil(budget));
            let requests: u64 = spans
                .iter()
                .map(|&(start, end)| (end - start).div_ceil(segment))
                .sum();
            if requests <= budget {
                let advised = spans.first().map_or(0, |&(start, _)| start);
                let (spans, keep) = self.after_open(spans);
                let (Some(&(start, _)), Some(&(_, end))) = (spans.first(), spans.last()) else {
                    // An open response serves all of it: the advice before
                    // ends, as an empty list would end it.
                    self.will_read_regions(&[]);
                    return;
                };
                let keep = keep.or((!self.cursors.is_empty()).then_some(0));
                self.begin_pipeline(start, end, keep, true, spans, Some(segment));
                if let Some(pipeline) = self.forward.first_mut() {
                    pipeline.ahead_from = advised;
                }
                return;
            }
            if spans.len() == 1 {
                return;
            }
            gap = gap.saturating_mul(2);
        }
    }

    /// The spans an open response does not already serve, and that
    /// response. A response that holds the first span's start (read, or
    /// still to come within `max_skip`) serves the spans as far as it
    /// reaches, up to the first gap it could not read through (more than
    /// `max_skip`); the pipeline requests the rest. A small object's first
    /// response often holds all of it.
    fn after_open(&self, spans: Vec<(u64, u64)>) -> (Vec<(u64, u64)>, Option<usize>) {
        let Some(&(first, _)) = spans.first() else {
            return (spans, None);
        };
        let max_skip = self.settings.max_skip;
        let mut best: Option<(usize, u64)> = None;
        for (i, body) in self.cursors.iter().enumerate() {
            if body.handle.is_none() || first < body.start || first >= body.end {
                continue;
            }
            if first.saturating_sub(body.pos) > max_skip {
                continue;
            }
            // How far its reads would reach along the spans.
            let mut reach = body.pos.max(first);
            for &(start, end) in &spans {
                if start >= body.end || start.saturating_sub(reach) > max_skip {
                    break;
                }
                reach = end.min(body.end);
            }
            if best.is_none_or(|(_, r)| reach > r) {
                best = Some((i, reach));
            }
        }
        let Some((keep, reach)) = best else {
            return (spans, None);
        };
        let rest = spans
            .into_iter()
            .filter(|&(_, end)| end > reach)
            .map(|(start, end)| (start.max(reach), end))
            .collect();
        (rest, Some(keep))
    }

    /// The gap between advised regions under which the reader reads
    /// through rather than skips ([`Settings::sparse_gap`]). Skipping a gap
    /// of `g` bytes saves its transfer, `g / B` with `B` the bandwidth of
    /// the requests in flight together, and costs one more request: a
    /// round trip `L` on one of `k` connections, about `L / k` of the
    /// pass's time. Skipping pays above `L × B / k`. Unless the settings fix
    /// it, `L` is the first response's wait as the host measured it and `B`
    /// is [`Settings::bandwidth`], and the gap is kept within 1 to 8 MiB; a
    /// host that measures no wait gets 4 MiB, the gap at a 275 ms round
    /// trip (`docs/design/range-extraction.md`).
    pub fn sparse_gap(&self) -> u64 {
        if let Some(gap) = self.settings.sparse_gap {
            return gap;
        }
        let Some(wait) = self.first_wait else {
            return 4 * MIB;
        };
        let parallel = u128::try_from(self.settings.parallel.max(1)).unwrap_or(1);
        let gap = wait
            .as_micros()
            .saturating_mul(u128::from(self.settings.bandwidth))
            / 1_000_000
            / parallel;
        u64::try_from(gap).unwrap_or(u64::MAX).clamp(MIB, 8 * MIB)
    }

    /// Ends every request; the reader can still read, with new requests.
    pub fn close(&mut self) {
        self.drop_all();
    }

    fn drop_all(&mut self) {
        for body in std::mem::take(&mut self.cursors) {
            if let Some(handle) = body.handle {
                self.host.cancel(handle);
            }
        }
        for pipeline in std::mem::take(&mut self.forward) {
            self.cancel_segments(pipeline.segments);
        }
    }

    fn cancel_segments(&mut self, segments: VecDeque<Segment<H::Handle>>) {
        for segment in segments {
            if let Some(handle) = segment.handle {
                self.host.cancel(handle);
            }
        }
    }

    fn cancel_cursor(&mut self, index: usize) {
        if index < self.cursors.len() {
            let body = self.cursors.remove(index);
            if let Some(handle) = body.handle {
                self.host.cancel(handle);
            }
        }
    }

    /// Counts and starts a request.
    fn begin(
        &mut self,
        offset: u64,
        span: u64,
        read_ahead: u64,
    ) -> Result<(H::Handle, u64), Error> {
        if self.stats.requests >= self.settings.max_requests {
            return Err(Error::RequestLimit(self.settings.max_requests));
        }
        let length = if !self.pinned {
            span
        } else {
            span.min(self.size.saturating_sub(offset))
        };
        self.stats.requests = self.stats.requests.saturating_add(1);
        self.stats.bytes_requested = self.stats.bytes_requested.saturating_add(length);
        let request = Request {
            offset,
            length,
            suffix: false,
            validator: self.validator.as_deref(),
            read_ahead: read_ahead.min(length.max(1)),
        };
        Ok((self.host.start(&request), length))
    }

    /// Checks a response to a request for `[offset, offset + length)`;
    /// returns the end of the bytes it holds and whether it is the whole
    /// object. The first response pins the size and the validator.
    fn accept(
        &mut self,
        response: &Response,
        offset: u64,
        length: u64,
        first: bool,
    ) -> Result<(u64, bool), Error> {
        if response.offset != offset && !response.whole {
            return Err(Error::Refused(format!(
                "asked for bytes from {offset}, the response starts at {}",
                response.offset
            )));
        }
        if first && !self.pinned {
            self.size = response.size;
            self.validator.clone_from(&response.validator);
            self.pinned = true;
            self.first_wait = response.wait;
        } else {
            if response.size != self.size {
                return Err(Error::Changed(format!(
                    "the object's size changed from {} to {}",
                    self.size, response.size
                )));
            }
            if let (Some(pinned), Some(seen)) = (&self.validator, &response.validator) {
                if pinned != seen {
                    return Err(Error::Changed("its validator changed".to_owned()));
                }
            }
        }
        let end = if response.whole {
            response.size
        } else {
            offset
                .saturating_add(length)
                .min(response.size)
                .min(response.end.unwrap_or(u64::MAX))
        };
        if end <= offset && !(response.size == 0 && offset == 0) {
            return Err(Error::Refused(format!(
                "the response for bytes from {offset} holds none of them"
            )));
        }
        if response.whole && offset != 0 {
            // The server ignores ranges: only the whole object from byte 0
            // can be read.
            return Err(Error::Refused(
                "the server does not answer range requests".to_owned(),
            ));
        }
        Ok((end, response.whole))
    }

    /// A response at `offset` of up to `span` bytes, opened now.
    fn request(&mut self, offset: u64, span: u64, first: bool) -> Result<Body<H::Handle>, Error> {
        let (mut handle, length) = self.begin(offset, span, CURSOR_READ_AHEAD)?;
        let response = match self.host.response(&mut handle) {
            Ok(response) => response,
            Err(error) => {
                self.host.cancel(handle);
                return Err(error.into());
            }
        };
        match self.accept(&response, offset, length, first) {
            Ok((end, whole)) => Ok(Body {
                handle: Some(handle),
                start: offset,
                pos: offset,
                end,
                whole,
            }),
            Err(error) => {
                self.host.cancel(handle);
                Err(error)
            }
        }
    }

    fn count_received(&mut self, n: usize) -> Result<(), Error> {
        self.stats.bytes_received = self.stats.bytes_received.saturating_add(n as u64);
        if self.stats.bytes_received > self.settings.max_bytes {
            return Err(Error::ByteLimit(self.settings.max_bytes));
        }
        Ok(())
    }

    fn read_once(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        if self.host.kept(offset, buf) {
            return Ok(());
        }
        let length = buf.len() as u64;
        let end = offset.saturating_add(length);
        // Kept bytes at the start of the read (copy windows refill from a
        // sample's start, a little before where the last read ended).
        let mut at = offset.saturating_add(self.recent.copy_prefix(offset, buf) as u64);
        while at < end {
            if let Some(p) = self.pipeline_for(at) {
                at = self.pipeline_copy(p, at, end, buf, offset)?;
                continue;
            }
            if let Some(c) = self.pick(at) {
                if c != 0 {
                    let cursor = self.cursors.remove(c);
                    self.cursors.insert(0, cursor);
                }
                at = self.copy_cursor(at, end, buf, offset)?;
                continue;
            }
            // Reading on past the end of the latest response, once it has
            // been read (to within `ONWARD` of its end), starts a pipeline;
            // anything else opens a response at the new position. A jump past
            // a response still mostly unread (a probe reading an index at
            // the end of a small file) keeps that response open.
            // While advice of regions is being read, a read outside them
            // ahead of its reader (one that runs on past a region) asks for
            // its own bytes only. A read behind it takes the usual path: if
            // the advice was never ended, a later pass over the same bytes
            // then costs one bounded pipeline, not a request per read.
            let sparse = self
                .forward
                .iter()
                .any(|p| p.sparse() && at >= p.ahead_from && at <= p.end);
            let onward = !sparse
                && self.cursors.first().is_some_and(|l| {
                    at >= l.end && at - l.end <= self.settings.max_skip && l.end - l.pos <= ONWARD
                });
            if onward {
                self.cancel_cursor(0);
                let keep = if self.cursors.is_empty() {
                    None
                } else {
                    Some(0)
                };
                self.start_pipeline(at, self.size, keep, false);
                continue;
            }
            let span = if sparse {
                end - at
            } else {
                self.settings.first_span.max(end - at)
            };
            let body = self.request(at, span, false)?;
            self.cursors.insert(0, body);
            let open = if self.forward.is_empty() {
                self.settings.open_bodies
            } else {
                1
            };
            while self.cursors.len() > open {
                self.cancel_cursor(self.cursors.len() - 1);
            }
        }
        Ok(())
    }

    /// The open response that serves a read at `at` with the smallest gap.
    fn pick(&self, at: u64) -> Option<usize> {
        let mut best: Option<(usize, u64)> = None;
        for (i, body) in self.cursors.iter().enumerate() {
            if body.handle.is_none() || at < body.pos || at >= body.end {
                continue;
            }
            let gap = at - body.pos;
            if !body.whole && gap > self.settings.max_skip {
                continue;
            }
            if best.is_none_or(|(_, g)| gap < g) {
                best = Some((i, gap));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Copies `[at, end)` from the first cursor, reading through any gap;
    /// returns where it stopped.
    fn copy_cursor(
        &mut self,
        mut at: u64,
        end: u64,
        buf: &mut [u8],
        base: u64,
    ) -> Result<u64, Error> {
        let mut scratch = Vec::new();
        loop {
            let Some(body) = self.cursors.first_mut() else {
                return Ok(at);
            };
            if at >= end || body.pos >= body.end {
                break;
            }
            let start = body.pos;
            let wanted = if start < at { at - start } else { end - start };
            let n = usize::try_from(wanted.min(body.end - start).min(CHUNK)).unwrap_or(usize::MAX);
            let Some(mut handle) = body.handle.take() else {
                break;
            };
            let direct = start >= at;
            let target = if direct {
                let into = usize::try_from(start - base).unwrap_or(usize::MAX);
                buf.get_mut(into..into.saturating_add(n))
            } else {
                scratch.resize(n, 0);
                scratch.get_mut(..n)
            };
            let Some(target) = target else {
                self.host.cancel(handle);
                return Err(Error::Failed("a read does not fit its buffer".to_owned()));
            };
            let got = match self.host.read(&mut handle, target) {
                Ok(got) => got,
                Err(error) => {
                    self.host.cancel(handle);
                    return Err(error.into());
                }
            };
            if let Some(read) = target.get(..got) {
                self.recent.keep(start, read);
                self.host.keep(start, read);
            }
            if got == 0 {
                self.host.cancel(handle);
                return Err(Error::Failed(
                    "the response ended before the requested range".to_owned(),
                ));
            }
            if let Some(body) = self.cursors.first_mut() {
                body.handle = Some(handle);
                body.pos = start.saturating_add(got as u64);
            }
            self.count_received(got)?;
            at = at.max(start.saturating_add(got as u64));
        }
        if let Some(body) = self.cursors.first_mut() {
            if body.pos >= body.end {
                // Used up: release its connection, and keep its end as the
                // latest position, so that reading on starts a pipeline.
                if let Some(handle) = body.handle.take() {
                    self.host.cancel(handle);
                }
                body.start = body.end;
                body.pos = body.end;
            }
        }
        Ok(at.min(end))
    }

    /// The pipeline a read at `at` belongs to: one whose requested bytes
    /// hold it, the one whose reader is nearest; else one whose frontier is
    /// within `max_skip`. Moved to the front.
    fn pipeline_for(&mut self, at: u64) -> Option<usize> {
        let mut best: Option<(usize, u64)> = None;
        for (i, p) in self.forward.iter().enumerate() {
            // A pipeline over advised regions holds every read in its spans
            // (it jumps to one it has not requested yet, `seek`).
            let reaches = at >= p.start()
                && at < p.end
                && if p.sparse() {
                    p.stop(at).is_some()
                } else {
                    at.saturating_sub(p.position.max(p.next)) <= self.settings.max_skip
                };
            if !reaches {
                continue;
            }
            let held = at < p.next;
            let score = (if held { 0 } else { self.size }).saturating_add(at.abs_diff(p.position));
            if best.is_none_or(|(_, s)| score < s) {
                best = Some((i, score));
            }
        }
        let (index, _) = best?;
        if index != 0 {
            let pipeline = self.forward.remove(index);
            self.forward.insert(0, pipeline);
        }
        Some(0)
    }

    /// Starts a pipeline over `[start, end)`. Every open response but `keep`
    /// is ended, so that the pipeline has the connections.
    fn start_pipeline(&mut self, start: u64, end: u64, keep: Option<usize>, replace: bool) {
        self.begin_pipeline(start, end, keep, replace, Vec::new(), None);
    }

    /// [`Self::start_pipeline`], requesting only `spans` if there are any,
    /// in segments of `segment` bytes if given.
    fn begin_pipeline(
        &mut self,
        start: u64,
        end: u64,
        keep: Option<usize>,
        replace: bool,
        spans: Vec<(u64, u64)>,
        segment: Option<u64>,
    ) {
        if replace {
            for pipeline in std::mem::take(&mut self.forward) {
                self.cancel_segments(pipeline.segments);
            }
        }
        while self.forward.len() >= self.settings.max_pipelines {
            if let Some(pipeline) = self.forward.pop() {
                self.cancel_segments(pipeline.segments);
            }
        }
        let kept = keep.and_then(|k| (k < self.cursors.len()).then(|| self.cursors.remove(k)));
        for body in std::mem::take(&mut self.cursors) {
            if let Some(handle) = body.handle {
                self.host.cancel(handle);
            }
        }
        self.cursors.extend(kept);
        let left = u64::from(
            self.settings
                .max_requests
                .saturating_sub(self.stats.requests)
                .saturating_sub(2)
                .max(1),
        );
        let segment = segment.unwrap_or_else(|| {
            self.settings
                .segment_bytes
                .max(end.saturating_sub(start).div_ceil(left))
        });
        // The pipelines share the connections and the memory.
        let count = self.forward.len().saturating_add(1);
        let share = (self.settings.parallel / count).max(1);
        let cap = MIB.max(self.settings.buffer_bytes / (share as u64).saturating_mul(count as u64));
        for p in &mut self.forward {
            p.parallel = share;
            p.read_ahead = cap;
        }
        self.stats.pipelines = self.stats.pipelines.saturating_add(1);
        self.forward.insert(
            0,
            Pipeline {
                end,
                segment_bytes: segment,
                parallel: share,
                read_ahead: cap,
                segments: VecDeque::new(),
                next: start,
                position: start,
                ahead_from: start,
                spans,
                span: 0,
            },
        );
        self.fill(0);
    }

    /// Starts segments until `parallel` are ahead of the reader.
    fn fill(&mut self, p: usize) {
        loop {
            let Some(pipeline) = self.forward.get_mut(p) else {
                return;
            };
            pipeline.skip_gaps();
            let pipeline = &*pipeline;
            let ahead = pipeline
                .segments
                .iter()
                .filter(|s| s.end > pipeline.position)
                .count();
            if ahead >= pipeline.parallel
                || pipeline.next >= pipeline.end
                || self.stats.requests.saturating_add(1) >= self.settings.max_requests
            {
                return;
            }
            let start = pipeline.next;
            let end = pipeline
                .stop(start)
                .unwrap_or(pipeline.end)
                .min(start.saturating_add(pipeline.segment_bytes));
            let read_ahead = pipeline.read_ahead;
            let Ok((handle, _)) = self.begin(start, end - start, read_ahead) else {
                return;
            };
            if let Some(pipeline) = self.forward.get_mut(p) {
                pipeline.segments.push_back(Segment {
                    start,
                    end,
                    handle: Some(handle),
                    opened: false,
                    body_end: start,
                    received: 0,
                    pieces: VecDeque::new(),
                });
                pipeline.next = end;
            }
        }
    }

    /// Moves pipeline `p`'s reader forward, drops what is too far behind,
    /// starts segments.
    fn advance(&mut self, p: usize, at: u64) {
        let keep_behind = self.settings.keep_behind;
        let mut dropped = VecDeque::new();
        let mut released = Vec::new();
        if let Some(pipeline) = self.forward.get_mut(p) {
            if at <= pipeline.position {
                return;
            }
            pipeline.position = at;
            pipeline.ahead_from = at;
            let keep = at.saturating_sub(keep_behind);
            let sparse = pipeline.sparse();
            for segment in &mut pipeline.segments {
                segment.trim(keep);
                // Over advised regions, a segment the reader has passed
                // without reading it to its end (a region it did not read)
                // releases its connection, so that the connections in use
                // stay those ahead of the reader; what has not arrived is
                // requested again if a read goes back for it (as after a
                // short response).
                if sparse && segment.end <= at && segment.arrived() < segment.end {
                    if let Some(handle) = segment.handle.take() {
                        released.push(handle);
                        segment.opened = true;
                        segment.body_end = segment.arrived();
                    }
                }
            }
            while pipeline.segments.front().is_some_and(|s| s.end <= keep) {
                dropped.extend(pipeline.segments.pop_front());
            }
        }
        for handle in released {
            self.host.cancel(handle);
        }
        self.cancel_segments(dropped);
        self.fill(p);
    }

    /// Restarts pipeline `p` at `at` if `at` is far past everything
    /// requested; over advised regions, if it is past everything requested
    /// at all, so that the spans in between are skipped, not requested.
    fn seek(&mut self, p: usize, at: u64) {
        let max_skip = self.settings.max_skip;
        let mut dropped = VecDeque::new();
        if let Some(pipeline) = self.forward.get_mut(p) {
            let reach = if pipeline.sparse() { 0 } else { max_skip };
            if at < pipeline.next.saturating_add(reach) {
                return;
            }
            dropped = std::mem::take(&mut pipeline.segments);
            pipeline.next = at;
            pipeline.position = at;
            pipeline.ahead_from = at;
            pipeline.span = pipeline.spans.partition_point(|&(_, end)| end <= at);
        }
        self.cancel_segments(dropped);
        self.fill(p);
    }

    /// Reads the next bytes of segment `s` of pipeline `p` from the host:
    /// the response head first, a request for the rest after a short
    /// response.
    fn pull(&mut self, p: usize, s: usize) -> Result<(), Error> {
        let (at, end, opened, body_end) = {
            let segment = self.segment(p, s)?;
            (
                segment.arrived(),
                segment.end,
                segment.opened,
                segment.body_end,
            )
        };
        if at >= end {
            return Err(Error::Failed(
                "a pipeline segment ended before its bytes".to_owned(),
            ));
        }
        let mut body_end = body_end;
        if !opened || at >= body_end {
            if opened {
                // A short response: request the rest.
                let old = self.segment_mut(p, s)?.handle.take();
                if let Some(old) = old {
                    self.host.cancel(old);
                }
                let read_ahead = self.forward.get(p).map_or(MIB, |q| q.read_ahead);
                let (handle, _) = self.begin(at, end - at, read_ahead)?;
                let segment = self.segment_mut(p, s)?;
                segment.handle = Some(handle);
                segment.opened = false;
            }
            let mut handle = self
                .segment_mut(p, s)?
                .handle
                .take()
                .ok_or_else(|| Error::Failed("a pipeline segment has no request".to_owned()))?;
            let accepted = match self.host.response(&mut handle) {
                Ok(response) => self.accept(&response, at, end - at, false),
                Err(error) => Err(error.into()),
            };
            match accepted {
                Ok((reached, _)) => {
                    body_end = reached;
                    let segment = self.segment_mut(p, s)?;
                    segment.handle = Some(handle);
                    segment.opened = true;
                    segment.body_end = reached;
                }
                Err(error) => {
                    self.host.cancel(handle);
                    return Err(error);
                }
            }
        }
        let n = CHUNK.min(body_end.saturating_sub(at)).min(end - at);
        let mut chunk = std::mem::take(&mut self.scratch);
        chunk.resize(usize::try_from(n).unwrap_or(usize::MAX), 0);
        let mut handle = self
            .segment_mut(p, s)?
            .handle
            .take()
            .ok_or_else(|| Error::Failed("a pipeline segment has no request".to_owned()))?;
        let read = self.host.read(&mut handle, &mut chunk);
        let got = match read {
            Ok(got) => got,
            Err(error) => {
                self.host.cancel(handle);
                return Err(error.into());
            }
        };
        if got == 0 {
            self.host.cancel(handle);
            self.scratch = chunk;
            return Err(Error::Failed(
                "the response ended before the requested range".to_owned(),
            ));
        }
        let received = chunk.get(..got).unwrap_or_default();
        self.host.keep(at, received);
        let segment = match self.forward.get_mut(p).and_then(|q| q.segments.get_mut(s)) {
            Some(segment) => segment,
            None => {
                self.host.cancel(handle);
                return Err(Error::Failed("a pipeline segment is gone".to_owned()));
            }
        };
        append(&mut segment.pieces, at, received);
        self.scratch = chunk;
        segment.received = segment.received.saturating_add(got as u64);
        if segment.arrived() >= segment.end {
            // Read to its end: release its connection now.
            self.host.cancel(handle);
        } else {
            segment.handle = Some(handle);
        }
        self.count_received(got)
    }

    fn segment(&self, p: usize, s: usize) -> Result<&Segment<H::Handle>, Error> {
        self.forward
            .get(p)
            .and_then(|q| q.segments.get(s))
            .ok_or_else(|| Error::Failed("a pipeline segment is gone".to_owned()))
    }

    fn segment_mut(&mut self, p: usize, s: usize) -> Result<&mut Segment<H::Handle>, Error> {
        self.forward
            .get_mut(p)
            .and_then(|q| q.segments.get_mut(s))
            .ok_or_else(|| Error::Failed("a pipeline segment is gone".to_owned()))
    }

    /// Copies `[at, min(to, end))` from pipeline `p`, reading the segments;
    /// returns where it stopped.
    fn pipeline_copy(
        &mut self,
        p: usize,
        mut at: u64,
        to: u64,
        buf: &mut [u8],
        base: u64,
    ) -> Result<u64, Error> {
        self.seek(p, at);
        let stop = to.min(self.forward.get(p).and_then(|q| q.stop(at)).unwrap_or(at));
        while at < stop {
            let s = match self.forward.get(p).and_then(|q| q.segment_at(at)) {
                Some(s) => s,
                None => {
                    // Not requested yet (a small gap past the frontier).
                    self.advance(p, at);
                    self.forward
                        .get(p)
                        .and_then(|q| q.segment_at(at))
                        .ok_or_else(|| {
                            Error::Failed("the pipeline cannot reach the read".to_owned())
                        })?
                }
            };
            let segment = self.segment(p, s)?;
            if at < segment.first() {
                return Err(Error::Failed(
                    "the pipeline dropped the bytes of a read".to_owned(),
                ));
            }
            if segment.arrived() <= at {
                self.pull(p, s)?;
                continue;
            }
            let till = segment.copy(at, stop.min(segment.end), buf, base);
            if till == at {
                return Err(Error::Failed(
                    "the pipeline lost the bytes of a read".to_owned(),
                ));
            }
            at = till;
            self.advance(p, at);
        }
        self.advance(p, at);
        Ok(at)
    }
}

impl<H: Host> Drop for Reader<H> {
    fn drop(&mut self) {
        self.drop_all();
    }
}
