import { RequestLimitError, SourceChangedError, SourceNotFoundError } from "./errors.js";

import { TRUSTED } from "./internal.js";

/** A response that breaks the range protocol; not retried. */
class ProtocolError extends Error {
  constructor(message) {
    super(message);
    this.name = "ProtocolError";
  }
}

/** Errors that a new request would not fix. */
function fatal(error) {
  return error instanceof SourceChangedError || error instanceof RequestLimitError || error instanceof ProtocolError || error instanceof SourceNotFoundError;
}

const KiB = 1024;

const MiB = 1024 * KiB;

/**
 * Defaults of the range reader (docs/design/wasm.md, "Sources").
 *
 * - A read at a new position opens a response of `firstSpan` bytes, read as
 *   a stream: a span bounds what a request may transfer, not what is held.
 *   A read up to `maxSkip` bytes ahead of an open response is served by
 *   reading through the gap rather than by a new request.
 * - Reading on past the end of a response, or the module's advice that a
 *   copy pass follows, starts a pipeline: `parallel` range requests of
 *   `segmentBytes` each, started early so that their round trips are paid
 *   before their bytes are needed, and handed out in order. Each is read
 *   into memory at most `bufferBytes / parallel` ahead of the reader; the
 *   rest waits in its connection. Segments grow for objects that would
 *   otherwise need more than `maxRequests` requests.
 * - The last `cacheBytes` received outside the pipeline are kept and serve
 *   reads that go back, such as the passes over the sample tables.
 * - `memoryBytes`, when not 0, bounds all the bytes the reader holds: those
 *   ahead of the reader, the 8 MiB kept behind each pipeline's reader and
 *   the cache. Where they add up to more, they are lowered to fit (`open`
 *   takes a lower bound for one session, for operations that need more
 *   memory of their own). 0 leaves their sum as the bound. Besides it, each
 *   open response holds the piece it is reading, at most 256 KiB.
 * - `maxRequests` caps one operation's requests, well below the subrequest
 *   limit of Workers' paid plan (set it to your plan's).
 * - A request or a body that fails is requested again from the failed
 *   read's offset, up to `retries` times per read (and for the first
 *   request), unless the failure is one a new request cannot fix.
 * - Advice that the reads touch only some regions (the headers of ProRes
 *   samples, the audio of an extraction) reads them in a pipeline that
 *   skips gaps of `sparseGap` bytes or more. Skipping a gap pays above
 *   round trip × bandwidth / `parallel` (docs/design/range-extraction.md);
 *   with `sparseGap: "auto"` the round trip is the wait for the session's
 *   first response and the bandwidth is `bandwidth` (bytes per second, an
 *   assumed value, not a measurement), the gap kept within 1 to 8 MiB.
 */
export const RANGE_DEFAULTS = Object.freeze({
  firstSpan: 4 * MiB,
  maxSkip: 8 * MiB,
  cacheBytes: 8 * MiB,
  parallel: 4,
  segmentBytes: 16 * MiB,
  bufferBytes: 24 * MiB,
  maxRequests: 1e3,
  retries: 2,
  memoryBytes: 0,
  sparseGap: "auto",
  bandwidth: 6e7
});

/** The least `memoryBytes` other than 0: room for a segment ahead, behind and in the cache. */
const MIN_MEMORY = 4 * MiB;

/**
 * `settings` with the bytes it may hold fitted to `memoryBytes` (0: no
 * bound beyond their sum), and the bytes each pipeline keeps behind its
 * reader as `keepBehind`. The parts are the buffer ahead (shared by the
 * pipelines), `keepBehind` for each of up to `MAX_PIPELINES` pipelines and
 * the cache. Where they add up to more than the bound, behind (both
 * pipelines together) and the cache each get at most a quarter of it, and
 * the buffer ahead the rest, never more than it was.
 */
function fitMemory(settings, memoryBytes) {
  const fitted = {
    ...settings,
    keepBehind: KEEP_BEHIND
  };
  const sum = fitted.bufferBytes + MAX_PIPELINES * fitted.keepBehind + fitted.cacheBytes;
  if (memoryBytes === 0 || sum <= memoryBytes) return fitted;
  fitted.keepBehind = Math.min(KEEP_BEHIND, Math.floor(memoryBytes / (4 * MAX_PIPELINES)));
  fitted.cacheBytes = Math.min(fitted.cacheBytes, Math.floor(memoryBytes / 4));
  fitted.bufferBytes = Math.min(fitted.bufferBytes, memoryBytes - MAX_PIPELINES * fitted.keepBehind - fitted.cacheBytes);
  return fitted;
}

/** The lower of two `memoryBytes` bounds, where 0 is none. */
function lowerBound(a, b) {
  if (a === 0) return b;
  if (b === 0) return a;
  return Math.min(a, b);
}

function checkMemory(value) {
  if (!Number.isSafeInteger(value) || value < 0 || value > 0 && value < MIN_MEMORY) {
    throw new TypeError(`range option memoryBytes must be 0 or at least ${MIN_MEMORY}`);
  }
}

/** An input already in memory: a `Uint8Array`, `ArrayBuffer` or view. */
export function bytesSource(data) {
  const bytes = toBytes(data);
  return {
    open() {
      return {
        [TRUSTED]: true,
        error: null,
        size: bytes.length,
        inMemory: true,
        readInto(offset, view) {
          view.set(bytes.subarray(offset, offset + view.length));
          return view.length;
        },
        stats: () => ({
          requests: 0,
          bytesRequested: 0,
          bytesReceived: 0
        }),
        close() {}
      };
    }
  };
}

function toBytes(data) {
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  throw new TypeError("bytesSource needs a Uint8Array, an ArrayBuffer or a view");
}

