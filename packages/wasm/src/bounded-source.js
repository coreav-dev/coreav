import {
  CoreAVError,
  RequestLimitError,
  SourceChangedError,
} from "./errors.js";
import { TRUSTED } from "./internal.js";

const PARTITION = 4 * 1024 * 1024;
const CHUNK = 256 * 1024;
const BUFFER_BYTES = Object.getOwnPropertyDescriptor(
  ArrayBuffer.prototype,
  "byteLength",
).get;
const RESIZABLE = Object.getOwnPropertyDescriptor(
  ArrayBuffer.prototype,
  "resizable",
)?.get;
const TYPED = Object.getPrototypeOf(Uint8Array.prototype);
const TYPED_BUFFER = Object.getOwnPropertyDescriptor(TYPED, "buffer").get;
const TYPED_OFFSET = Object.getOwnPropertyDescriptor(TYPED, "byteOffset").get;
const TYPED_BYTES = Object.getOwnPropertyDescriptor(TYPED, "byteLength").get;
const STREAM_LOCKED = Object.getOwnPropertyDescriptor(
  ReadableStream.prototype,
  "locked",
).get;
const GET_READER = ReadableStream.prototype.getReader;
const BYOB_READ = ReadableStreamBYOBReader.prototype.read;
const READER_CANCEL = ReadableStreamBYOBReader.prototype.cancel;
const STREAM_CANCEL = ReadableStream.prototype.cancel;

export const boundedOpeners = new WeakMap();

export class SourceCleanupError extends CoreAVError {
  constructor(cause) {
    super(
      {
        schema: "coreav.error/1",
        kind: "io",
        limit: null,
        offset: null,
        message: "the source could not be closed",
      },
      { cause },
    );
  }
}

export function hostLimit(message) {
  return new CoreAVError({
    schema: "coreav.error/1",
    kind: "limit",
    limit: "thumbnailHostBytes",
    offset: null,
    message,
  });
}

export function boundedMemorySession(bytes) {
  const buffer = TYPED_BUFFER.call(bytes);
  let capacity;
  try {
    capacity = BUFFER_BYTES.call(buffer);
    if (RESIZABLE?.call(buffer))
      throw hostLimit("bounded memory sources require a fixed ArrayBuffer");
  } catch (error) {
    if (error instanceof CoreAVError) throw error;
    throw hostLimit("bounded memory sources require a fixed ArrayBuffer");
  }
  if (capacity > PARTITION)
    throw hostLimit("the in-memory source backing buffer exceeds 4 MiB");
  const view = new Uint8Array(
    buffer,
    TYPED_OFFSET.call(bytes),
    TYPED_BYTES.call(bytes),
  );
  let closed = false;
  return {
    [TRUSTED]: true,
    error: null,
    size: view.length,
    inMemory: true,
    readInto(offset, out) {
      if (closed) return 0;
      out.set(view.subarray(offset, offset + out.length));
      return out.length;
    },
    stats: () => ({
      requests: 0,
      bytesRequested: 0,
      bytesReceived: 0,
      maxBackingBytes: capacity,
      maxChargedBytes: capacity,
      liveBackingBytes: closed ? 0 : capacity,
    }),
    close() {
      closed = true;
    },
  };
}

export class BackingBudget {
  constructor() {
    this.buffers = new Map();
    this.used = { reader: 0, result: 0, response: 0 };
    this.peak = 0;
    this.chargedPeak = 0;
  }

  allocate(kind, bytes) {
    if (
      !Number.isSafeInteger(bytes) ||
      bytes < 0 ||
      this.used[kind] + bytes > PARTITION
    ) {
      throw hostLimit("the bounded source exceeded a host buffer partition");
    }
    this.used[kind] += bytes;
    this.chargedPeak = Math.max(
      this.chargedPeak,
      Object.values(this.used).reduce((a, b) => a + b, 0),
    );
    let view;
    try {
      view = new Uint8Array(bytes);
    } catch (cause) {
      this.used[kind] -= bytes;
      throw new CoreAVError(
        {
          schema: "coreav.error/1",
          kind: "limit",
          limit: "thumbnailHostBytes",
          offset: null,
          message: "the bounded source could not allocate a host buffer",
        },
        { cause },
      );
    }
    const token = { kind, bytes, view, released: false };
    this.buffers.set(token, view.buffer.byteLength);
    this.note();
    return token;
  }

  transferred(token, view) {
    if (!(view instanceof Uint8Array) || view.buffer.byteLength > token.bytes) {
      throw hostLimit("a BYOB source returned an oversized backing buffer");
    }
    token.view = view;
    this.buffers.set(token, view.buffer.byteLength);
    this.note();
  }

  note() {
    const unique = new Map();
    for (const [token, bytes] of this.buffers)
      unique.set(token.view.buffer, bytes);
    this.peak = Math.max(
      this.peak,
      [...unique.values()].reduce((a, b) => a + b, 0),
    );
  }

  release(token) {
    if (token === null || token.released) return;
    token.released = true;
    this.used[token.kind] -= token.bytes;
    this.buffers.delete(token);
    token.view = null;
  }

  stats() {
    const unique = new Map();
    for (const [token, bytes] of this.buffers) unique.set(token.view.buffer, bytes);
    return {
      maxBackingBytes: this.peak,
      maxChargedBytes: this.chargedPeak,
      liveBackingBytes: [...unique.values()].reduce((a, b) => a + b, 0),
    };
  }
}

