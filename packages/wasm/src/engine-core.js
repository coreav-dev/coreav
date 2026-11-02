import { CoreAVError, SourceNotFoundError } from "./errors.js";

import { EXNREF_PROBE, TRUSTED } from "./internal.js";
import {
  SourceCleanupError,
  boundedOpeners,
  hostLimit,
} from "./bounded-source.js";

export function createEngine(bindings, build, { exclusive = false } = {}) {
  const {
    initSync,
    runDocument,
    runDocumentInMemory,
    runIndexedStream,
    runStream,
    version,
  } = bindings;

  let exports = null;

  let trapped = null;

  let cleanupError = null;

  let active = false;
  const lease = { acquired: 0, released: 0, rejected: 0, sourceOpens: 0 };
  let observedSession = null;
  let outputLive = 0;
  let host = null;

  function noteHost() {
    if (!exclusive || host === null) return;
    const stats = observedSession?.stats?.();
    host.maxBackingBytes = Math.max(host.maxBackingBytes, (stats?.maxBackingBytes ?? 0) + host.maxOutputBackingBytes);
    host.maxChargedBytes = Math.max(host.maxChargedBytes, (stats?.maxChargedBytes ?? 0) + host.maxOutputChargedBytes);
    host.liveBytes = (stats?.liveBackingBytes ?? 0) + outputLive;
  }

  function diagnostics() {
    noteHost();
    return {
      lease: { active, ...lease },
      host: host === null ? null : { ...host },
      settled: !active && outputLive === 0 && observedSession === null,
    };
  }

  function acquire() {
    if (exclusive && active) {
      lease.rejected += 1;
      throw new TypeError("the CoreAV module already has an active operation");
    }
    if (exclusive) {
      active = true;
      lease.acquired += 1;
      host = { requestId: lease.acquired, maxBackingBytes: 0, maxChargedBytes: 0, maxOutputBackingBytes: 0, maxOutputChargedBytes: 0, liveBytes: 0 };
    }
    let released = false;
    return () => {
      if (released) return;
      released = true;
      if (exclusive) {
        noteHost();
        observedSession = null;
        active = false;
        lease.released += 1;
      }
    };
  }

  /** Initialize once with the package module or its bytes. Needs JSPI and exception references; later calls do not replace the instance. */
  function init(module) {
    if (exports !== null) return;
    if (
      typeof WebAssembly.Suspending !== "function" ||
      typeof WebAssembly.promising !== "function"
    ) {
      throw new Error(
        "CoreAV needs WebAssembly JSPI (Node.js 24: --experimental-wasm-jspi --experimental-wasm-exnref)",
      );
    }
    if (
      !WebAssembly.validate(EXNREF_PROBE) ||
      typeof WebAssembly.Tag !== "function" ||
      !(WebAssembly.JSTag instanceof WebAssembly.Tag)
    ) {
      throw new Error(
        "CoreAV needs WebAssembly exception references (Node.js 24: --experimental-wasm-jspi --experimental-wasm-exnref)",
      );
    }
    exports = initSync({
      module: module,
    });
  }

  /** Version, packaged-artifact SHA-256 and current linear memory size. Custom init callers must supply the matching module. */
  function moduleInfo() {
    ready();
    return {
      version: version(),
      build: build,
      memoryBytes: exports.memory.buffer.byteLength,
    };
  }

  function ready() {
    if (exports === null) {
      throw new Error(
        "the CoreAV module is not loaded; import @coreav/wasm/workers or @coreav/wasm/node, or call init(module)",
      );
    }
    if (cleanupError !== null) {
      throw new Error(
        "CoreAV source cleanup failed and the instance cannot run again",
        { cause: cleanupError },
      );
    }
    if (trapped !== null) {
      throw new Error(
        "the CoreAV module trapped earlier in this isolate and cannot run again",
        {
          cause: trapped,
        },
      );
    }
  }

  async function closeSession(session) {
    try {
      await session?.close();
      noteHost();
    } catch (cause) {
      if (!exclusive) throw cause;
      cleanupError = cause;
      if (cause instanceof CoreAVError) throw cause;
      throw new CoreAVError(ioDocument("the source could not be closed"), {
        cause,
      });
    }
  }

  /**
   * Opens `source` and wraps its session so that its last error is kept.
   * `open` is the session's options (`memoryBytes`, for an operation that
   * needs much of the isolate itself); a source may ignore them.
   */
  async function openSession(source, open) {
    if (
      source === null ||
      typeof source !== "object" ||
      typeof source.open !== "function"
    ) {
      throw new TypeError(
        "source must be a CoreAV source, such as bytesSource(), r2Source() or httpSource()",
      );
    }
    const opener = exclusive ? boundedOpeners.get(source) : source.open;
    if (typeof opener !== "function") {
      throw new TypeError(
        "the bounded module requires its bytesSource, httpSource or r2Source adapter",
      );
    }
    let session;
    try {
      if (exclusive) lease.sourceOpens += 1;
      session = await opener.call(source, open);
      if (exclusive) observedSession = session;
    } catch (error) {
      if (error instanceof SourceNotFoundError) throw error;
      if (exclusive && error instanceof SourceCleanupError)
        cleanupError = error;
      if (exclusive && error instanceof CoreAVError) throw error;
      throw new CoreAVError(ioDocument("the source could not be opened"), {
        cause: error,
      });
    }
    const size = session.size;
    if (!Number.isSafeInteger(size) || size < 0) {
      await closeSession(session);
      throw new TypeError("the source session has no valid size");
    }
    if (session[TRUSTED] === true) return session;
    const wrapper = {
      error: null,
      size: size,
      inMemory: session.inMemory === true,
      close: () => session.close?.(),
    };
    if (wrapper.inMemory) {
      wrapper.readInto = (offset, view) => {
        if (trapped !== null) return 0;
        try {
          const count = session.readInto(offset, view);
          if (count === view.length) return count;
          wrapper.error = new Error(
            `the source copied ${count} of ${view.length} bytes`,
          );
        } catch (error) {
          wrapper.error = error;
        }
        return 0;
      };
    } else {
      const fail = (error) => {
        wrapper.error = error;
        throw error;
      };
      const check = (bytes, length) => {
        if (trapped !== null) fail(new Error("the CoreAV module trapped"));
        if (!(bytes instanceof Uint8Array) || bytes.length !== length) {
          const got =
            bytes instanceof Uint8Array
              ? `${bytes.length} bytes`
              : bytes === null
                ? "null"
                : typeof bytes;
          fail(
            new Error(
              `the source returned ${got} for a read of ${length} bytes`,
            ),
          );
        }
        return bytes;
      };
      wrapper.readInto = (offset, view) => {
        if (trapped !== null) return 0;
        try {
          return session.readNow?.(offset, view) ?? 0;
        } catch {
          return 0;
        }
      };
      wrapper.plan = (offset, length) => {
        try {
          session.plan?.(offset, length);
        } catch {}
      };
      wrapper.planRegions = (regions) => {
        try {
          session.planRegions?.(regions);
        } catch {}
      };
      wrapper.read = (offset, length) => {
        try {
          return Promise.resolve(session.read(offset, length)).then(
            (bytes) => check(bytes, length),
            fail,
          );
        } catch (error) {
          wrapper.error = error;
          return Promise.reject(error);
        }
      };
    }
    return wrapper;
  }

  function ioDocument(message) {
    return {
      schema: "coreav.error/1",
      kind: "io",
      limit: null,
      offset: null,
      message: message,
    };
  }

  /** The error for a rejection of the module, given what failed around it. */
  function failure(rejection, session, sinkError) {
    if (sinkError?.name === "AbortError") return sinkError;
    if (typeof rejection !== "string") {
      if (rejection instanceof WebAssembly.RuntimeError) trapped = rejection;
      return rejection;
    }
    const value = JSON.parse(rejection);
    if (typeof value.usage === "string") return new TypeError(value.usage);
    const cause = sinkError ?? session?.error;
    if (exclusive && cause instanceof CoreAVError && cause.kind === "limit")
      return cause;
    if (value.kind === "io" && cause) {
      return new CoreAVError(rejection, { cause: cause });
    }
    return new CoreAVError(rejection);
  }

  function abortError(signal) {
    const error = new DOMException("The operation was aborted", "AbortError");
    if (signal?.reason !== undefined)
      Object.defineProperty(error, "cause", { value: signal.reason });
    return error;
  }

  function requestOptions(options, open) {
    const { signal, ...plain } = options ?? {};
    if (signal !== undefined && !(signal instanceof AbortSignal))
      throw new TypeError("signal must be an AbortSignal");
    if (signal?.aborted) throw abortError(signal);
    return {
      signal,
      json: JSON.stringify(plain),
      open: signal === undefined ? open : { ...open, signal },
    };
  }

  /** Runs an operation that returns a document; resolves to its JSON text. */
  async function runToText(operation, source, options, open = undefined) {
    ready();
    const release = acquire();
    let session;
    let closing;
    let signal;
    const close = () => session === undefined ? Promise.resolve() : (closing ??= closeSession(session));
    const onAbort = () => { close().catch(() => {}); };
    try {
      const request = requestOptions(options, open);
      signal = request.signal;
      signal?.addEventListener("abort", onAbort, { once: true });
      const json = request.json;
      session = await openSession(source, request.open);
      if (signal?.aborted) throw abortError(signal);
      const text = session.inMemory
        ? runDocumentInMemory(operation, session, session.size, json)
        : await runDocument(operation, session, session.size, false, json);
      if (signal?.aborted) throw abortError(signal);
      return text;
    } catch (rejection) {
      if (signal?.aborted) throw abortError(signal);
      throw failure(rejection, session, null);
    } finally {
      try {
        await close();
      } finally {
        signal?.removeEventListener("abort", onAbort);
        release();
      }
    }
  }

  /** Output bytes queued (four chunks of the module's) before it waits for the reader. */
  const QUEUED_BYTES = 1024 * 1024;

  /**
   * Runs an operation that writes an output. Resolves once the first bytes
   * are written, to `{ body, summary }`: `body` streams the output with
   * backpressure, `summary` resolves to the operation's summary after the last
   * byte. Rejects, with no output, when the operation fails before writing.
   * A failure after the first bytes errors `body` and rejects `summary`;
   * cancelling `body` stops the operation. `index`, for operations that read
   * an index besides the source (`hlsGet`), is copied into the module.
   * `open` is passed to the source's `open()`.
   */
  async function runToStream(
    operation,
    source,
    options,
    index = null,
    open = undefined,
  ) {
    ready();
    const release = acquire();
    let session;
    let closing;
    let signal;
    let controller;
    let waiter = null;
    let cancelled = null;
    const close = () => session === undefined ? Promise.resolve() : (closing ??= closeSession(session));
    const onAbort = () => {
      cancelled ??= abortError(signal);
      controller?.error(cancelled);
      if (waiter !== null) {
        const { reject } = waiter;
        waiter = null;
        reject(cancelled);
      }
      close().catch(() => {});
    };
    try {
      const request = requestOptions(options, open);
      signal = request.signal;
      signal?.addEventListener("abort", onAbort, { once: true });
      const json = request.json;
      session = await openSession(source, request.open);
      if (signal?.aborted) throw abortError(signal);
      let summary;
      const body = new ReadableStream(
        {
          start(c) {
            controller = c;
          },
          pull() {
            if (waiter !== null) {
              const { resolve: resolve } = waiter;
              waiter = null;
              resolve();
            }
          },
          cancel(reason) {
            cancelled = reason ?? new Error("the output was cancelled");
            const closed = close();
            if (waiter !== null) {
              const { reject: reject } = waiter;
              waiter = null;
              reject(cancelled);
            }
            return exclusive
              ? Promise.allSettled([closed, summary]).then(() => {})
              : closed;
          },
        },
        {
          highWaterMark: exclusive ? 0 : QUEUED_BYTES,
          size: (chunk) => chunk.byteLength,
        },
      );
      let wrote;
      const firstWrite = new Promise((resolve) => {
        wrote = resolve;
      });
      const sink = {
        write(view) {
          if (cancelled !== null) throw cancelled;
          if (trapped !== null) throw new Error("the CoreAV module trapped");
          if (
            exclusive &&
            (!(view instanceof Uint8Array) || view.byteLength > 256 * 1024)
          ) {
            throw hostLimit(
              "the CoreAV module wrote more than 256 KiB at once",
            );
          }
          const charge = exclusive ? view.byteLength : 0;
          if (exclusive && outputLive + charge > QUEUED_BYTES + 256 * 1024)
            throw hostLimit("the output exceeded its host buffer partition");
          outputLive += charge;
          if (exclusive) {
            host.maxOutputChargedBytes = Math.max(host.maxOutputChargedBytes, outputLive);
            host.maxOutputBackingBytes = Math.max(host.maxOutputBackingBytes, outputLive);
            noteHost();
          }
          try {
            controller.enqueue(view.slice());
          } catch (error) {
            outputLive -= charge;
            throw error;
          }
          wrote();
          if (controller.desiredSize > 0) {
            outputLive -= charge;
            return undefined;
          }
          return new Promise((resolve, reject) => {
            waiter = {
              resolve: resolve,
              reject: reject,
            };
          }).finally(() => { outputLive -= charge; noteHost(); });
        },
      };
      const running = Promise.resolve().then(() =>
        index === null
          ? runStream(
              operation,
              session,
              session.size,
              session.inMemory,
              sink,
              json,
            )
          : runIndexedStream(
              operation,
              index,
              session,
              session.size,
              session.inMemory,
              sink,
              json,
            ),
      );
      summary = running
        .then(
          (text) => {
            if (cancelled !== null) throw cancelled;
            if (cancelled === null) controller.close();
            return JSON.parse(text);
          },
          (rejection) => {
            const error = failure(rejection, session, cancelled);
            if (cancelled === null) controller.error(error);
            throw error;
          },
        )
        .finally(async () => {
          try {
            await close();
          } finally {
            signal?.removeEventListener("abort", onAbort);
            release();
          }
        });
      summary.catch(() => {});
      await Promise.race([firstWrite, summary]);
      return {
        body: body,
        summary: summary,
      };
    } catch (error) {
      try {
        await close();
      } finally {
        signal?.removeEventListener("abort", onAbort);
        release();
      }
      if (signal?.aborted) throw abortError(signal);
      throw error;
    }
  }

  return { init, moduleInfo, runToText, runToStream, diagnostics };
}
