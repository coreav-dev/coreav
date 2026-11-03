import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

test("the installed worker creates its stable isolate identity only inside a request", () => {
  const result = spawnSync(process.execPath, ["--experimental-vm-modules", "--input-type=module", "-e", `
    import assert from "node:assert/strict";
    import { readFileSync } from "node:fs";
    import { createContext, SourceTextModule } from "node:vm";
    let inRequest = false, randomCalls = 0;
    const context = createContext({
      Request, Response, URL, Headers, ReadableStream, DOMException, setTimeout, clearTimeout,
      crypto: { randomUUID() {
        assert.equal(inRequest, true, "randomness is forbidden during module evaluation");
        randomCalls += 1;
        return "request-created-isolate";
      } },
    });
    const sources = {
      "@coreav/wasm-h264/workers": 'export const moduleInfo = () => ({ build: "test", memoryBytes: 65536 }); export const r2Source = () => ({}); export async function thumbnail() { throw new WebAssembly.RuntimeError("private-marker"); }',
      "H264_ENGINE": 'export const diagnostics = () => ({ settled: true, host: { requestId: 1 } });',
      "HOST_CASES": 'export const runHostCases = () => { throw new Error("media execution is forbidden"); };',
      "./batch.mjs": readFileSync(process.argv[2], "utf8"),
    };
    const worker = new SourceTextModule(readFileSync(process.argv[1], "utf8"), { context });
    await worker.link(name => {
      assert.ok(Object.hasOwn(sources, name));
      return new SourceTextModule(sources[name], { context });
    });
    await worker.evaluate();
    assert.equal(randomCalls, 0);
    inRequest = true;
    const first = await (await worker.namespace.default.fetch(new Request("http://worker/memory"))).json();
    const second = await (await worker.namespace.default.fetch(new Request("http://worker/memory"))).json();
    inRequest = false;
    assert.equal(randomCalls, 1);
    assert.equal(first.isolate, "request-created-isolate");
    assert.deepEqual(second, first);
    assert.equal(first.served, 0);
    assert.equal(first.inFlight, 0);
    assert.equal(first.pending, 0);
    inRequest = true;
    await (await worker.namespace.default.fetch(new Request("http://worker/thumbnail", { method: "POST", body: JSON.stringify({ requestId: "typed-fault", fixture: "no-media", options: {} }) }))).json();
    const failed = await (await worker.namespace.default.fetch(new Request("http://worker/result/typed-fault"))).text();
    assert.equal(JSON.parse(failed).failureType, "wasm_trap");
    assert.equal(JSON.parse(failed).causeType, "none");
    assert.equal(failed.includes("private-marker"), false);
    inRequest = false;
  `, fileURLToPath(new URL("./worker.js", import.meta.url)), fileURLToPath(new URL("../../../wasm/examples/test/batch.mjs", import.meta.url))], { encoding: "utf8", timeout: 5000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
});