/**
 * A remote object read by byte ranges. `fetchRange({ offset, length,
 * validator, signal })` starts one request and resolves to `{ body, offset,
 * end, size, validator }`: `body` streams the bytes from `offset` (the
 * first byte the response holds, which must equal the requested offset)
 * for at most `length` bytes (or, with `whole: true`, the whole object
 * from byte 0); `end`, if given, is the offset just past the last byte it
 * holds, `size` is the object's length and `validator` its ETag, if any.
 * When `validator` is given, the request must fail with
 * `SourceChangedError` if the object no longer has it. `signal` aborts when
 * the session closes; a request still waiting should then fail.
 *
 * The returned source keeps the read statistics of its latest session in
 * `lastStats`.
 */
export function rangeSource(fetchRange, options = {}) {
  const settings = {
    ...RANGE_DEFAULTS,
    ...options
  };
  for (const [name, value] of Object.entries(settings)) {
    if (!(name in RANGE_DEFAULTS)) throw new TypeError(`unknown range option ${name}`);
    if (name === "sparseGap" && value === "auto") continue;
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new TypeError(`range option ${name} must be a non-negative integer`);
    }
  }
  if (settings.firstSpan < 1 || settings.segmentBytes < 1 || settings.parallel < 1) {
    throw new TypeError("range options need firstSpan, segmentBytes and parallel of at least 1");
  }
  checkMemory(settings.memoryBytes);
  const source = {
    lastStats: null,
    /**
         * Opens a session. `memoryBytes`, when given, lowers the source's own
         * bound for this session only.
         */
    async open({memoryBytes: memoryBytes = 0} = {}) {
      checkMemory(memoryBytes);
      const fitted = fitMemory(settings, lowerBound(settings.memoryBytes, memoryBytes));
      const session = new RangeSession(fetchRange, fitted, stats => {
        source.lastStats = stats;
      });
      await session.start();
      return session;
    }
  };
  return source;
}

/** One open response body, read front to back. */
class Body {
  constructor(stream, start, end, span, whole) {
    this.reader = stream.getReader();
    this.start = start;
    this.pos = start;
    this.end = end;
    this.span = span;
    this.whole = whole;
    this.chunk = null;
    this.at = 0;
    this.ended = false;
  }
  /** The next bytes of the body, at most `n`; advances past them. */
  async next(n) {
    if (this.chunk === null || this.at === this.chunk.length) {
      this.chunk = await this.pull(Math.min(n, JOIN_BYTES));
      this.at = 0;
    }
    const piece = this.chunk.subarray(this.at, this.at + Math.min(n, this.chunk.length - this.at));
    this.at += piece.length;
    this.pos += piece.length;
    return piece;
  }
  /**
     * The next chunk of the stream, or consecutive chunks joined up to
     * `wanted` bytes: an R2 binding's body arrives in 4 KiB chunks, and a
     * piece per chunk costs the reader more than the copy that joins them.
     * Joining stops at `wanted`, so a small read does not wait for more.
     */
  async pull(wanted) {
    const first = await this.chunkOf(await this.reader.read());
    if (first.length >= wanted) return first;
    const parts = [ first ];
    let total = first.length;
    while (total < wanted && this.pos + total < this.end && !this.ended) {
      const {done: done, value: value} = await this.reader.read();
      if (done) {
        this.ended = true;
        break;
      }
      parts.push(this.chunkOf({
        done: done,
        value: value
      }));
      total += value.length;
    }
    const joined = new Uint8Array(total);
    let at = 0;
    for (const part of parts) {
      joined.set(part, at);
      at += part.length;
    }
    return joined;
  }
  /** A read's chunk, checked. */
  chunkOf({done: done, value: value}) {
    if (done || this.ended) throw new Error("the response ended before the requested range");
    if (!(value instanceof Uint8Array)) throw new ProtocolError("a response body chunk is not bytes");
    return value;
  }
  cancel() {
    this.reader.cancel().catch(() => {});
  }
}

/** The most bytes `Body` joins from consecutive small chunks. */
const JOIN_BYTES = 256 * 1024;

/** Recently received bytes, kept up to a byte bound, oldest dropped first. */
class Recent {
  constructor(limit) {
    this.limit = limit;
    this.pieces = [];
    this.bytes = 0;
  }
  keep(start, bytes) {
    if (this.limit === 0 || bytes.length === 0) return;
    const kept = bytes.length * 2 < bytes.buffer.byteLength ? bytes.slice() : bytes;
    this.pieces.push({
      start: start,
      bytes: kept
    });
    this.bytes += bytes.length;
    while (this.bytes > this.limit) this.bytes -= this.pieces.shift().bytes.length;
  }
  /**
     * Copies the kept bytes at the start of [offset, end) into `out` (from its
     * index 0), as far as they reach without a gap; returns the count.
     */
  copyPrefix(offset, end, out) {
    let pos = offset;
    while (pos < end) {
      let found = null;
      for (let i = this.pieces.length - 1; i >= 0; i--) {
        const piece = this.pieces[i];
        if (piece.start <= pos && pos < piece.start + piece.bytes.length) {
          found = piece;
          break;
        }
      }
      if (found === null) break;
      const from = pos - found.start;
      const n = Math.min(end - pos, found.bytes.length - from);
      out.set(found.bytes.subarray(from, from + n), pos - offset);
      pos += n;
    }
    return pos - offset;
  }
  /** The bytes [offset, offset + length) if every one is kept, else null. */
  get(offset, length) {
    const end = offset + length;
    let out = null;
    let pos = offset;
    while (pos < end) {
      let found = null;
      for (let i = this.pieces.length - 1; i >= 0; i--) {
        const piece = this.pieces[i];
        if (piece.start <= pos && pos < piece.start + piece.bytes.length) {
          found = piece;
          break;
        }
      }
      if (found === null) return null;
      const from = pos - found.start;
      const n = Math.min(end - pos, found.bytes.length - from);
      if (out === null && n === length) return found.bytes.subarray(from, from + n);
      out ??= new Uint8Array(length);
      out.set(found.bytes.subarray(from, from + n), pos - offset);
      pos += n;
    }
    return out;
  }
}

