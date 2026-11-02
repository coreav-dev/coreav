import { MessageChannel, Worker } from "node:worker_threads";
import { boundedOpeners } from "./bounded-source.js";
import {
  CoreAVError,
  RequestLimitError,
  SourceChangedError,
  SourceNotFoundError,
  errorResponse,
} from "./errors.js";
import { createAdmission, runBounded } from "./node-broker.js";
import { restoreError, serializeError } from "./node-session.js";
import { RANGE_DEFAULTS, bytesSource, httpSource, rangeSource } from "./sources.js";

export {
  CoreAVError,
  RequestLimitError,
  SourceChangedError,
  SourceNotFoundError,
  errorResponse,
};
export { RANGE_DEFAULTS, bytesSource, httpSource, rangeSource };

const admission = createAdmission();
const streams = new Set(["clip", "extractAudio", "extractSubtitles", "remux", "sanitize", "hlsGet", "thumbnail"]);
let worker = null;
let ready = null;
let info = null;

function spawn() {
  const child = new Worker(new URL("./node-worker.js", import.meta.url), { type: "module", execArgv: [] });
  worker = child;
  ready = new Promise((resolve, reject) => {
    const onMessage = (message) => {
      if (message?.type === "info") info = message.info;
      if (message?.type !== "ready") return;
      info = message.info;
      child.unref();
      resolve(child);
    };
    child.on("message", onMessage);
    child.once("error", reject);
  });
  child.once("exit", () => {
    if (worker === child) {
      worker = null;
      ready = null;
      info = null;
    }
  });
  return ready;
}

await spawn();

export function init() {}

export function moduleInfo() {
  if (info === null) {
    throw new Error("the CoreAV module was discarded");
  }
  return info;
}

function requireBounded(source) {
  if (typeof boundedOpeners.get(source) !== "function") {
    throw new TypeError("the bounded module requires its bytesSource, httpSource or r2Source adapter");
  }
}

export function probe(source, options) {
  return call("probe", source, options);
}
export function probeText(source, options) {
  return call("probeText", source, options);
}
export function ffprobe(source, options) {
  return call("ffprobe", source, options);
}
export function ffprobeText(source, options) {
  return call("ffprobeText", source, options);
}
export function check(source, options) {
  return call("check", source, options);
}
export function chapters(source, options) {
  return call("chapters", source, options);
}
export function clip(source, options) {
  return call("clip", source, options);
}
export function extractAudio(source, options) {
  return call("extractAudio", source, options);
}
export function extractSubtitles(source, options) {
  return call("extractSubtitles", source, options);
}
export function remux(source, options) {
  return call("remux", source, options);
}
export function sanitize(source, options) {
  return call("sanitize", source, options);
}
export function hlsIndex(source, options) {
  return call("hlsIndex", source, options);
}
export function hlsGet(index, name, source, options) {
  const segment = typeof name === "string" && name.endsWith(".m4s");
  return call("hlsGet", segment ? source : null, options, { index: index, name: name, segment: segment });
}
export function thumbnail(source, options) {
  return call("thumbnail", source, options);
}

function call(name, source, options = {}, extra = null) {
  if (source !== null && source !== undefined) requireBounded(source);
  const opener = source === null || source === undefined ? null : boundedOpeners.get(source);
  return runBounded(admission, options, {
    schedule: (ms, fn) => setTimeout(fn, ms),
    clear: (timer) => clearTimeout(timer),
    terminate() {
      const current = worker;
      worker = null;
      ready = null;
      info = null;
      return current ? current.terminate() : Promise.resolve(0);
    },
    open(arg) {
      if (opener === null) throw new TypeError("this operation has no bounded source");
      return opener.call(source, arg);
    },
  }, (io) => execute(name, io, extra));
}

function execute(name, io, extra) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let output;
    const fail = (error) => {
      if (output?.started) { output.fail(error); return; }
      if (settled) return;
      settled = true;
      reject(error);
    };
    io.onStop(fail);
    const channel = new MessageChannel();
    const port = channel.port1;
    output = streams.has(name) ? parentOutput(port) : null;
    if (output) io.bind(output);
    const onMessage = (message) => onWorker(message, { io, port, output, name, resolve, fail, settled: () => settled, mark: () => { settled = true; } });
    const onClose = () => {
      if (!settled || (output?.started && !output.ended))
        fail(new CoreAVError({ schema: "coreav.error/1", kind: "io", limit: null, offset: null, message: "the worker stopped before completing the operation" }));
    };
    port.on("message", onMessage);
    port.on("close", onClose);
    io.deferCleanup(() => {
      port.off("message", onMessage);
      port.off("close", onClose);
      port.close();
    });
    deliver(name, io, extra, channel.port2).catch(fail);
  });
}