export class BoundedRangeSession {
  constructor(fetchRange, settings, report, signal = undefined) {
    this.fetchRange = fetchRange;
    this.settings = settings;
    this.report = report;
    this.budget = new BackingBudget();
    this.aborts = new AbortController();
    this.signal = signal;
    this.onAbort = () => {
      this.close().catch(() => {});
    };
    this.response = null;
    this.pending = null;
    this.reading = null;
    this.result = null;
    this.closing = null;
    this.closed = false;
    this.cleanupError = null;
    this.size = undefined;
    this.validator = undefined;
    this.error = null;
    this.requests = 0;
    this.bytesReceived = 0;
    this[TRUSTED] = true;
  }

  async start() {
    try {
      this.signal?.addEventListener("abort", this.onAbort, { once: true });
      if (this.signal?.aborted) {
        await this.close();
        throw new DOMException("The operation was aborted", "AbortError");
      }
      await this.request(0, PARTITION);
      return this;
    } catch (error) {
      await this.close();
      throw error;
    }
  }

  request(offset, length) {
    const pending = this.replaceResponse(offset, length);
    this.pending = pending;
    return pending.finally(() => {
      if (this.pending === pending) this.pending = null;
    });
  }

  async replaceResponse(offset, length) {
    const previous = this.response;
    this.response = null;
    await this.cancel(previous?.reader);
    if (this.closed) throw new Error("the source session is closed");
    if (this.requests >= this.settings.maxRequests)
      throw new RequestLimitError(this.settings.maxRequests);
    this.requests += 1;
    const response = await this.fetchRange({
      offset,
      length,
      validator: this.validator,
      signal: this.aborts.signal,
    });
    const body = response.body;
    let nativeBody = false;
    try {
      STREAM_LOCKED.call(body);
      nativeBody = true;
    } catch {}
    let reader;
    try {
      if (!nativeBody)
        throw new TypeError("the bounded source requires a native byte stream");
      if (this.closed) throw new Error("the source session is closed");
      const { size, end = Math.min(size, offset + length) } = response;
      if (
        !Number.isSafeInteger(size) ||
        size < 0 ||
        response.offset !== offset ||
        !Number.isSafeInteger(end) ||
        end < offset ||
        end > size ||
        end > offset + length ||
        response.whole
      ) {
        throw new TypeError(
          "the bounded source requires an exact byte-range response",
        );
      }
      if (
        this.size !== undefined &&
        (size !== this.size || response.validator !== this.validator)
      ) {
        throw new SourceChangedError();
      }
      if (size > 0 && end === offset)
        throw new TypeError("the range response contains no requested bytes");
      reader = GET_READER.call(body, { mode: "byob" });
      this.size = size;
      this.validator = response.validator;
      this.response = { reader, pos: offset, end };
    } catch (error) {
      if (reader) await this.cancel(reader);
      else if (nativeBody) await this.cancel(body, true);
      throw error;
    }
  }

  read(offset, length) {
    if (this.reading !== null)
      return Promise.reject(
        new TypeError("bounded source reads must not overlap"),
      );
    const reading = this.readBytes(offset, length).catch((error) => {
      this.error = error;
      throw error;
    });
    this.reading = reading;
    return reading.finally(() => {
      if (this.reading === reading) this.reading = null;
    });
  }

  async readBytes(offset, length) {
    if (this.closed) throw new Error("the source session is closed");
    if (
      !Number.isSafeInteger(offset) ||
      offset < 0 ||
      !Number.isSafeInteger(length) ||
      length < 0 ||
      length > this.size - offset
    ) {
      throw new TypeError("the bounded source read is outside the object");
    }
    this.budget.release(this.result);
    this.result = null;
    const result = this.budget.allocate("result", length);
    try {
      let written = 0;
      while (written < length) {
        if (this.closed) throw new Error("the source session is closed");
        const at = offset + written;
        if (
          this.response === null ||
          this.response.pos !== at ||
          this.response.end === at
        ) {
          await this.request(at, Math.min(PARTITION, this.size - at));
        }
        const response = this.response;
        const wanted = Math.min(
          CHUNK,
          length - written,
          response.end - response.pos,
        );
        const chunk = this.budget.allocate("response", wanted);
        try {
          const { value } = await BYOB_READ.call(response.reader, chunk.view);
          if (this.closed) throw new Error("the source session is closed");
          if (
            !(value instanceof Uint8Array) ||
            value.length === 0 ||
            value.length > wanted
          ) {
            throw new TypeError(
              "the range response ended before the requested bytes",
            );
          }
          this.budget.transferred(chunk, value);
          result.view.set(value, written);
          response.pos += value.length;
          written += value.length;
          this.bytesReceived += value.length;
        } finally {
          this.budget.release(chunk);
        }
      }
      this.result = result;
      return result.view;
    } catch (error) {
      this.budget.release(result);
      throw error;
    }
  }

  readNow() {
    return 0;
  }

  async cancel(reader, body = false) {
    try {
      if (reader) await (body ? STREAM_CANCEL : READER_CANCEL).call(reader);
    } catch (cause) {
      this.cleanupError ??= cause;
      throw cause;
    }
  }

  stats() {
    return {
      ...this.budget.stats(),
      requests: this.requests,
      bytesReceived: this.bytesReceived,
    };
  }

  close() {
    if (this.closing !== null) return this.closing;
    this.closed = true;
    this.signal?.removeEventListener("abort", this.onAbort);
    const response = this.response;
    this.response = null;
    const cancelled = this.cancel(response?.reader);
    this.aborts.abort();
    this.closing = (async () => {
      await Promise.allSettled([cancelled, this.pending, this.reading]);
      this.budget.release(this.result);
      this.result = null;
      this.report(this.stats());
      if (this.cleanupError !== null) {
        throw new SourceCleanupError(this.cleanupError);
      }
    })();
    return this.closing;
  }
}
