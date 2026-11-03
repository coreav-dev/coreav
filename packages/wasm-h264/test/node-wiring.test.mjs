import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, cpSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const packageRoot = fileURLToPath(new URL("..", import.meta.url));

test("node wiring keeps admission, causes, cleanup, and an idle process exit", { timeout: 12_000 }, async () => {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-h264-wire-"));
  const control = join(scratch, "control.json");
  const inits = join(scratch, "inits.txt");
  try {
    cpSync(join(packageRoot, "src"), join(scratch, "src"), { recursive: true });
    mkdirSync(join(scratch, "wasm"));
    writeFileSync(join(scratch, "wasm/coreav_bg.wasm"), Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]));
    writeFileSync(join(scratch, "wasm/build.js"), 'export const BUILD = "wire-build";\n');
    writeFileSync(join(scratch, "wasm/coreav.js"), fakeBindings(control, inits));
    writeFileSync(control, JSON.stringify({ mode: "ok" }));
    writeFileSync(inits, "");
    writeFileSync(join(scratch, "harness.mjs"), harness());
    writeFileSync(join(scratch, "poison.mjs"), poisonHarness());
    writeFileSync(join(scratch, "idle.mjs"), idleHarness());
    const env = {
      ...process.env,
      COREAV_WIRE_CONTROL: control,
      COREAV_WIRE_INITS: inits,
    };
    const functional = await runNode(join(scratch, "harness.mjs"), env, 9_000);
    const idle = await runNode(join(scratch, "idle.mjs"), env, 2_500);
    const poison = await runNode(join(scratch, "poison.mjs"), env, 9_000);
    const functionalReport = functional.status === 0 ? JSON.parse(functional.stdout.trim().split("\n").at(-1)) : null;
    const poisonReport = poison.status === 0 ? JSON.parse(poison.stdout.trim().split("\n").at(-1)) : null;
    const problems = [];
    if (functional.status !== 0) problems.push(`functional exit ${functional.status}\n${functional.stderr || functional.stdout}`);
    else if (functionalReport.failures.length !== 0 || functionalReport.unhandled !== 0) problems.push(`functional ${JSON.stringify(functionalReport)}`);
    if (idle.killed || idle.status !== 0) problems.push(`idle killed=${idle.killed} status=${idle.status}\n${idle.stderr || idle.stdout}`);
    if (poison.status !== 0) problems.push(`poison exit ${poison.status}\n${poison.stderr || poison.stdout}`);
    else if (poisonReport.failures.length !== 0 || poisonReport.unhandled !== 0) problems.push(`poison ${JSON.stringify(poisonReport)}`);
    assert.deepEqual(problems, []);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

test("a file worker strips both input-type spellings from an eval caller", { timeout: 8_000 }, async () => {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-h264-eval-"));
  try {
    const control = join(scratch, "control.json");
    const inits = join(scratch, "inits.txt");
    cpSync(join(packageRoot, "src"), join(scratch, "src"), { recursive: true });
    mkdirSync(join(scratch, "wasm"));
    writeFileSync(join(scratch, "wasm/coreav_bg.wasm"), Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]));
    writeFileSync(join(scratch, "wasm/build.js"), 'export const BUILD = "eval-build";\n');
    writeFileSync(join(scratch, "wasm/coreav.js"), fakeBindings(control, inits));
    writeFileSync(control, JSON.stringify({ mode: "ok" }));
    writeFileSync(inits, "");
    const code = `await import(${JSON.stringify(pathToFileURL(join(scratch, "src/node.js")).href)});`;
    for (const flags of [["--input-type=module", "--eval"], ["--input-type", "module", "--eval"]]) {
      const result = await runNode(code, process.env, 3_000, flags);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.killed, false);
    }
    assert.equal(readFileSync(inits, "utf8"), "init\ninit\n");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

function runNode(script, env, timeout, flags = []) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [...flags, script], { env, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => child.kill(), timeout);
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("exit", (status, signal) => {
      clearTimeout(timer);
      resolve({ status, signal, killed: signal !== null, stdout, stderr });
    });
  });
}