async function deliver(name, io, extra, port) {
  const child = worker ?? await spawn();
  await ready;
  if (worker !== child) throw new Error("the CoreAV worker stopped");
  child.ref();
  io.deferCleanup(() => child.unref());
  const transfer = [port];
  let payload = extra;
  if (extra?.index instanceof Uint8Array) {
    const index = new Uint8Array(extra.index);
    payload = { ...extra, index: index };
    transfer.push(index.buffer);
  }
  child.postMessage({ type: "run", call: name, options: io.options, extra: payload, port: port }, transfer);
}

function onWorker(message, state) {
  if (state.io.stopped) return;
  if (message?.info) info = message.info;
  if (message?.type === "need-open") {
    state.io.open(message.open ?? undefined).then((opened) => {
      try {
        const transfer = [];
        const reply = { type: "opened", id: message.id, size: opened.size, inMemory: opened.inMemory === true };
        if (opened.inMemory) {
          const bytes = new Uint8Array(opened.size);
          if (opened.readInto(0, bytes) !== opened.size) {
            throw new Error("the source copied the wrong number of bytes");
          }
          reply.bytes = bytes;
          transfer.push(bytes.buffer);
        }
        state.port.postMessage(reply, transfer);
      } catch (error) {
        state.port.postMessage({ type: "failed", id: message.id, error: serializeError(error) });
      }
    }, (error) => {
      state.port.postMessage({ type: "failed", id: message.id, error: serializeError(error) });
    });
    return;
  }
  if (message?.type === "read") {
    state.io.open().then(async (opened) => {
      const bytes = await opened.read(message.offset, message.length);
      const copy = new Uint8Array(bytes);
      state.port.postMessage({ type: "read-result", id: message.id, bytes: copy }, [copy.buffer]);
    }).catch((error) => {
      state.port.postMessage({ type: "failed", id: message.id, error: serializeError(error) });
    });
    return;
  }
  if (message?.type === "plan" || message?.type === "plan-regions") {
    state.io.open().then((opened) => {
      if (message.type === "plan") opened.plan?.(message.offset, message.length);
      else opened.planRegions?.(message.regions);
    }, () => {});
    return;
  }
  if (message?.type === "chunk") {
    if (!state.output.push(message.bytes)) return;
    if (!state.output.started) {
      state.output.started = true;
      state.mark();
      state.resolve({
        value: { body: state.output.body, summary: state.output.summary, contentType: message.contentType },
        pending: state.output.completed,
      });
    }
    return;
  }
  if (message?.type === "done") {
    if (state.output) {
      if (!state.output.finish(message.summary)) return;
      if (!state.output.started) {
        state.output.started = true;
        state.mark();
        state.resolve({
          value: { body: state.output.body, summary: state.output.summary, contentType: message.contentType },
          pending: state.output.completed,
        });
      }
      return;
    }
    state.mark();
    state.resolve({ value: message.value });
    return;
  }
  if (message?.type === "fail") {
    const error = restoreError(message.error);
    if (state.output?.started) state.output.fail(error);
    else state.fail(error);
  }
}

function parentOutput(port) {
  let controller = null;
  let cancelled = false;
  const queued = [];
  let summaryResolve;
  let summaryReject;
  let completedResolve;
  let completedReject;
  const summary = new Promise((resolve, reject) => {
    summaryResolve = resolve;
    summaryReject = reject;
  });
  const completed = new Promise((resolve, reject) => {
    completedResolve = resolve;
    completedReject = reject;
  });
  summary.catch(() => {});
  completed.catch(() => {});
  const body = new ReadableStream({
    start(stream) {
      controller = stream;
      for (const chunk of queued) stream.enqueue(chunk);
      queued.length = 0;
    },
    pull() {
      port.postMessage({ type: "pull" });
    },
    cancel(reason) {
      cancelled = true;
      queued.length = 0;
      port.postMessage({ type: "cancel" });
      return completed.then(() => {}, () => {});
    },
  }, { highWaterMark: 0, size: (chunk) => chunk.byteLength });
  return {
    body: body,
    summary: summary,
    completed: completed,
    started: false,
    ended: false,
    push(bytes) {
      if (this.ended || cancelled) return false;
      if (controller) controller.enqueue(bytes);
      else queued.push(bytes);
      return true;
    },
    finish(value) {
      if (this.ended) return false;
      if (cancelled) {
        this.fail(new DOMException("The output was cancelled", "AbortError"));
        return false;
      }
      this.ended = true;
      controller?.close();
      summaryResolve(value);
      completedResolve();
      return true;
    },
    fail(error) {
      if (this.ended) return;
      this.ended = true;
      controller?.error(error);
      summaryReject(error);
      completedReject(error);
    },
  };
}
