import { initSync, runDocument, runDocumentInMemory, runIndexedStream, runStream, version } from "../wasm/coreav.js";

import { BUILD } from "../wasm/build.js";

import { CoreAVError, SourceNotFoundError } from "./errors.js";

import { TRUSTED } from "./internal.js";

let exports = null;

let trapped = null;

export function init(module) {
  if (exports !== null) return;
  if (typeof WebAssembly.Suspending !== "function" || typeof WebAssembly.promising !== "function") {
    throw new Error("CoreAV needs WebAssembly JSPI (Node.js 24: --experimental-wasm-jspi --experimental-wasm-exnref)");
  }
  exports = initSync({
    module: module
  });
}

export function moduleInfo() {
  ready();
  return {
    version: version(),
    build: BUILD,
    memoryBytes: exports.memory.buffer.byteLength
  };
}

function ready() {
  if (exports === null) {
    throw new Error("the CoreAV module is not loaded; import @coreav/wasm/workers or @coreav/wasm/node, or call init(module)");
  }
  if (trapped !== null) {
    throw new Error("the CoreAV module trapped earlier in this isolate and cannot run again", {
      cause: trapped
    });
  }
}

async function openSession(source, open) {
  if (source === null || typeof source !== "object" || typeof source.open !== "function") {
    throw new TypeError("source must be a CoreAV source, such as bytesSource(), r2Source() or httpSource()");
  }
  let session;
  try {
    session = await source.open(open);
  } catch (error) {
    if (error instanceof SourceNotFoundError) throw error;
    throw new CoreAVError(ioDocument(`the source could not be opened (${error.message ?? error})`), {
      cause: error
    });
  }
  const size = session.size;
  if (!Number.isSafeInteger(size) || size < 0) {
    session.close?.();
    throw new TypeError("the source session has no valid size");
  }
  if (session[TRUSTED] === true) return session;
  const wrapper = {
    error: null,
    size: size,
    inMemory: session.inMemory === true,
    close: () => session.close?.()
  };
  if (wrapper.inMemory) {
    wrapper.readInto = (offset, view) => {
      if (trapped !== null) return 0;
      try {
        const count = session.readInto(offset, view);
        if (count === view.length) return count;
        wrapper.error = new Error(`the source copied ${count} of ${view.length} bytes`);
      } catch (error) {
        wrapper.error = error;
      }
      return 0;
    };
  } else {
    const fail = error => {
      wrapper.error = error;
      throw error;
    };
    const check = (bytes, length) => {
      if (trapped !== null) fail(new Error("the CoreAV module trapped"));
      if (!(bytes instanceof Uint8Array) || bytes.length !== length) {
        const got = bytes instanceof Uint8Array ? `${bytes.length} bytes` : bytes === null ? "null" : typeof bytes;
        fail(new Error(`the source returned ${got} for a read of ${length} bytes`));
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
    wrapper.planRegions = regions => {
      try {
        session.planRegions?.(regions);
      } catch {}
    };
    wrapper.read = (offset, length) => {
      try {
        return Promise.resolve(session.read(offset, length)).then(bytes => check(bytes, length), fail);
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
    message: message
  };
}

function failure(rejection, session, sinkError) {
  if (typeof rejection !== "string") {
    if (rejection instanceof WebAssembly.RuntimeError) trapped = rejection;
    return rejection;
  }
  const value = JSON.parse(rejection);
  if (typeof value.usage === "string") return new TypeError(value.usage);
  const cause = sinkError ?? session.error;
  if (value.kind === "io" && cause) {
    const error = new CoreAVError(rejection, {
      cause: cause
    });
    error.message = `${value.message} (${cause.message ?? cause})`;
    return error;
  }
  return new CoreAVError(rejection);
}

export async function runToText(operation, source, options, open = undefined) {
  ready();
  const json = JSON.stringify(options);
  const session = await openSession(source, open);
  try {
    const text = session.inMemory ? runDocumentInMemory(operation, session, session.size, json) : await runDocument(operation, session, session.size, false, json);
    return text;
  } catch (rejection) {
    throw failure(rejection, session, null);
  } finally {
    session.close();
  }
}

const QUEUED_BYTES = 1024 * 1024;

export async function runToStream(operation, source, options, index = null, open = undefined) {
  ready();
  const json = JSON.stringify(options);
  const session = await openSession(source, open);
  let controller;
  let waiter = null;
  let cancelled = null;
  const body = new ReadableStream({
    start(c) {
      controller = c;
    },
    pull() {
      if (waiter !== null) {
        const {resolve: resolve} = waiter;
        waiter = null;
        resolve();
      }
    },
    cancel(reason) {
      cancelled = reason ?? new Error("the output was cancelled");
      session.close();
      if (waiter !== null) {
        const {reject: reject} = waiter;
        waiter = null;
        reject(cancelled);
      }
    }
  }, {
    highWaterMark: QUEUED_BYTES,
    size: chunk => chunk.byteLength
  });
  let wrote;
  const firstWrite = new Promise(resolve => {
    wrote = resolve;
  });
  const sink = {
    write(view) {
      if (cancelled !== null) throw cancelled;
      if (trapped !== null) throw new Error("the CoreAV module trapped");
      controller.enqueue(view.slice());
      wrote();
      if (controller.desiredSize > 0) return undefined;
      return new Promise((resolve, reject) => {
        waiter = {
          resolve: resolve,
          reject: reject
        };
      });
    }
  };
  const running = index === null ? runStream(operation, session, session.size, session.inMemory, sink, json) : runIndexedStream(operation, index, session, session.size, session.inMemory, sink, json);
  const summary = running.then(text => {
    if (cancelled === null) controller.close();
    return JSON.parse(text);
  }, rejection => {
    const error = failure(rejection, session, cancelled);
    if (cancelled === null) controller.error(error);
    throw error;
  }).finally(() => session.close());
  summary.catch(() => {});
  await Promise.race([ firstWrite, summary ]);
  return {
    body: body,
    summary: summary
  };
}