/**
 * Open responses a session keeps besides its pipeline. Workers allow six
 * simultaneous connections per request and queue the rest until one
 * closes, so an idle response must never hold one that a pipeline waits
 * for: with a pipeline running, one other response stays open.
 */
const OPEN_BODIES = 3;

/** Connections Workers allow open at once per request (more are queued). */
const CONNECTIONS = 6;

/** One open response at a position in the object, read front to back. */
class Cursor {
  constructor(body) {
    this.body = body;
  }
  cancel() {
    this.body?.cancel();
    this.body = null;
  }
}

/** Bytes behind the reader that a pipeline keeps: copy windows refill from a sample's start, a little back. */
const KEEP_BEHIND = 8 * MiB;

/**
 * Pipelines a session runs at once: the tracks of a clip may be stored
 * tens of megabytes apart, and each is read forward, so each gets its own.
 */
const MAX_PIPELINES = 2;

/**
 * One segment of a pipeline: the bytes [start, end), requested early so
 * that its round trip is paid before its bytes are needed, and read into
 * memory at most `cap` bytes ahead of the reader. The rest waits in the
 * connection until the reader gets there. A response that holds less than
 * asked is followed by a request for the rest.
 */
class Segment {
  constructor(pipeline, start, end) {
    this.pipeline = pipeline;
    this.start = start;
    this.end = end;
    this.pieces = [];
    this.received = 0;
    this.done = false;
    this.error = null;
    this.cancelled = false;
    this.body = null;
    this.waiters = [];
    this.fill(pipeline.session);
  }
  /** Bytes received and not yet passed by the reader. */
  ahead() {
    return this.start + this.received - Math.max(this.pipeline.position, this.start);
  }
  async fill(session) {
    try {
      while (!this.cancelled && this.start + this.received < this.end) {
        const at = this.start + this.received;
        const body = await session.request(at, this.end - at);
        if (this.cancelled) {
          body.cancel();
          break;
        }
        this.body = body;
        while (!this.cancelled && body.pos < body.end) {
          while (!this.cancelled && this.ahead() >= this.pipeline.cap) await this.pipeline.progress();
          if (this.cancelled) break;
          const pieceAt = body.pos;
          const room = Math.max(1, this.pipeline.cap - this.ahead());
          const piece = await body.next(Math.min(body.end - body.pos, room));
          if (this.cancelled) break;
          if (pieceAt + piece.length > this.pipeline.position - session.settings.keepBehind) {
            this.pieces.push({
              at: pieceAt,
              bytes: piece
            });
          }
          this.received += piece.length;
          session.bytesReceived += piece.length;
          session.noteHeld();
          this.wake();
        }
        body.cancel();
        this.body = null;
      }
    } catch (error) {
      if (!this.cancelled) this.error = error;
    }
    this.done = true;
    this.wake();
    if (!this.cancelled) this.pipeline.fill();
  }
  wake() {
    const waiters = this.waiters;
    this.waiters = [];
    for (const resolve of waiters) resolve();
  }
  wait() {
    return new Promise(resolve => this.waiters.push(resolve));
  }
  /** The first kept byte. */
  first() {
    return this.pieces.length > 0 ? this.pieces[0].at : this.start + this.received;
  }
  /** Drops kept pieces that end at or before `before`. */
  trim(before) {
    let drop = 0;
    while (drop < this.pieces.length && this.pieces[drop].at + this.pieces[drop].bytes.length <= before) drop += 1;
    if (drop > 0) this.pieces.splice(0, drop);
  }
  /** Copies the received bytes of [at, to) into `out` at `at - base`; returns where it stopped. */
  copy(at, to, out, base) {
    const pieces = this.pieces;
    let low = 0;
    let high = pieces.length - 1;
    let index = -1;
    while (low <= high) {
      const middle = low + high >> 1;
      if (pieces[middle].at <= at) {
        index = middle;
        low = middle + 1;
      } else {
        high = middle - 1;
      }
    }
    if (index < 0) return at;
    while (index < pieces.length && at < to) {
      const {at: pieceAt, bytes: bytes} = pieces[index];
      const pieceEnd = pieceAt + bytes.length;
      if (pieceAt > at) break;
      if (pieceEnd > at) {
        const till = Math.min(to, pieceEnd);
        out.set(bytes.subarray(at - pieceAt, till - pieceAt), at - base);
        at = till;
      }
      index += 1;
    }
    return at;
  }
  cancel() {
    this.cancelled = true;
    this.body?.cancel();
    this.pieces = [];
    this.wake();
  }
}

/**
 * A forward region [start, end) read ahead by up to `parallel` segments at
 * once, handed out in order. Each segment holds at most `cap` bytes ahead
 * of the reader and `keepBehind` bytes behind it. A read more than
 * `maxSkip` beyond everything requested restarts the pipeline there.
 */
