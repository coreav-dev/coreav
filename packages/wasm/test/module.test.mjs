import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import * as api from "../src/index.js";
import { runDocumentInMemory } from "../wasm/coreav.js";
import { collect, fakeRangeSource } from "./helpers.mjs";
import { serve } from "./http-server.mjs";
import { moduleHash } from "../scripts/fingerprint.mjs";
import { fileURLToPath } from "node:url";

const fixtures = process.env.COREAV_FIXTURES;
assert.ok(fixtures, "set COREAV_FIXTURES to the generated fixtures directory");
const read = (name) => new Uint8Array(readFileSync(join(fixtures, name)));
const basic = read("basic.mp4");
const chapters = read("chapters.mp4");

test("operations before init name the explicit entry points", async () => {
  assert.throws(() => api.moduleInfo(), /not loaded/);
  await assert.rejects(api.probe(api.bytesSource(basic)), /not loaded/);
});

test("init accepts a compiled module and later calls do not replace it", () => {
  assert.equal(typeof WebAssembly.Suspending, "function");
  assert.equal(typeof WebAssembly.promising, "function");
  const module = new WebAssembly.Module(readFileSync(new URL("../wasm/coreav_bg.wasm", import.meta.url)));
  api.init(module);
  const version = JSON.parse(readFileSync(new URL("../package.json", import.meta.url))).version;
  assert.equal(api.moduleInfo().version, version);
  assert.equal(api.moduleInfo().build, moduleHash(fileURLToPath(new URL("..", import.meta.url))));
  assert.ok(api.moduleInfo().memoryBytes > 0);
  api.init(new Uint8Array([0]));
  assert.equal(api.moduleInfo().version, version);
});

test("ffprobeText preserves the module's exact text and ffprobe parses it", async () => {
  for (const bytes of [basic, read("browser.webm")]) {
    const session = api.bytesSource(bytes).open();
    try {
      const expected = runDocumentInMemory("ffprobe", session, session.size, "{}");
      const text = await api.ffprobeText(api.bytesSource(bytes));
      assert.equal(text, expected);
      assert.equal(await api.ffprobeText(fakeRangeSource(bytes, { firstSpan: 1024 })), expected);
      const document = await api.ffprobe(api.bytesSource(bytes));
      assert.deepEqual(document, JSON.parse(text));
      assert.equal(document.format.filename, "-");
      assert.ok(document.streams.length > 0);
      assert.equal((await api.ffprobe(api.bytesSource(bytes), { filename: "clip.mp4" })).format.filename, "clip.mp4");
    } finally {
      session.close();
    }
  }
});

test("ffprobe accepts filename and limits rather than arbitrary command options", async () => {
  for (const option of ["format", "allow", "strict", "scanFragments", "scanFrameSizes", "argv"]) {
    await assert.rejects(api.ffprobe(api.bytesSource(basic), { [option]: true }), (error) => {
      assert.ok(error instanceof TypeError);
      assert.match(error.message, new RegExp(option));
      return true;
    });
  }
  await assert.rejects(api.ffprobe(api.bytesSource(basic), { limits: { maxTracks: 1 } }), (error) => {
    assert.ok(error instanceof api.CoreAVError);
    assert.equal(error.kind, "limit");
    return true;
  });
});

test("probe rejects retired format and filename options", async () => {
  for (const options of [{ format: "ffprobe-compat" }, { filename: "clip.mp4" }]) {
    await assert.rejects(api.probe(api.bytesSource(basic), options), TypeError);
  }
  assert.equal((await api.probe(api.bytesSource(basic))).schema, "coreav.probe/1");
});

async function result(run, source) {
  const output = await run(source);
  if (output.body) {
    const bytes = await collect(output.body);
    return { bytes, summary: await output.summary, contentType: output.contentType };
  }
  return output;
}

const policy = JSON.parse(readFileSync(new URL("../../../docs/schema/policy-examples/web-upload.json", import.meta.url)));
const operations = [
  ["probe", api.probe],
  ["probeText", api.probeText],
  ["ffprobe", api.ffprobe],
  ["ffprobeText", api.ffprobeText],
  ["check", (source) => api.check(source, { policy })],
  ["chapters", api.chapters],
  ["extractSubtitles", api.extractSubtitles],
  ["extractAudio", api.extractAudio],
  ["clip", (source) => api.clip(source, { start: 0, end: 1 })],
  ["remux", api.remux],
  ["sanitize", api.sanitize],
  ["hlsIndex", (source) => api.hlsIndex(source, { segmentDuration: 1 })]
];

for (const [name, run] of operations) {
  test(`memory and range paths agree for ${name}`, async () => {
    const memory = await result(run, api.bytesSource(chapters));
    const ranged = await result(run, fakeRangeSource(chapters, { firstSpan: 1024, maxSkip: 8192 }));
    assert.deepEqual(ranged, memory);
  });
}

test("HLS resources agree between memory and range paths", async () => {
  const { index } = await api.hlsIndex(api.bytesSource(basic), { segmentDuration: 1 });
  const queue = ["master.m3u8"];
  const seen = new Set(queue);
  let segments = 0;
  while (queue.length) {
    const name = queue.shift();
    const memory = await result((source) => api.hlsGet(index, name, source), api.bytesSource(basic));
    const ranged = await result((source) => api.hlsGet(index, name, source), fakeRangeSource(basic, { firstSpan: 1024 }));
    assert.deepEqual(ranged, memory, name);
    if (name.endsWith(".m4s")) segments += 1;
    if (name.endsWith(".m3u8")) {
      const playlist = new TextDecoder().decode(memory.bytes);
      const names = [...playlist.matchAll(/URI="([^"]+)"/g)].map((match) => match[1]);
      names.push(...playlist.split(/\r?\n/).filter((line) => line && !line.startsWith("#")));
      for (const resource of names) {
        if (!seen.has(resource)) {
          seen.add(resource);
          queue.push(resource);
        }
      }
    }
  }
  assert.ok(segments > 0);
});

test("module errors retain exact document text through HTTP responses", async () => {
  const error = await api.probe(api.bytesSource(new Uint8Array())).then(() => null, (error) => error);
  assert.ok(error instanceof api.CoreAVError);
  assert.deepEqual(JSON.parse(error.documentText), error.document);
  const response = api.errorResponse(error);
  assert.equal(response.status, 422);
  assert.equal(await response.text(), error.documentText);
});

test("HTTP and memory sources give the same module result", async () => {
  const server = await serve(basic);
  try {
    assert.deepEqual(await api.probe(api.httpSource(server.url)), await api.probe(api.bytesSource(basic)));
  } finally {
    await server.close();
  }
});

test("runtime root exports agree with its declarations", () => {
  const text = readFileSync(new URL("../src/index.d.ts", import.meta.url), "utf8");
  const declared = [...text.matchAll(/export (?:function|const|class) (\w+)/g)].map((match) => match[1]).sort();
  assert.deepEqual(Object.keys(api).sort(), declared);
  assert.equal("r2Source" in api, false);
});