function fakeBindings(control, inits) {
  return `import { appendFileSync, readFileSync } from "node:fs";
const controlPath = ${JSON.stringify(control)};
const initPath = ${JSON.stringify(inits)};
installJspi();
function installJspi() {
  if (typeof WebAssembly.Suspending !== "function") WebAssembly.Suspending = function Suspending(fn) { this.fn = fn; };
  if (typeof WebAssembly.promising !== "function") WebAssembly.promising = function promising(fn) { return fn; };
  if (typeof WebAssembly.Tag !== "function") WebAssembly.Tag = function Tag() {};
  if (!(WebAssembly.JSTag instanceof WebAssembly.Tag)) WebAssembly.JSTag = new WebAssembly.Tag();
  WebAssembly.validate = () => true;
}
function mode() {
  return JSON.parse(readFileSync(controlPath, "utf8")).mode;
}
const memory = new WebAssembly.Memory({ initial: 1 });
export function initSync() {
  appendFileSync(initPath, "init\\n");
  return { memory };
}
export function version() { return "wire"; }
export function runDocumentInMemory(operation) {
  return JSON.stringify({ schema: operation === "probe" ? "coreav.probe/1" : "coreav.document/1" });
}
export function runDocument(operation) {
  return runDocumentInMemory(operation);
}
export async function runStream(_operation, session, _size, _inMemory, sink) {
  const selected = mode();
  if (selected === "block") Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 2000);
  if (selected === "read") await session.read(0, 4);
  const chunk = new Uint8Array([9, 8, 7, 6]);
  await sink.write(chunk);
  if (selected === "hold") await new Promise((resolve) => setTimeout(resolve, 800));
  return JSON.stringify({ schema: "coreav.thumbnail/1", output: { bytes: 4, format: "jpeg", width: 320, height: 1 } });
}
export function runIndexedStream() { throw new Error("unused"); }
`;
}

function harness() {
  return `import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { CoreAVError } from "./src/errors.js";
import { boundedOpeners } from "./src/bounded-source.js";
import { TRUSTED } from "./src/internal.js";
const unhandled = [];
process.on("unhandledRejection", (error) => unhandled.push(error?.message ?? String(error)));
const api = await import("./src/node.js");
const failures = [];
function check(name, fn) {
  return fn().then(() => {}, (error) => failures.push(name + ": " + (error?.stack ?? error)));
}
const bytes = api.bytesSource(new Uint8Array([1, 2, 3, 4]));
await check("thumbnail-and-probe", async () => {
  writeFileSync(process.env.COREAV_WIRE_CONTROL, JSON.stringify({ mode: "ok" }));
  const shot = await api.thumbnail(bytes, { at: "1" });
  const body = new Uint8Array(await new Response(shot.body).arrayBuffer());
  assert.deepEqual([...body], [9, 8, 7, 6]);
  const summary = await shot.summary;
  assert.equal(summary.schema, "coreav.thumbnail/1");
  const probed = await api.probe(bytes);
  assert.equal(probed.schema, "coreav.probe/1");
  assert.equal(readFileSync(process.env.COREAV_WIRE_INITS, "utf8").trim().split("\\n").length, 1);
});
await check("ranged-read-cause", async () => {
  writeFileSync(process.env.COREAV_WIRE_CONTROL, JSON.stringify({ mode: "read" }));
  const cause = new Error("backend-detail");
  const source = { open() {} };
  boundedOpeners.set(source, async () => ({
    [TRUSTED]: true,
    size: 8,
    inMemory: false,
    read() {
      return Promise.reject(new CoreAVError({
        schema: "coreav.error/1", kind: "io", limit: null, offset: 3, message: "the range read failed",
      }, { cause }));
    },
    readNow() { return 0; },
    plan() {},
    planRegions() {},
    close() {},
  }));
  await assert.rejects(api.thumbnail(source, { at: "1", deadline: 2000 }), (error) => {
    assert.equal(error instanceof CoreAVError, true, error?.stack ?? String(error));
    assert.equal(error.kind, "io");
    assert.equal(error.message, "the range read failed");
    assert.equal(error.cause?.message, "backend-detail");
    return true;
  });
});
await check("cancel-output", async () => {
  writeFileSync(process.env.COREAV_WIRE_CONTROL, JSON.stringify({ mode: "hold" }));
  const shot = await api.thumbnail(bytes, { at: "1", deadline: 2000 });
  await shot.body.cancel(new Error("stop"));
  await assert.rejects(shot.summary);
});
await check("deadline-kills-blocked-worker", async () => {
  writeFileSync(process.env.COREAV_WIRE_CONTROL, JSON.stringify({ mode: "block" }));
  const before = readFileSync(process.env.COREAV_WIRE_INITS, "utf8").trim().split("\\n").filter(Boolean).length;
  const started = Date.now();
  await assert.rejects(api.thumbnail(bytes, { at: "1", deadline: 400 }), (error) => {
    assert.equal(error?.kind, "limit");
    assert.equal(error?.limit, "timeout");
    return true;
  });
  assert.ok(Date.now() - started < 1200, "deadline waited for the blocked worker");
  writeFileSync(process.env.COREAV_WIRE_CONTROL, JSON.stringify({ mode: "ok" }));
  await api.probe(bytes);
  const after = readFileSync(process.env.COREAV_WIRE_INITS, "utf8").trim().split("\\n").filter(Boolean).length;
  assert.equal(after, before + 1);
});
await check("abort-message-is-not-the-caller-reason", async () => {
  writeFileSync(process.env.COREAV_WIRE_CONTROL, JSON.stringify({ mode: "hold" }));
  const controller = new AbortController();
  const run = api.thumbnail(bytes, { at: "1", signal: controller.signal, deadline: 2000 });
  controller.abort(new Error("credential-token"));
  await assert.rejects(run, (error) => {
    assert.equal(error?.name, "AbortError");
    assert.equal(String(error?.message).includes("credential-token"), false);
    return true;
  });
});
console.log(JSON.stringify({ failures, unhandled: unhandled.length, unhandledMessages: unhandled }));
process.exit(failures.length === 0 && unhandled.length === 0 ? 0 : 1);
`;
}