class Pipeline {
  /**
     * `spans`, when given, are the only bytes requested: sorted, disjoint
     * [start, end) pairs within [start, end) (advice of regions).
     */
  constructor(session, start, end, segment, parallel, cap, budget, spans = null) {
    this.session = session;
    this.end = end;
    this.segmentBytes = segment;
    this.parallel = parallel;
    this.cap = cap;
    this.budget = budget;
    this.spans = spans;
    this.span = 0;
    this.segments = [];
    this.next = start;
    this.position = start;
    this.aheadFrom = start;
    this.watchers = [];
    this.fill();
  }
  /**
     * Where a read at `at` must stop: the end of the pipeline, or of the span
     * that holds `at`; the reader serves the rest from elsewhere (a read that
     * runs on into a skipped gap).
     */
  limit(at) {
    if (this.spans === null) return this.end;
    const index = this.spanAt(at);
    return index >= 0 ? this.spans[index][1] : at;
  }
  /** The index of the span that holds `at`, or -1. */
  spanAt(at) {
    const spans = this.spans;
    let low = 0;
    let high = spans.length - 1;
    while (low <= high) {
      const middle = low + high >> 1;
      const [start, end] = spans[middle];
      if (at < start) high = middle - 1; else if (at >= end) low = middle + 1; else return middle;
    }
    return -1;
  }
  get start() {
    return this.segments.length > 0 ? this.segments[0].first() : this.next;
  }
  /** Whether a read at `at` belongs to the pipeline. */
  reaches(at) {
    if (this.spans !== null) return at >= this.start && this.spanAt(at) >= 0;
    return at >= this.start && at < this.end && at - Math.max(this.position, this.next) <= this.session.settings.maxSkip;
  }
  /** Resolves when the reader moves on. */
  progress() {
    return new Promise(resolve => this.watchers.push(resolve));
  }
  /** Moves the reader's position forward, trims what is too far behind, starts segments. */
  advance(at) {
    if (at <= this.position) return;
    this.position = at;
    this.aheadFrom = at;
    const keep = at - this.session.settings.keepBehind;
    for (const segment of this.segments) segment.trim(keep);
    while (this.segments.length > 0 && this.segments[0].end <= keep) this.segments.shift().cancel();
    const watchers = this.watchers;
    this.watchers = [];
    for (const resolve of watchers) resolve();
    this.fill();
  }
  /**
     * Starts segments: at most `parallel` ahead of the reader, read to their
     * end or still running, and downloads still running (also one the reader
     * has passed, which reads on to its end unstalled) at most `parallel`.
     * Against the pipeline's `budget` of the buffer count the bytes a segment
     * read to its end holds ahead of the reader, and for one still running
     * what it may still hold, at most `cap` (the rest waits in its
     * connection), so that `memoryBytes` holds.
     */
  fill() {
    const session = this.session;
    const budget = this.budget;
    let active = 0;
    let ahead = 0;
    let committed = 0;
    for (const segment of this.segments) {
      if (!segment.done) active += 1;
      if (segment.end > this.position) ahead += 1;
      committed += this.holds(segment);
    }
    let others = session.openResponses();
    for (const pipeline of session.forward) {
      if (pipeline !== this) others += pipeline.segments.filter(segment => !segment.done).length;
    }
    while (active < this.parallel && ahead < this.parallel && active + others < CONNECTIONS && this.next < this.end && !session.closed && session.requests + 1 < session.settings.maxRequests) {
      let limit = this.end;
      if (this.spans !== null) {
        const span = this.spans[this.span];
        if (span === undefined) break;
        if (this.next >= span[1]) {
          this.span += 1;
          continue;
        }
        if (this.next < span[0]) this.next = span[0];
        limit = span[1];
      }
      const end = Math.min(limit, this.next + this.segmentBytes);
      const holds = Math.min(this.cap, end - this.next);
      if (committed > 0 && committed + holds > budget) break;
      this.segments.push(new Segment(this, this.next, end));
      committed += holds;
      this.next = end;
      active += 1;
      ahead += 1;
    }
  }
  /** The bytes `segment` holds or may still hold ahead of the reader. */
  holds(segment) {
    if (segment.end <= this.position) return 0;
    const ahead = segment.end - Math.max(segment.start, this.position);
    return segment.done ? ahead : Math.min(this.cap, ahead);
  }
  /** Restarts at `at` if it is far past everything requested. */
  seek(at) {
    if (this.spans !== null) {
      if (at >= this.next && this.segmentAt(at) === null) this.restart(at);
      return;
    }
    if (at >= this.next + this.session.settings.maxSkip) this.restart(at);
  }
  /** Drops every segment and starts again at `at`. */
  restart(at) {
    this.cancel();
    this.next = at;
    this.position = at;
    this.aheadFrom = at;
    this.spanFrom(at);
    this.fill();
  }
  /**
     * Points `span` at the span that holds `at`, or at the first one after
     * it. Every move of `next` backwards goes through this: `fill()` reads
     * `spans[span]` and would otherwise skip the rest of an earlier span.
     */
  spanFrom(at) {
    if (this.spans === null) return;
    const index = this.spanAt(at);
    this.span = index >= 0 ? index : this.spans.findIndex(([start]) => start > at);
    if (this.span < 0) this.span = this.spans.length;
  }
  /**
     * Takes a smaller share of the connections and the memory (a second
     * pipeline started): segments beyond its share of downloads or of the
     * buffer that the reader has not reached are cancelled and requested
     * again when it gets there.
     */
  shrink(parallel, cap, budget) {
    this.parallel = parallel;
    this.cap = cap;
    this.budget = budget;
    let active = 0;
    let committed = 0;
    for (let i = 0; i < this.segments.length; i++) {
      const segment = this.segments[i];
      if (segment.end <= this.position) continue;
      if (!segment.done) active += 1;
      committed += this.holds(segment);
      if ((active > parallel || committed > budget) && segment.start > this.position) {
        this.next = segment.start;
        this.spanFrom(segment.start);
        for (const dropped of this.segments.splice(i)) dropped.cancel();
        break;
      }
    }
  }
  segmentAt(at) {
    return this.segments.find(segment => segment.start <= at && at < segment.end) ?? null;
  }
  /** Copies what has arrived of [at, to); returns where it stopped (no wait). */
  copyNow(at, to, out, base) {
    this.seek(at);
    to = Math.min(to, this.limit(at));
    while (at < to) {
      const segment = this.segmentAt(at);
      if (segment === null) break;
      const till = segment.copy(at, Math.min(to, segment.end), out, base);
      if (till === at) break;
      at = till;
    }
    this.advance(at);
    return at;
  }
  /** Copies [at, min(to, end)), waiting for the bytes; returns where it stopped. */
  async copy(at, to, out, base) {
    this.seek(at);
    const stop = Math.min(to, this.limit(at));
    while (at < stop) {
      let segment = this.segmentAt(at);
      if (segment === null) {
        this.advance(at);
        segment = this.segmentAt(at);
        if (segment === null) {
          this.restart(at);
          segment = this.segmentAt(at);
        }
        if (segment === null) throw new Error("the pipeline cannot reach the read");
      }
      if (at < segment.first()) throw new Error("the pipeline dropped the bytes of a read");
      let till = segment.copy(at, Math.min(stop, segment.end), out, base);
      while (till === at) {
        if (segment.error !== null) throw segment.error;
        if (segment.done) throw new Error("a pipeline segment ended before its bytes");
        this.advance(at);
        await segment.wait();
        till = segment.copy(at, Math.min(stop, segment.end), out, base);
      }
      at = till;
    }
    this.advance(at);
    return at;
  }
  cancel() {
    for (const segment of this.segments) segment.cancel();
    this.segments = [];
    const watchers = this.watchers;
    this.watchers = [];
    for (const resolve of watchers) resolve();
  }
}

