import assert from "node:assert/strict";
import { test } from "node:test";
import { createEngine } from "../../wasm/src/engine-core.js";
import { bytesSource } from "../../wasm/src/sources.js";
import { r2Source } from "../../wasm/src/r2.js";
import { CoreAVError } from "../../wasm/src/errors.js";
import { runHostCases } from "./installed-host-cases.mjs";

test("installed host cases exercise real source leases without a media decoder", async () => {
  async function document(_name, source) {
    try { await source.read(0, 4); }
    catch { throw JSON.stringify({ schema: "coreav.error/1", kind: "io", limit: null, offset: null, message: "read failed" }); }
    return "{}";
  }
  const engine = createEngine({
    initSync: () => ({ memory: { buffer: new ArrayBuffer(65536) } }),
    version: () => "test",
    runDocument: document,
    async runStream(name, source, size, memory, sink) {
      await document(name, source);
      await sink.write(new Uint8Array([1, 2, 3]));
      return "{}";
    },
  }, "test", { exclusive: true });
  const saved = [WebAssembly.Suspending, WebAssembly.promising, WebAssembly.validate];
  WebAssembly.Suspending = function () {};
  WebAssembly.promising = function () {};
  WebAssembly.validate = () => true;
  try { engine.init({}); }
  finally { [WebAssembly.Suspending, WebAssembly.promising, WebAssembly.validate] = saved; }
  const api = {
    CoreAVError, bytesSource, r2Source,
    thumbnail: (source, options) => engine.runToStream("thumbnail", source, options),
    probe: (source) => engine.runToText("probe", source, {}),
  };
  const bytes = new Uint8Array([1, 2, 3, 4, 5]);
  const env = { FIXTURES: { async fetch(_url, request) {
    const match = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range);
    const start = Number(match[1]), end = Math.min(Number(match[2]), bytes.length - 1);
    return new Response(bytes.slice(start, end + 1), { status: 206, headers: {
      "content-range": `bytes ${start}-${end}/${bytes.length}`, etag: "test",
    } });
  } } };
  const source = (fixture, held) => r2Source({ async get(key, request) {
    if (held && !held.entered) { held.entered = true; held.notify(); await held.wait; }
    const { offset, length } = request.range;
    const response = await env.FIXTURES.fetch(key, { headers: { range: `bytes=${offset}-${offset + length - 1}` } });
    return { body: response.body, size: bytes.length, etag: "test", range: { offset } };
  } }, fixture);
  async function drain(output) {
    const reader = output.body.getReader();
    while (!(await reader.read()).done) {}
    return output.summary;
  }
  const result = await runHostCases({ api, diagnostics: engine.diagnostics, source, drain, env, fixture: "synthetic", options: { at: "0" } });
  assert.equal(result.complete, true);
  assert.deepEqual(result.cases.map(row => [row.id, row.outcome]), [["short-read", "io"], ["giant-backing", "limit"], ["cancellation", "cancelled"]]);
  assert.ok(result.cases.every(row => row.complete && row.settled && row.benignReuse));
  assert.deepEqual(result.cases[2].stages, ["opening", "output-backpressure"]);
  assert.equal(engine.diagnostics().settled, true);
});
