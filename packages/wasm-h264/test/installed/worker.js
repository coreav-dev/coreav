import * as api from "@coreav/wasm-h264/workers";
import { diagnostics } from "H264_ENGINE";
import { runHostCases } from "HOST_CASES";
import { createTracker } from "./batch.mjs";

const { thumbnail, probe, r2Source, moduleInfo } = api;

let isolate;
const tracker = createTracker({ deadlineMs: 15000 });
const results = new Map();

function source(env, fixture, held) {
  return r2Source({ async get(key, options) {
    if (held && !held.entered) {
      held.entered = true; held.notify(); await held.wait;
    }
    const { offset, length } = options.range;
    const response = await env.FIXTURES.fetch(`http://fixtures.test/${encodeURIComponent(key)}`, {
      headers: { range: `bytes=${offset}-${offset + length - 1}`, ...(options.onlyIf ? { "if-match": options.onlyIf.etagMatches } : {}) },
    });
    if (response.status !== 206) throw new Error("fixture range unavailable");
    const match = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(response.headers.get("content-range") ?? "");
    if (!match || Number(match[1]) !== offset) throw new Error("fixture range invalid");
    return { body: response.body, size: Number(match[3]), etag: response.headers.get("etag"), range: { offset } };
  } }, fixture, { memoryBytes: 4 * 1048576, parallel: 1, bufferBytes: 4 * 1048576, segmentBytes: 4 * 1048576 });
}

async function drain(output) {
  const reader = output.body.getReader();
  while (!(await reader.read()).done) {}
  return output.summary;
}

function failureType(error) {
  if (error == null) return "none";
  try {
    if (error instanceof WebAssembly.RuntimeError) return "wasm_trap";
    if (error instanceof TypeError) return "type_error";
    if (error instanceof RangeError) return "range_error";
    if (typeof api.CoreAVError === "function" && error instanceof api.CoreAVError) return "coreav";
    if (typeof api.SourceNotFoundError === "function" && error instanceof api.SourceNotFoundError) return "source_not_found";
    if (typeof api.RequestLimitError === "function" && error instanceof api.RequestLimitError) return "request_limit";
    if (error instanceof DOMException) return error.name === "AbortError" ? "abort" : "dom_exception";
    if (error instanceof Error) return "error";
  } catch {}
  return "other";
}

function failureTypes(error) {
  let cause;
  try { cause = error?.cause; } catch {}
  return { failureType: failureType(error), causeType: failureType(cause) };
}

async function admission(env, args) {
  let notify, release;
  const entered = new Promise(resolve => { notify = resolve; });
  const wait = new Promise(resolve => { release = resolve; });
  const before = diagnostics();
  const held = { wait, notify, entered: false };
  const selected = source(env, args.fixture, args.stage === "output-backpressure" ? null : held);
  const running = args.stage === "probe-first" ? probe(selected) : thumbnail(selected, args.options);
  running.catch(() => {});
  let output;
  if (args.stage === "output-backpressure") output = await running;
  else await entered;
  const atHold = diagnostics();
  if (atHold.lease.active !== true || atHold.lease.acquired !== before.lease.acquired + 1) throw new Error("actual lease was not held");
  const attempts = args.stage === "probe-first" ? ["thumbnail"] : ["thumbnail", "probe"];
  for (const operation of attempts) {
    let refused = false;
    try {
      const fresh = source(env, args.fixture);
      const result = operation === "thumbnail" ? await thumbnail(fresh, args.options) : await probe(fresh);
      if (result?.body) await drain(result);
    } catch (error) {
      refused = error instanceof TypeError && /active operation/.test(error.message);
    }
    if (!refused) throw new Error("overlapping operation was not refused");
  }
  const rejected = diagnostics();
  release();
  if (args.stage === "probe-first") await running;
  else await drain(output ?? await running);
  const cleaned = diagnostics();
  await drain(await thumbnail(source(env, args.fixture), args.options));
  const reused = diagnostics();
  return {
    stage: args.stage, reused: !reused.lease.active, drained: true, settled: reused.settled,
    holdAcquired: atHold.lease.acquired - before.lease.acquired,
    rejections: rejected.lease.rejected - atHold.lease.rejected,
    extraAcquisitions: rejected.lease.acquired - atHold.lease.acquired,
    extraSourceOpens: rejected.lease.sourceOpens - atHold.lease.sourceOpens,
    activeAtHold: atHold.lease.active, activeAfterCleanup: cleaned.lease.active,
    holdReleased: cleaned.lease.released - before.lease.released,
    hostSnapshots: [cleaned.host, reused.host],
  };
}

export default {
  async fetch(request, env, ctx) {
    isolate ??= crypto.randomUUID();
    const url = new URL(request.url);
    if (url.pathname === "/memory") return Response.json({ ...moduleInfo(), ...await tracker.reading(), isolate, diagnostic: diagnostics() });
    if (url.pathname.startsWith("/result/")) {
      await tracker.reading();
      const id = url.pathname.slice(8), result = results.get(id);
      if (!result) return new Response(null, { status: 404 });
      results.delete(id);
      return Response.json({ ...result, diagnostic: diagnostics(), isolate });
    }
    const complete = await tracker.enter();
    const args = await request.json();
    if (typeof args.requestId !== "string" || !/^[a-zA-Z0-9-]{1,64}$/.test(args.requestId) || results.has(args.requestId)) {
      complete(); return new Response(null, { status: 400 });
    }
    const before = diagnostics();
    try {
      if (url.pathname === "/host-cases") {
        const report = await runHostCases({ api, diagnostics, source: (fixture, held) => source(env, fixture, held), drain, env, fixture: args.fixture, options: args.options });
        return tracker.track(Response.json(report), complete);
      }
      if (url.pathname === "/admission") {
        const witness = await admission(env, args);
        results.set(args.requestId, { outcome: "admission", complete: true, admission: witness, operationId: diagnostics().host.requestId });
        return tracker.track(Response.json({ admitted: true }), complete);
      }
      const output = await thumbnail(source(env, args.fixture), args.options);
      const finished = output.summary.then(summary => {
        results.set(args.requestId, { outcome: "success", complete: true, summary, operationId: diagnostics().host.requestId });
      }, error => {
        results.set(args.requestId, { outcome: error.document?.kind ?? "runtime", complete: false, error: error.document ?? null, ...failureTypes(error) });
      });
      ctx.waitUntil(tracker.defer(finished));
      if (diagnostics().lease.acquired !== before.lease.acquired + 1) throw new Error("operation identity unavailable");
      return tracker.track(new Response(output.body, { headers: { "content-type": output.contentType } }), complete);
    } catch (error) {
      results.set(args.requestId, { outcome: error.document?.kind ?? "runtime", complete: true, error: error.document ?? null, operationId: diagnostics().host.requestId, ...failureTypes(error) });
      return tracker.track(Response.json({ refused: true }, { status: error.document ? 422 : 500 }), complete);
    }
  },
};