class RangeSession {
  constructor(fetchRange, settings, report) {
    this.fetchRange = fetchRange;
    this.settings = settings;
    this.report = report;
    this.inMemory = false;
    this.size = undefined;
    this.validator = undefined;
    this.cursors = [];
    this.forward = [];
    this.recent = new Recent(settings.cacheBytes);
    this.requests = 0;
    this.pipelines = 0;
    this.bytesRequested = 0;
    this.bytesReceived = 0;
    this.maxHeldBytes = 0;
    this.closed = false;
    this.firstWait = undefined;
    this.aborts = new AbortController;
  }
  /** The first request, at offset 0, which also gives the size; retried as reads are. */
  async start() {
    let retries = this.settings.retries;
    for (;;) {
      try {
        this.cursors = [ new Cursor(await this.request(0, this.settings.firstSpan)) ];
        return;
      } catch (error) {
        if (fatal(error) || this.closed || retries === 0) throw error;
        retries -= 1;
      }
    }
  }
  /** Starts a request for up to `span` bytes from `offset`; resolves to its body. */
  async request(offset, span) {
    if (this.closed) throw new Error("the source session is closed");
    if (this.requests >= this.settings.maxRequests) {
      throw new RequestLimitError(this.settings.maxRequests);
    }
    const length = this.size === undefined ? span : Math.min(span, this.size - offset);
    this.requests += 1;
    this.bytesRequested += length;
    const started = performance.now();
    const response = await this.fetchRange({
      offset: offset,
      length: length,
      validator: this.validator,
      signal: this.aborts.signal
    });
    const waited = performance.now() - started;
    if (!response || !response.body) throw new ProtocolError("fetchRange resolved without a body");
    const {body: body, size: size, validator: validator} = response;
    const cancel = () => body.cancel().catch(() => {});
    if (response.offset !== offset) {
      cancel();
      throw new ProtocolError(`asked for bytes from ${offset}, the response starts at ${response.offset}`);
    }
    if (!Number.isSafeInteger(size) || size < 0 || this.size !== undefined && size !== this.size) {
      cancel();
      throw new SourceChangedError(`the object's size changed or is invalid (${size})`);
    }
    if (this.validator !== undefined && validator !== undefined && validator !== this.validator) {
      cancel();
      throw new SourceChangedError;
    }
    if (this.size === undefined) {
      this.size = size;
      this.validator = validator ?? undefined;
      this.firstWait = waited;
    }
    const whole = response.whole === true;
    const end = whole ? size : Math.min(offset + length, size, response.end ?? Number.POSITIVE_INFINITY);
    if (!(end > offset) && !(size === 0 && offset === 0)) {
      cancel();
      throw new ProtocolError(`the response for bytes from ${offset} holds none of them`);
    }
    if (this.closed) {
      cancel();
      throw new Error("the source session is closed");
    }
    return new Body(body, offset, end, span, whole);
  }
  dropBodies() {
    for (const cursor of this.cursors) cursor.cancel();
    this.cursors = [];
    for (const pipeline of this.forward) pipeline.cancel();
    this.forward = [];
  }
  /**
     * The pipeline a read at `at` belongs to, or null: one whose requested
     * bytes hold `at`, the one whose reader is nearest; else one whose
     * frontier is within `maxSkip`.
     */
  pipelineFor(at) {
    let best = null;
    let bestScore = Number.POSITIVE_INFINITY;
    for (const pipeline of this.forward) {
      if (!pipeline.reaches(at)) continue;
      const held = at < pipeline.next;
      const score = (held ? 0 : this.size) + Math.abs(at - pipeline.position);
      if (score < bestScore) {
        best = pipeline;
        bestScore = score;
      }
    }
    if (best !== null && this.forward[0] !== best) {
      this.forward.splice(this.forward.indexOf(best), 1);
      this.forward.unshift(best);
    }
    return best;
  }
  /**
     * Starts a pipeline over [start, end): `parallel` segments in flight,
     * sized so that the region fits the remaining requests, each read ahead
     * by at most `bufferBytes / parallel`. Every response but the one kept
     * closes, so that the pipeline's connections are free.
     */
  startPipeline(start, end, keep = this.cursors[0], replace = false, spans = null, spanSegment = null) {
    const {segmentBytes: segmentBytes, bufferBytes: bufferBytes, parallel: parallel, maxRequests: maxRequests} = this.settings;
    if (replace) {
      for (const pipeline of this.forward) pipeline.cancel();
      this.forward = [];
    }
    while (this.forward.length >= MAX_PIPELINES) this.forward.pop().cancel();
    for (const cursor of this.cursors) if (cursor !== keep) cursor.cancel();
    this.cursors = keep === undefined ? [] : [ keep ];
    const left = Math.max(1, maxRequests - this.requests - 2);
    const segment = spans !== null ? spanSegment ?? segmentBytes : Math.max(segmentBytes, Math.ceil((end - start) / left));
    const count = this.forward.length + 1;
    const share = Math.max(1, Math.floor(parallel / count));
    const budget = Math.max(1, Math.floor(bufferBytes / count));
    const cap = Math.min(budget, Math.max(MiB, Math.floor(bufferBytes / (share * count))));
    for (const pipeline of this.forward) pipeline.shrink(share, cap, budget);
    this.pipelines += 1;
    this.forward.unshift(new Pipeline(this, start, end, segment, share, cap, budget, spans));
  }
  /**
     * Advice from the module that the reads that follow touch only these
     * regions, offset and length pairs in increasing order: a scan of each
     * sample's header (ProRes), or the reads of an audio extraction's copy
     * pass. One pipeline reads them, joining regions whose gaps are under
     * `sparseGap` (cheaper to read through than a round trip) and skipping
     * the larger gaps, in at most half the requests left; where they would
     * need more, gaps double until they fit. An open response serves the
     * spans it reaches (`afterOpen`). An empty list ends the advice: its
     * pipeline is dropped and reads go anywhere again.
     */
  planRegions(flat) {
    if (this.closed || this.size === undefined) return;
    if (flat.length === 0) {
      for (const pipeline of this.forward) if (pipeline.spans !== null) pipeline.cancel();
      this.forward = this.forward.filter(pipeline => pipeline.spans === null);
      return;
    }
    if (this.cursors.some(cursor => cursor.body?.whole)) return;
    const regions = [];
    for (let i = 0; i + 1 < flat.length; i += 2) {
      const start = flat[i];
      const end = Math.min(this.size, start + flat[i + 1]);
      if (start >= 0 && start < end) regions.push([ start, end ]);
    }
    if (regions.length === 0) return;
    regions.sort((a, b) => a[0] - b[0]);
    const {segmentBytes: segmentBytes, maxRequests: maxRequests} = this.settings;
    const budget = Math.floor((maxRequests - this.requests) / 2);
    if (budget < 1) return;
    for (let gap = Math.max(1, this.sparseGap()); ;gap *= 2) {
      const spans = [];
      for (const [start, end] of regions) {
        const last = spans.at(-1);
        if (last !== undefined && start - last[1] < gap) last[1] = Math.max(last[1], end); else spans.push([ start, end ]);
      }
      const bytes = spans.reduce((n, [start, end]) => n + (end - start), 0);
      const segment = Math.max(segmentBytes, Math.ceil(bytes / budget));
      const requests = spans.reduce((n, [start, end]) => n + Math.ceil((end - start) / segment), 0);
      if (requests <= budget) {
        const {rest: rest, keep: keep} = this.afterOpen(spans);
        if (rest.length === 0) {
          this.planRegions([]);
          return;
        }
        this.startPipeline(rest[0][0], rest.at(-1)[1], keep ?? this.cursors[0], true, rest, segment);
        this.forward[0].aheadFrom = spans[0][0];
        return;
      }
      if (spans.length === 1) return;
    }
  }
  /**
     * The spans an open response does not already serve, and that response
     * (as `plan` starts after what an open response holds). A response that
     * holds the first span's start, read or still to come within `maxSkip`,
     * serves the spans as far as its reads reach, up to the first gap it
     * could not read through; the pipeline requests the rest. A small
     * object's first response may hold much of it.
     */
  afterOpen(spans) {
    const first = spans[0][0];
    const {maxSkip: maxSkip} = this.settings;
    let keep;
    let best = -1;
    for (const cursor of this.cursors) {
      const body = cursor.body;
      if (body === null || first < body.start || first >= body.end) continue;
      if (first - body.pos > maxSkip) continue;
      let reach = Math.max(body.pos, first);
      for (const [start, end] of spans) {
        if (start >= body.end || start - reach > maxSkip) break;
        reach = Math.min(end, body.end);
      }
      if (reach > best) {
        best = reach;
        keep = cursor;
      }
    }
    if (keep === undefined) return {
      rest: spans,
      keep: keep
    };
    const rest = spans.filter(([, end]) => end > best).map(([start, end]) => [ Math.max(start, best), end ]);
    return {
      rest: rest,
      keep: keep
    };
  }
  /**
     * The gap between advised regions under which they are read through:
     * `sparseGap`, or with `"auto"`, the first response's wait × `bandwidth`
     * / `parallel`, kept within 1 to 8 MiB (4 MiB before any response).
     */
  sparseGap() {
    const {sparseGap: sparseGap, bandwidth: bandwidth, parallel: parallel} = this.settings;
    if (sparseGap !== "auto") return sparseGap;
    if (this.firstWait === undefined) return 4 * MiB;
    const gap = Math.floor(this.firstWait / 1e3 * bandwidth / Math.max(1, parallel));
    return Math.min(8 * MiB, Math.max(MiB, gap));
  }
  /**
     * Advice from the module that reads of about `length` bytes from `offset`
     * follow: a writer's copy pass is about to start. A pipeline starts now
     * over them (a sixteenth more, at least one segment, for samples of other
     * tracks between them), after the part an open response already holds.
     */
  plan(offset, length) {
    if (this.closed || this.size === undefined || !(offset >= 0 && offset < this.size)) return;
    if (this.requests + 1 >= this.settings.maxRequests) return;
    if (this.cursors.some(cursor => cursor.body?.whole)) return;
    const end = Math.min(this.size, offset + length + Math.max(Math.ceil(length / 16), this.settings.segmentBytes));
    let start = offset;
    let keep;
    for (const cursor of this.cursors) {
      const body = cursor.body;
      if (body === null || offset < body.start || offset >= body.end) continue;
      if (offset - body.pos > this.settings.maxSkip) continue;
      if (body.end > start) {
        start = body.end;
        keep = cursor;
      }
    }
    const current = this.pipelineFor(start);
    if (current !== null && current.end >= end) return;
    if (start < end) this.startPipeline(start, end, keep ?? this.cursors[0], true);
  }
  async read(offset, length) {
    let retries = this.settings.retries;
    for (;;) {
      try {
        return await this.readOnce(offset, length);
      } catch (error) {
        this.dropBodies();
        if (fatal(error) || this.closed || retries === 0) throw error;
        retries -= 1;
      }
    }
  }
  /**
     * Copies [offset, offset + view.length) into `view` if every byte is
     * here now (kept, or arrived in the pipeline); returns the count, or 0.
     */
  readNow(offset, view) {
    if (this.closed) return 0;
    const length = view.length;
    const kept = this.recent.get(offset, length);
    if (kept !== null) {
      view.set(kept);
      return length;
    }
    const pipeline = this.pipelineFor(offset);
    if (pipeline === null) return 0;
    const at = pipeline.copyNow(offset, offset + length, view, offset);
    return at === offset + length ? length : 0;
  }
  /** The open response that serves a read at `at` with the smallest gap, or null. */
  pick(at) {
    let best = null;
    for (const cursor of this.cursors) {
      const body = cursor.body;
      if (body === null || at < body.pos || at >= body.end) continue;
      if (!body.whole && at - body.pos > this.settings.maxSkip) continue;
      if (best === null || at - body.pos < at - best.body.pos) best = cursor;
    }
    return best;
  }
  async readOnce(offset, length) {
    const kept = this.recent.get(offset, length);
    if (kept !== null) return kept;
    const end = offset + length;
    const out = new Uint8Array(length);
    let at = offset + this.recent.copyPrefix(offset, end, out);
    while (at < end) {
      if (this.closed) throw new Error("the source session is closed");
      const pipeline = this.pipelineFor(at);
      if (pipeline !== null) {
        at = await pipeline.copy(at, end, out, offset);
        continue;
      }
      const cursor = this.pick(at);
      if (cursor !== null) {
        if (this.cursors[0] !== cursor) {
          this.cursors.splice(this.cursors.indexOf(cursor), 1);
          this.cursors.unshift(cursor);
        }
        at = await this.fromCursor(cursor, at, end, out, offset);
        continue;
      }
      const sparse = this.forward.some(pipeline => pipeline.spans !== null && at >= pipeline.aheadFrom && at <= pipeline.end);
      const latest = this.cursors[0]?.body ?? null;
      if (!sparse && latest !== null && at >= latest.end && at - latest.end <= this.settings.maxSkip) {
        this.cursors.shift().cancel();
        this.startPipeline(at, this.size);
        continue;
      }
      const room = (this.forward.length === 0 ? OPEN_BODIES : 1) - 1;
      while (this.cursors.length > room) this.cursors.pop().cancel();
      this.makeRoomForResponse();
      const cursor2 = new Cursor(await this.request(at, sparse ? end - at : Math.max(this.settings.firstSpan, end - at)));
      this.cursors.unshift(cursor2);
    }
    return out;
  }
  /** Copies [at, end) from a response, reading through any gap; returns where it stopped. */
  async fromCursor(cursor, at, end, out, base) {
    const body = cursor.body;
    while (at < end && body.pos < body.end) {
      const start = body.pos;
      const wanted = body.pos < at ? at - body.pos : end - body.pos;
      const piece = await body.next(Math.min(wanted, body.end - body.pos));
      if (cursor.body !== body) throw new Error("the source session is closed");
      this.bytesReceived += piece.length;
      this.recent.keep(start, piece);
      this.noteHeld();
      if (start + piece.length <= at) continue;
      const from = Math.max(at, start);
      out.set(piece.subarray(from - start), from - base);
      at = start + piece.length;
    }
    if (body.pos === body.end) {
      body.cancel();
      const index = this.cursors.indexOf(cursor);
      if (index >= 0) this.cursors.splice(index, 1);
      this.cursors.unshift(new Cursor(new EndOfBody(body.end)));
      for (const pipeline of this.forward) pipeline.fill();
    }
    return Math.min(at, end);
  }
  stats() {
    return {
      requests: this.requests,
      pipelines: this.pipelines,
      bytesRequested: this.bytesRequested,
      bytesReceived: this.bytesReceived,
      maxHeldBytes: this.maxHeldBytes
    };
  }
  /** Responses open outside the pipelines (a used-up one holds no connection). */
  openResponses() {
    return this.cursors.filter(cursor => cursor.body instanceof Body).length;
  }
  /**
     * Workers allow `CONNECTIONS` open at once and queue the rest. Before a
     * response opens while pipelines run, they give up a connection if theirs
     * and the open responses would take every one: a segment stalled at its
     * cap would otherwise hold a connection the new response waits for. The
     * farthest segment the reader has not reached is cancelled and requested
     * again when it gets there.
     */
  makeRoomForResponse() {
    for (;;) {
      let running = 0;
      for (const pipeline of this.forward) running += pipeline.segments.filter(segment => !segment.done).length;
      if (running + this.openResponses() + 1 <= CONNECTIONS) return;
      const pipeline = this.forward.find(candidate => candidate.parallel > 1);
      if (pipeline === undefined) return;
      pipeline.shrink(pipeline.parallel - 1, pipeline.cap, pipeline.budget);
      const after = this.forward.reduce((n, p) => n + p.segments.filter(segment => !segment.done).length, 0);
      if (after >= running) return;
    }
  }
  /** Notes the bytes held now (`stats().maxHeldBytes`). */
  noteHeld() {
    let held = this.recent.bytes;
    for (const pipeline of this.forward) {
      for (const segment of pipeline.segments) {
        for (const piece of segment.pieces) held += piece.bytes.length;
      }
    }
    if (held > this.maxHeldBytes) this.maxHeldBytes = held;
  }
  close() {
    if (this.closed) return;
    this.closed = true;
    this.aborts.abort(new Error("the source session is closed"));
    this.dropBodies();
    this.report(this.stats());
  }
}

