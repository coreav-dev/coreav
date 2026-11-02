import { readFileSync } from "node:fs";
import { parentPort } from "node:worker_threads";
import * as api from "./api.js";
import { init, moduleInfo } from "./engine.js";
import { brokerSource, serializeError } from "./node-session.js";
import { thumbnail } from "./thumbnail.js";

init(readFileSync(new URL("../wasm/coreav_bg.wasm", import.meta.url)));
parentPort.postMessage({ type: "ready", info: moduleInfo() });

const calls = {
  probe: (source, options) => api.probe(source, options),
  probeText: (source, options) => api.probeText(source, options),
  ffprobe: (source, options) => api.ffprobe(source, options),
  ffprobeText: (source, options) => api.ffprobeText(source, options),
  check: (source, options) => api.check(source, options),
  chapters: (source, options) => api.chapters(source, options),
  clip: (source, options) => api.clip(source, options),
  extractAudio: (source, options) => api.extractAudio(source, options),
  extractSubtitles: (source, options) => api.extractSubtitles(source, options),
  remux: (source, options) => api.remux(source, options),
  sanitize: (source, options) => api.sanitize(source, options),
  hlsIndex: (source, options) => api.hlsIndex(source, options),
  hlsGet: (source, options, extra) => api.hlsGet(extra.index, extra.name, extra.segment ? source : null, options),
  thumbnail: (source, options) => thumbnail(source, options),
};

function reportInfo() {
  parentPort.postMessage({ type: "info", info: moduleInfo() });
}

parentPort.on("message", (message) => {
  if (message?.type !== "run" || typeof calls[message.call] !== "function") return;
  run(message).catch((error) => {
    message.port.postMessage({ type: "fail", error: serializeError(error), info: currentInfo() });
  }).finally(() => { message.port.close(); reportInfo(); });
});

function currentInfo() {
  try { return moduleInfo(); } catch { return null; }
}

async function run(message) {
  const port = message.port;
  const source = brokerSource(port);
  const result = await calls[message.call](source, message.options ?? {}, message.extra);
  if (result && typeof result.body?.getReader === "function") {
    await forward(port, result);
    return;
  }
  if (result?.index instanceof Uint8Array) {
    const index = new Uint8Array(result.index);
    port.postMessage({ type: "done", value: { index: index, summary: result.summary }, info: currentInfo() }, [index.buffer]);
    return;
  }
  port.postMessage({ type: "done", value: result, info: currentInfo() });
}

async function forward(port, result) {
  const reader = result.body.getReader();
  let credits = 0;
  let waiting = null;
  let cancelled = null;
  const onMessage = (message) => {
    if (message?.type === "pull") {
      credits += 1;
      if (waiting !== null) {
        const resolve = waiting;
        waiting = null;
        resolve();
      }
    } else if (message?.type === "cancel") {
      cancelled = new DOMException("The output was cancelled", "AbortError");
      reader.cancel(cancelled).catch(() => {});
      if (waiting !== null) { const resolve = waiting; waiting = null; resolve(); }
    }
  };
  port.on("message", onMessage);
  let first = true;
  try {
    while (true) {
      if (!first) {
        while (credits < 1) {
          if (cancelled !== null) throw cancelled;
          await new Promise((resolve) => {
            waiting = resolve;
          });
        }
        credits -= 1;
      }
      if (cancelled !== null) throw cancelled;
      const next = await reader.read();
      if (next.done) break;
      const bytes = new Uint8Array(next.value);
      port.postMessage({ type: "chunk", first: first, contentType: result.contentType, bytes: bytes }, [bytes.buffer]);
      first = false;
    }
    port.postMessage({ type: "done", summary: await result.summary, contentType: result.contentType, info: currentInfo() });
  } catch (error) {
    await reader.cancel(error).catch(() => {});
    await Promise.resolve(result.summary).catch(() => {});
    port.postMessage({ type: "fail", error: serializeError(error), info: currentInfo() });
  } finally {
    port.off("message", onMessage);
  }
}