function poisonHarness() {
  return `import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { CoreAVError } from "./src/errors.js";
import { boundedOpeners } from "./src/bounded-source.js";
import { TRUSTED } from "./src/internal.js";
const unhandled = [];
process.on("unhandledRejection", (error) => unhandled.push(error?.message ?? String(error)));
const api = await import("./src/node.js");
const failures = [];
const bytes = api.bytesSource(new Uint8Array([1, 2, 3, 4]));
try {
  writeFileSync(process.env.COREAV_WIRE_CONTROL, JSON.stringify({ mode: "ok" }));
  let closed = false;
  const source = { open() {} };
  boundedOpeners.set(source, async () => ({
    [TRUSTED]: true,
    size: 4,
    inMemory: true,
    readInto(offset, view) { view.set([1, 2, 3, 4].slice(offset, offset + view.length)); return Math.min(view.length, 4 - offset); },
    async close() {
      await new Promise((resolve) => setTimeout(resolve, 400));
      closed = true;
      throw new Error("close failed");
    },
  }));
  const shot = await api.thumbnail(source, { at: "1", deadline: 2000 });
  let bodyState = "pending";
  let summaryState = "pending";
  let bodyError = null;
  let summaryError = null;
  const bodyDone = new Response(shot.body).arrayBuffer().then(
    () => { bodyState = "success"; },
    (error) => { bodyState = "failure"; bodyError = error; },
  );
  const summaryDone = shot.summary.then(
    () => { summaryState = "success"; },
    (error) => { summaryState = "failure"; summaryError = error; },
  );
  await new Promise((resolve) => setTimeout(resolve, 150));
  assert.equal(bodyState, "pending");
  assert.equal(summaryState, "pending");
  assert.equal(closed, false);
  await bodyDone;
  await summaryDone;
  assert.equal(closed, true);
  assert.equal(bodyState, "failure");
  assert.equal(summaryState, "failure");
  assert.equal(bodyError instanceof CoreAVError, true, bodyError?.stack ?? String(bodyError));
  assert.equal(bodyError.kind, "io");
  assert.equal(bodyError.message, "the source could not be closed");
  assert.equal(bodyError.cause?.message, "close failed");
  assert.equal(summaryError instanceof CoreAVError, true, summaryError?.stack ?? String(summaryError));
  assert.equal(summaryError.kind, "io");
  assert.equal(summaryError.message, "the source could not be closed");
  assert.equal(summaryError.cause?.message, "close failed");
  await assert.rejects(api.thumbnail(bytes, { at: "1", deadline: 2000 }), (error) => {
    assert.equal(error instanceof Error, true, error?.stack ?? String(error));
    assert.equal(error.message, "CoreAV source cleanup failed and the instance cannot run again");
    assert.equal(error.cause, bodyError);
    return true;
  });
} catch (error) {
  failures.push("close-before-summary: " + (error?.stack ?? error));
}
console.log(JSON.stringify({ failures, unhandled: unhandled.length, unhandledMessages: unhandled }));
process.exit(failures.length === 0 && unhandled.length === 0 ? 0 : 1);
`;
}

function idleHarness() {
  return `const api = await import("./src/node.js");
await api.probe(api.bytesSource(new Uint8Array([1, 2, 3, 4])));
`;
}
