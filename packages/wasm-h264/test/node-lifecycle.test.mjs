import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const bindings = `
WebAssembly.Suspending = function () {};
WebAssembly.promising = function () {};
WebAssembly.validate = () => true;
WebAssembly.JSTag ??= new WebAssembly.Tag({ parameters: [] });
export function initSync() { return { memory: new WebAssembly.Memory({ initial: 1 }) }; }
export function version() { return "lifecycle"; }
export function runDocument() { return "{}"; }
export function runDocumentInMemory() { return "{}"; }
export async function runStream(_operation, source, _size, _memory, sink) {
  try {
    if (source.readInto(0, new Uint8Array(1)) === 0) await source.read(0, 1);
  } catch {
    throw JSON.stringify({ schema: "coreav.error/1", kind: "io", limit: null, offset: null, message: "controlled read failure" });
  }
  for (let index = 0; index < 4; index += 1) await sink.write(new Uint8Array([index]));
  return '{"schema":"coreav.thumbnail/1"}';
}
export function runIndexedStream() { throw new Error("unused"); }
`;

function run(code, timeout = 5_000) {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-h264-lifecycle-"));
  try {
    cpSync(fileURLToPath(new URL("../src", import.meta.url)), join(scratch, "src"), { recursive: true });
    mkdirSync(join(scratch, "wasm"));
    writeFileSync(join(scratch, "wasm/coreav_bg.wasm"), new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]));
    writeFileSync(join(scratch, "wasm/build.js"), 'export const BUILD = "lifecycle";\n');
    writeFileSync(join(scratch, "wasm/coreav.js"), bindings);
    const script = join(scratch, "case.mjs");
    writeFileSync(script, 'import assert from "node:assert/strict";\nimport * as api from "./src/node.js";\n' + code);
    const result = spawnSync(process.execPath, [script], { encoding: "utf8", timeout });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, null, result.stderr);
    assert.equal(result.status, 0, result.stderr);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

test("cancelling with a worker chunk in flight does not crash the parent", () => {
  run(`
const output = await api.thumbnail(api.bytesSource(new Uint8Array([1])), { at: 0, deadline: 2000 });
output.summary.catch(() => {});
const reader = output.body.getReader();
await reader.read();
const pending = reader.read();
for (let index = 0; index < 10; index += 1) await Promise.resolve();
Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 100);
const cancelled = reader.cancel();
await Promise.allSettled([pending, cancelled, output.summary]);
await assert.rejects(output.summary);
await api.probe(api.bytesSource(new Uint8Array([1])));
`);
});

test("a missing HTTP source retains its public error class across both brokers", () => {
  run(`
import { createServer } from "node:http";
import { once } from "node:events";
const server = createServer((_request, response) => { response.writeHead(404); response.end(); });
server.listen(0, "127.0.0.1");
await once(server, "listening");
try {
  const source = api.httpSource("http://127.0.0.1:" + server.address().port + "/missing");
  await assert.rejects(api.probe(source), error => {
    assert.ok(error instanceof api.SourceNotFoundError);
    assert.equal(api.errorResponse(error).status, 404);
    return true;
  });
} finally {
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
`);
});

test("the installed Node lifecycle driver exercises every controlled stage", () => {
  run(`
import { lifecycle } from ${JSON.stringify(new URL("./installed/node.mjs", import.meta.url).href)};
import { boundedOpeners } from "./src/bounded-source.js";
await lifecycle(api, boundedOpeners, new Uint8Array([1]), 500);
`, 15_000);
});