/** The place where a used-up response ended: no bytes, no connection. */
class EndOfBody {
  constructor(end) {
    this.start = end;
    this.pos = end;
    this.end = end;
    this.whole = false;
  }
  cancel() {}
}

const CONTENT_RANGE = /^bytes (\d+)-(\d+)\/(\d+)$/;

/**
 * An object behind an HTTP(S) URL that answers range requests, such as a
 * presigned object-storage URL. `options.headers` are sent with every
 * request, `options.fetch` replaces the global `fetch`, and redirects are
 * refused unless `options.followRedirects` is set. At most five redirects
 * are followed, with no HTTPS downgrade; caller headers are dropped for
 * the rest of the chain after its first cross-origin hop. Only identity
 * encoding is accepted, so headers and body describe the same bytes.
 * Other options are those of `rangeSource`; maxRequests counts reader
 * requests, not redirect hops. A strong ETag is pinned with `If-Match`;
 * without one, only the object's size is checked between requests.
 */
export function httpSource(url, options = {}) {
  let target;
  try { target = new URL(url); }
  catch { throw new TypeError("HTTP sources need a valid URL"); }
  if (target.username || target.password) throw new TypeError("HTTP source URLs must not carry credentials");
  if (target.protocol !== "http:" && target.protocol !== "https:") throw new TypeError("HTTP sources need an http or https URL");
  const {headers: headers = {}, fetch: fetchImpl = globalThis.fetch, followRedirects: followRedirects = false, ...range} = options;
  const callerHeaders = new Headers(headers);
  for (const name of ["range", "if-match", "accept-encoding"]) callerHeaders.delete(name);
  return rangeSource(async ({offset: offset, length: length, validator: validator, signal: signal}) => {
    let current = target;
    let includeCallerHeaders = true;
    let response;
    for (let hop = 0; hop <= 5; hop += 1) {
      const request = includeCallerHeaders ? new Headers(callerHeaders) : new Headers();
      request.set("accept-encoding", "identity");
      request.set("range", `bytes=${offset}-${offset + Math.max(length, 1) - 1}`);
      if (validator !== undefined) request.set("if-match", validator);
      response = await fetchImpl(current, { headers: request, redirect: "manual", signal: signal });
      if (response.type !== "opaqueredirect" && !(response.status >= 300 && response.status < 400)) break;
      await response.body?.cancel();
      if (!followRedirects) throw new ProtocolError("the server redirected the request (followRedirects is off)");
      if (response.type === "opaqueredirect" || ![301, 302, 303, 307, 308].includes(response.status)) throw new ProtocolError("the server returned an unsupported redirect");
      if (hop === 5) throw new ProtocolError("the server redirected too many times");
      const location = response.headers.get("location");
      if (location === null) throw new ProtocolError("the redirect has no Location");
      let next;
      try { next = new URL(location, current); }
      catch { throw new ProtocolError("the redirect target is invalid"); }
      if (next.username || next.password) throw new ProtocolError("the redirect target must not carry credentials");
      if (next.protocol !== "http:" && next.protocol !== "https:") throw new ProtocolError("the redirect target must use http or https");
      if (current.protocol === "https:" && next.protocol === "http:") throw new ProtocolError("the redirect must not downgrade https");
      if (next.origin !== current.origin) includeCallerHeaders = false;
      current = next;
    }
    const drop = () => response.body?.cancel().catch(() => {});
    const encoding = response.headers.get("content-encoding");
    if (encoding !== null && encoding.trim().toLowerCase() !== "identity") {
      drop();
      throw new ProtocolError("compressed HTTP responses do not preserve byte ranges (Content-Encoding must be identity)");
    }
    if (response.status === 412) {
      drop();
      throw new SourceChangedError;
    }
    const etag = response.headers.get("etag");
    const strong = etag !== null && !etag.startsWith("W/") ? etag : undefined;
    if (response.status === 206) {
      const match = CONTENT_RANGE.exec(response.headers.get("content-range") ?? "");
      if (match === null) {
        drop();
        throw new ProtocolError("a 206 response without a usable Content-Range");
      }
      return {
        body: response.body,
        offset: Number(match[1]),
        end: Number(match[2]) + 1,
        size: Number(match[3]),
        validator: strong
      };
    }
    if (response.status === 416 && offset === 0) {
      drop();
      return {
        body: new ReadableStream({
          start: c => c.close()
        }),
        offset: 0,
        size: 0,
        validator: strong
      };
    }
    if (response.status === 200 && offset === 0) {
      const declared = response.headers.get("content-length");
      const size = declared === null ? Number.NaN : Number(declared);
      if (!Number.isSafeInteger(size)) {
        drop();
        throw new ProtocolError("a 200 response without a Content-Length");
      }
      return {
        body: response.body,
        offset: 0,
        size: size,
        validator: strong,
        whole: true
      };
    }
    drop();
    if (response.status === 200) throw new ProtocolError("the server does not answer range requests");
    if (response.status === 404) throw new SourceNotFoundError;
    const error = new Error(`HTTP ${response.status} for a range request`);
    throw response.status < 500 ? new ProtocolError(error.message) : error;
  }, range);
}
