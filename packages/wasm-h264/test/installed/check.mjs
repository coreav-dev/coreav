import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { randomUUID, createHash } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import { createReadStream, appendFileSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { registeredBars } from "../../../wasm/examples/test/registry.mjs";
import { archiveEntries, validateArchive, consumerEnv } from "../../../wasm/scripts/check-package.mjs";
import { profile } from "../../scripts/profile.mjs";
import { fixtureRoster, gateRoster, parityRoster, roster, rosterSha256, sha, optionsSha256 } from "./roster.mjs";
import { ThumbnailLedger, validateParity, validateHostCases, failureRecord } from "./ledger.mjs";

let captureOutput;
async function main() {
if (process.platform !== "linux") throw new Error("installed thumbnail media preflight is Linux-only");
const args = process.argv.slice(2), settings = {};
while (args.length) {
  const name = args.shift();
  if (!["--fixtures", "--tarball", "--package-record", "--edge-native", "--allocation-report", "--allocation-binary", "--host-cases", "--output"].includes(name) || !args.length || settings[name]) throw new Error("invalid installed thumbnail preflight arguments");
  settings[name] = resolve(args.shift());
}
for (const name of ["--fixtures", "--tarball", "--package-record", "--edge-native", "--allocation-report", "--allocation-binary", "--host-cases", "--output"]) assert.ok(settings[name], `missing ${name}`);
const repo = fileURLToPath(new URL("../../../..", import.meta.url));
const output = settings["--output"];
assert.ok(relative(repo, output).startsWith("..") || isAbsolute(relative(repo, output)), "private captures must be outside the checkout");
mkdirSync(output, { recursive: true });
captureOutput = output;
const require = createRequire(new URL("../../../wasm/examples/package.json", import.meta.url));
const { build, stop: stopBundler, version: esbuildVersion } = await import(pathToFileURL(require.resolve("esbuild")));
const workerd = require.resolve(`@cloudflare/workerd-linux-${process.arch === "x64" ? "64" : "arm64"}/bin/workerd`);
const dependencies = JSON.parse(readFileSync(fileURLToPath(new URL("../../../wasm/examples/package.json", import.meta.url)))).devDependencies;
const workerdVersion = require("workerd/package.json").version;
assert.equal(esbuildVersion, dependencies.esbuild); assert.equal(workerdVersion, dependencies.workerd);
const nodePins = new Set([...readFileSync(resolve(repo, ".github/workflows/wasm.yml"), "utf8").matchAll(/node-version:\s*['"]([\d.]+)['"]/g)].map(match => match[1]));
assert.equal(nodePins.size, 1, "workflow must name one Node pin"); assert.equal(process.versions.node, [...nodePins][0]);
const workerdBinarySha256 = sha(readFileSync(workerd));
const fixtureRoot = settings["--fixtures"];
const manifestBytes = readFileSync(join(fixtureRoot, "manifest.json"));
const manifest = JSON.parse(manifestBytes);
assert.equal(manifest.rosterSha256, rosterSha256, "fixture roster drifted");
assert.equal(manifest.schema, roster.schema);
assert.equal(manifest.generatorSha256, sha(readFileSync(resolve(repo, "scripts/wasm-thumbnail-fixtures.py"))), "fixture generator changed");
assert.equal(manifest.oraclePinSha256, sha(readFileSync(resolve(repo, "scripts/oracle-ffmpeg.env"))), "oracle pin changed");
assert.equal(manifest.generationSha256, sha(readFileSync(join(fixtureRoot, "generation.json"))), "generation invocation record changed");
assert.deepEqual(manifest.fixtures.map(row => row.id), fixtureRoster().map(row => row.id), "fixture roster is incomplete, reordered or duplicated");
for (const [index, expected] of fixtureRoster().entries()) {
  const row = manifest.fixtures[index];
  for (const name of ["geometry", "entropy", "container"]) assert.equal(row[name], expected[name], "fixture labels drifted");
  const geometry = roster.geometries.find(item => item.id === row.geometry) ?? (row.geometry.startsWith("4k") ? { width: 3840, height: 2160 } : { width: 640, height: 360 });
  assert.equal(row.codedWidth, Math.ceil(geometry.width / 16) * 16);
  assert.equal(row.codedHeight, Math.ceil(geometry.height / 16) * 16);
  assert.equal(row.visibleWidth, geometry.width);
  assert.equal(row.visibleHeight, row.geometry === "coded-crop" ? 1080 : geometry.height);
}
assert.deepEqual(manifest.hostile.map(row => row.id).sort(), [...roster.hostile].sort(), "required hostile fixture roster is incomplete or duplicated");
assert.deepEqual(manifest.boundary.map(row => row.id).sort(), [...roster.boundaries].sort(), "required valid boundary fixture roster is incomplete or duplicated");
const files = new Map([...manifest.fixtures, ...manifest.hostile, ...manifest.boundary].map(row => [row.id, row]));
for (const row of files.values()) {
  assert.match(row.sha256, /^[a-f0-9]{64}$/);
  assert.ok(typeof row.path === "string" && !isAbsolute(row.path) && !row.path.split(/[\\/]/).includes(".."));
  assert.equal(sha(readFileSync(join(fixtureRoot, row.path))), row.sha256, "fixture bytes changed before capture");
}
const archive = readFileSync(settings["--tarball"]), tarballSha256 = sha(archive);
const packed = validateArchive(archiveEntries(archive), profile.packed);
const artifactSha256 = sha(packed.get("package/wasm/coreav_bg.wasm"));
const packageRecordBytes = readFileSync(settings["--package-record"]), packageRecord = JSON.parse(packageRecordBytes);
assert.equal(packageRecord.tarballSha256, tarballSha256);
assert.equal(packageRecord.sha256["wasm/coreav_bg.wasm"], artifactSha256);
assert.equal(packageRecord.provenance.wasmSha256, artifactSha256);
assert.equal(packageRecord.provenance.target, "wasm32-unknown-unknown");
assert.equal(packageRecord.provenance.profile, "edge");
assert.deepEqual(packageRecord.provenance.features, ["native-h264"]);
assert.ok(Number.isSafeInteger(packageRecord.provenance.simdInstructions) && packageRecord.provenance.simdInstructions > 0);
const rustPin = /^RUST_DIST_TOOLCHAIN=(.+)$/m.exec(readFileSync(resolve(repo, "scripts/rust-dist.env"), "utf8"))?.[1];
const binaryenPin = /^BINARYEN_VERSION=(\d+)$/m.exec(readFileSync(resolve(repo, "scripts/binaryen.env"), "utf8"))?.[1];
const bindgenPin = /^wasm-bindgen = "=([^"]+)"$/m.exec(readFileSync(resolve(repo, "crates/coreav-wasm/Cargo.toml"), "utf8"))?.[1];
assert.ok(rustPin && binaryenPin && bindgenPin);
assert.ok(packageRecord.provenance.rustc.startsWith(`rustc ${rustPin} `));
assert.equal(packageRecord.provenance.wasmBindgen, bindgenPin);
assert.match(packageRecord.provenance.wasmOpt, new RegExp(`^wasm-opt version ${binaryenPin}(?:\\s|$)`));
const allocationBinarySha256 = sha(readFileSync(settings["--allocation-binary"]));
const allocation = JSON.parse(readFileSync(settings["--allocation-report"]));
const allocationReportSha256 = sha(readFileSync(settings["--allocation-report"]));
assert.equal(allocation.schema, "coreav.thumbnail-allocations/1");
assert.equal(allocation.testBinarySha256, allocationBinarySha256);
assert.ok(Array.isArray(allocation.records), "native allocation witness records are required");
assert.deepEqual(allocation.records.map(row => row.id).sort(), fixtureRoster().filter(row => row.geometry.startsWith("4k")).map(row => row.id).sort(), "allocation witness roster drifted");
const allocationRecords = new Map();
for (const row of allocation.records) {
  assert.ok(!allocationRecords.has(row.id), "duplicate native allocation witness");
  assert.equal(row.binarySha256, allocationBinarySha256, "native allocation binary identity changed");
  allocationRecords.set(row.id, row);
}
const bars = registeredBars(readFileSync(resolve(repo, "docs/design/wasm-thumbnail-memory-bars.md"), "utf8"), "wasm-thumbnail-memory-bars");
const groups = gateRoster(), ledger = new ThumbnailLedger(bars, artifactSha256, tarballSha256, manifest.fixtures, groups);
const nativeSha256 = sha(readFileSync(settings["--edge-native"]));
const hostCasesSha256 = sha(readFileSync(settings["--host-cases"]));
const consumer = mkdtempSync(join(tmpdir(), "coreav-thumbnail-installed-"));
const workers = new Set(), runtimeLogs = [];
let server, comparisons = 0, finished = false, workerNumber = 0;
let stage = "install", item = 0, failure;
const recordsFile = join(output, "captures.ndjson");
writeFileSync(recordsFile, "");
writeFileSync(join(output, "verdict.json"), JSON.stringify({ state: "running", preflightOnly: true }) + "\n");

function record(value) { appendFileSync(recordsFile, JSON.stringify(value) + "\n"); }
function alive(worker) {
  assert.equal(worker.launchError, undefined, "runtime launch failed");
  assert.equal(worker.child.exitCode, null, "runtime exited");
  assert.equal(worker.child.signalCode, null, "runtime was terminated");
}
async function freePort() {
  const temporary = createServer(); temporary.listen(0, "127.0.0.1"); await once(temporary, "listening");
  const port = temporary.address().port;
  await new Promise(resolve => temporary.close(resolve)); return port;
}
async function json(worker, path, body) {
  alive(worker);
  const response = await fetch(worker.base + path, { ...(body ? { method: "POST", body: JSON.stringify(body) } : {}), signal: AbortSignal.timeout(30000) });
  assert.ok(response.ok, "runtime diagnostic unavailable");
  const result = await response.json(); alive(worker); return result;
}
async function reading(worker) {
  const info = await json(worker, "/memory");
  assert.equal(info.isolate, worker.isolate); assert.equal(info.build, artifactSha256);
  assert.equal(info.inFlight, 0); assert.equal(info.pending, 0); assert.equal(info.diagnostic.settled, true);
  assert.equal(info.diagnostic.lease.acquired, info.diagnostic.lease.released);
  if (info.diagnostic.host !== null) assert.equal(info.diagnostic.host.requestId, info.diagnostic.lease.acquired);
  return info;
}
async function start() {
  stage = "bundle";
  const directory = join(consumer, `worker-${workerNumber++}`); mkdirSync(directory);
  const bundle = await build({ absWorkingDir: consumer, entryPoints: [join(consumer, "worker.js")], outfile: join(directory, "bundle.js"), bundle: true, format: "esm", platform: "neutral", metafile: true,
    alias: { H264_ENGINE: join(consumer, "node_modules/@coreav/wasm-h264/src/engine.js"), HOST_CASES: join(consumer, "host-cases.js") },
    plugins: [{ name: "installed-module", setup(builder) { builder.onResolve({ filter: /\.wasm$/ }, args => {
      const path = resolve(args.resolveDir, args.path);
      assert.equal(sha(readFileSync(path)), artifactSha256);
      copyFileSync(path, join(directory, "coreav_bg.wasm"));
      return { path: "./coreav_bg.wasm", external: true };
    }); } }],
  });
  for (const path of Object.keys(bundle.metafile.inputs)) {
    const name = relative(consumer, resolve(consumer, path));
    assert.ok(!isAbsolute(name) && name !== ".." && !name.startsWith("../"), "bundle used repository source instead of installed package");
  }
  assert.equal(Object.keys(bundle.metafile.inputs).filter(name => name.endsWith("src/engine.js")).length, 1);
  assert.ok(!Object.keys(bundle.metafile.inputs).some(name => /node_modules\/@coreav\/wasm\//.test(name)), "default module also loaded");
  const port = await freePort();
  writeFileSync(join(directory, "config.capnp"), `using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
services = [(name = "thumbnail", worker = (compatibilityDate = "2026-11-01", modules = [(name = "bundle.js", esModule = embed "bundle.js"), (name = "coreav_bg.wasm", wasm = embed "coreav_bg.wasm")], bindings = [(name = "FIXTURES", service = "fixtures")], globalOutbound = "fixtures")), (name = "fixtures", external = (address = "127.0.0.1:${server.address().port}", http = ()))],
sockets = [(name = "http", address = "127.0.0.1:${port}", http = (), service = "thumbnail")]);\n`);
  stage = "runtime-launch";
  const child = spawn(workerd, ["serve", join(directory, "config.capnp")], { stdio: ["ignore", "pipe", "pipe"] });
  const worker = { child, base: `http://127.0.0.1:${port}` }; workers.add(worker);
  child.on("error", error => { worker.launchError = error; });
  child.stdout.on("data", chunk => runtimeLogs.push(String(chunk))); child.stderr.on("data", chunk => runtimeLogs.push(String(chunk)));
  stage = "runtime-startup";
  for (let attempt = 0; attempt < 100; attempt += 1) {
    alive(worker);
    let response;
    try { response = await fetch(worker.base + "/memory", { signal: AbortSignal.timeout(300) }); } catch {}
    if (response?.ok) {
      const info = await response.json(); assert.equal(info.build, artifactSha256);
      worker.isolate = info.isolate; worker.initial = info; return worker;
    }
    await delay(50);
  }
  throw new Error("runtime startup deadline");
}
async function stop(worker, strict = true) {
  workers.delete(worker);
  if (worker.launchError || worker.child.exitCode !== null || worker.child.signalCode !== null) {
    if (strict) throw new Error("runtime exited before shutdown");
    return;
  }
  const requested = worker.child.kill("SIGTERM");
  if (strict) assert.equal(requested, true, "runtime exited before shutdown request");
  const kill = setTimeout(() => worker.child.kill("SIGKILL"), 2000);
  try {
    const [code, signal] = await once(worker.child, "exit");
    if (strict) assert.ok(code === 0 || code === null && ["SIGTERM", "SIGKILL"].includes(signal), "runtime exited unexpectedly");
  } finally { clearTimeout(kill); }
}
function native(row) {
  const fixture = files.get(row.fixture), path = join(fixtureRoot, fixture.path), image = join(output, "native-image.bin");
  const options = { ...row.options };
  if (options.width === undefined && options.height === undefined) options.width = 320;
  assert.equal(sha(readFileSync(path)), fixture.sha256);
  const result = spawnSync(settings["--edge-native"], ["run", "thumbnail", path, JSON.stringify(options), image], { timeout: 15000, maxBuffer: 16 << 20, encoding: "utf8" });
  appendFileSync(join(output, "native.log"), result.stderr ?? "");
  assert.ifError(result.error); assert.ok(result.status === 0 || result.status === 1, "native runtime exited");
  assert.equal(sha(readFileSync(path)), fixture.sha256);
  const document = JSON.parse(result.stdout);
  if (result.status !== 0) assert.equal(statSync(image).size, 0, "native refusal wrote partial image bytes");
  const bytes = result.status === 0 ? readFileSync(image) : Buffer.alloc(0);
  return { complete: true, runtimeExit: false, inputSha256: fixture.sha256, optionsSha256: optionsSha256(row.options), requestOptions: row.options, resolvedOptions: options, outcome: result.status === 0 ? "success" : document.kind, bytes: bytes.length, outputSha256: bytes.length ? sha(bytes) : null, ...(result.status === 0 ? { summary: document } : { error: document }), binarySha256: nativeSha256 };
}
async function run(worker, row) {
  item += 1;
  const requestId = randomUUID(), fixture = files.get(row.fixture);
  stage = "before-memory";
  const before = await reading(worker);
  stage = "request";
  const response = await fetch(worker.base + (row.operation === "admission" ? "/admission" : "/thumbnail"), { method: "POST", body: JSON.stringify({ requestId, fixture: row.fixture, options: row.options, stage: row.id }), signal: AbortSignal.timeout(30000) });
  const hash = createHash("sha256"); let bytes = 0;
  stage = "response";
  for await (const chunk of response.body) { if (response.status === 200 && row.operation !== "admission") { hash.update(chunk); bytes += chunk.byteLength; } }
  stage = "result";
  const made = await json(worker, `/result/${requestId}`);
  stage = "after-memory";
  const info = await reading(worker);
  stage = "request-accounting";
  assert.equal(info.served, before.served + 1);
  assert.equal(made.isolate, worker.isolate); assert.equal(made.operationId, info.diagnostic.host.requestId);
  assert.equal(info.diagnostic.lease.acquired - before.diagnostic.lease.acquired, row.operation === "admission" ? 2 : 1);
  const actual = { ...made, requestId, inputSha256: fixture.sha256, optionsSha256: optionsSha256(row.options), options: row.options, bytes, outputSha256: bytes ? hash.digest("hex") : null, runtimeExit: false };
  if (row.operation !== "admission") {
    stage = "native-reference";
    const reference = native(row);
    record({ classification: "parity", id: row.id, ...actual, native: reference });
    stage = "parity";
    validateParity(actual, reference, { ...row, inputSha256: fixture.sha256 }); comparisons += 1;
  }
  stage = "host-accounting";
  const host = { ...info.diagnostic.host };
  if (made.admission) for (const snapshot of made.admission.hostSnapshots) {
    assert.equal(snapshot.liveBytes, 0);
    for (const name of ["maxBackingBytes", "maxChargedBytes", "maxOutputBackingBytes", "maxOutputChargedBytes"]) host[name] = Math.max(host[name], snapshot[name]);
  }
  return { ...actual, artifactSha256, tarballSha256, isolate: worker.isolate, memoryBytes: info.memoryBytes, settled: info.diagnostic.settled, pending: info.pending, inFlight: info.inFlight, served: info.served, host,
    ...(row.expectation === "limit" && row.fixture.startsWith("4k") ? { nativeAllocation: allocationRecords.get(row.fixture) } : {}) };
}

try {
  const checkedTarball = join(consumer, "checked.tgz");
  writeFileSync(checkedTarball, archive);
  writeFileSync(join(consumer, "package.json"), JSON.stringify({ name: "thumbnail-installed-preflight", private: true, type: "module", dependencies: { "@coreav/wasm-h264": `file:${checkedTarball}` } }));
  const installed = spawnSync("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--package-lock=false"], { cwd: consumer, env: consumerEnv(), timeout: 30000, encoding: "utf8" });
  assert.ifError(installed.error); assert.equal(installed.status, 0, "installed tarball setup failed");
  stage = "installed-files";
  const packageRoot = join(consumer, "node_modules/@coreav/wasm-h264");
  assert.equal(realpathSync(packageRoot), join(realpathSync(consumer), "node_modules/@coreav/wasm-h264"), "package is a source link");
  assert.equal(sha(readFileSync(join(packageRoot, "wasm/coreav_bg.wasm"))), artifactSha256);
  for (const [path, bytes] of packed) assert.equal(sha(readFileSync(join(packageRoot, path.slice(8)))), sha(bytes), "installed package bytes differ from checked archive");
  copyFileSync(fileURLToPath(new URL("./worker.js", import.meta.url)), join(consumer, "worker.js"));
  copyFileSync(settings["--host-cases"], join(consumer, "host-cases.js"));
  copyFileSync(fileURLToPath(new URL("../../../wasm/examples/test/batch.mjs", import.meta.url)), join(consumer, "batch.mjs"));
  stage = "fixture-server";
  server = createServer((request, response) => {
    const id = decodeURIComponent(new URL(request.url, "http://fixtures.test").pathname.slice(1)), fixture = files.get(id);
    if (!fixture) { response.writeHead(404).end(); return; }
    const path = join(fixtureRoot, fixture.path), size = statSync(path).size, range = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? "");
    if (!range) { response.writeHead(400).end(); return; }
    const start = Number(range[1]), end = Math.min(Number(range[2]), size - 1), etag = `"${fixture.sha256}"`;
    if (request.headers["if-match"] && request.headers["if-match"] !== etag) { response.writeHead(412).end(); return; }
    if (start > end || end >= size) { response.writeHead(416).end(); return; }
    response.writeHead(206, { etag, "content-length": end - start + 1, "content-range": `bytes ${start}-${end}/${size}` });
    const stream = createReadStream(path, { start, end, highWaterMark: 65536 });
    stream.on("error", () => response.destroy()); response.on("close", () => stream.destroy()); stream.pipe(response);
  });
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  for (const [gate, rows] of groups) {
    const worker = await start();
    stage = "fresh-isolate";
    ledger.begin(gate, worker.initial);
    for (const row of rows) {
      if (row.operation === "startup") item += 1;
      const result = row.operation === "startup" ? { requestId: randomUUID(), operationId: 0, served: 0, outcome: "startup", complete: true, runtimeExit: false, optionsSha256: optionsSha256({}), artifactSha256, tarballSha256, isolate: worker.isolate, memoryBytes: worker.initial.memoryBytes, inFlight: 0, pending: 0, settled: worker.initial.diagnostic.settled, host: worker.initial.diagnostic.host } : await run(worker, row);
      stage = "memory-ledger";
      record({ classification: "gate", gate, step: row.id, ...result }); ledger.record(gate, row.id, result);
    }
    stage = "shutdown";
    alive(worker); await stop(worker);
  }
  const parity = await start();
  for (const row of parityRoster()) await run(parity, row);
  for (const row of [...manifest.hostile, ...manifest.boundary]) {
    assert.ok(row.options && typeof row.expectation === "string", "fixture expectation unavailable");
    await run(parity, { ...row, operation: "thumbnail", fixture: row.id });
    await run(parity, { id: `${row.id}-reuse`, fixture: "360p-cavlc-mp4", options: { at: "0", format: "jpeg" }, expectation: "success" });
  }
  stage = "shutdown";
  alive(parity); await stop(parity);
  const hostWorker = await start(), hostBefore = await reading(hostWorker);
  stage = "host-cases"; item += 1;
  const hostReport = await json(hostWorker, "/host-cases", { requestId: randomUUID(), fixture: "360p-cavlc-mp4", options: { at: "0", format: "jpeg" } });
  const hostAfter = await reading(hostWorker);
  validateHostCases(hostReport, hostBefore.diagnostic, hostAfter.diagnostic);
  record({ classification: "host-cases", artifactSha256, tarballSha256, hostCasesSha256, isolate: hostWorker.isolate, report: hostReport, before: hostBefore, after: hostAfter });
  stage = "shutdown";
  alive(hostWorker); await stop(hostWorker);
  stage = "integrity";
  for (const fixture of files.values()) assert.equal(sha(readFileSync(join(fixtureRoot, fixture.path))), fixture.sha256, "fixture bytes changed during capture");
  assert.equal(sha(readFileSync(settings["--edge-native"])), nativeSha256, "native reference changed during capture");
  assert.equal(sha(readFileSync(settings["--allocation-binary"])), allocationBinarySha256, "allocation witness binary changed during capture");
  assert.equal(sha(readFileSync(settings["--allocation-report"])), allocationReportSha256, "allocation witness report changed during capture");
  assert.equal(sha(readFileSync(settings["--host-cases"])), hostCasesSha256, "host case module changed during capture");
  assert.equal(sha(readFileSync(workerd)), workerdBinarySha256, "runtime binary changed during capture");
  assert.equal(sha(readFileSync(settings["--package-record"])), sha(packageRecordBytes), "package check record changed during capture");
  stage = "verdict";
  const verdict = ledger.finish(); assert.equal(verdict.state, "passed", "registered thumbnail memory gate failed");
  finished = true;
  console.log(`thumbnail preflight: ${verdict.gatesPassed} gates, ${verdict.readings} readings, ${comparisons} parity checks passed; Workers envelope unvalidated`);
} catch (error) {
  failure = failureRecord(stage, item, error);
  writeFileSync(join(output, "capture-error.txt"), String(error.stack ?? error));
  console.error(`thumbnail preflight: failed stage=${failure.stage} item=${failure.item} reason=${failure.reason} actual=${failure.actual ?? "not_applicable"} native=${failure.native ?? "not_applicable"} type=${failure.type ?? "not_applicable"} cause=${failure.cause ?? "not_applicable"}; inspect private verdict and captures`);
  process.exitCode = 1;
} finally {
  const verdict = ledger.finish();
  writeFileSync(join(output, "verdict.json"), JSON.stringify({ ...verdict, ...(!finished && { state: "failed" }), ...(failure && { failure }), preflightOnly: true, hostedEnvelope: "unvalidated", artifactSha256, tarballSha256, packageRecordSha256: sha(packageRecordBytes), manifestSha256: sha(manifestBytes), rosterSha256, nativeSha256, allocationBinarySha256, allocationReportSha256, hostCasesSha256, runtime: { node: process.version, v8: process.versions.v8, esbuild: esbuildVersion, workerd: workerdVersion, workerdBinarySha256 } }) + "\n");
  writeFileSync(join(output, "runtime.log"), runtimeLogs.join(""));
  for (const worker of [...workers]) await stop(worker, false);
  if (server) await new Promise(resolve => { server.close(resolve); server.closeAllConnections(); });
  stopBundler(); rmSync(consumer, { recursive: true, force: true });
}

}
await main().catch(error => {
  if (captureOutput) {
    writeFileSync(join(captureOutput, "verdict.json"), JSON.stringify({ state: "failed", preflightOnly: true, reason: "initialization" }) + "\n");
    writeFileSync(join(captureOutput, "initialization-error.txt"), String(error.stack ?? error));
  }
  console.error("thumbnail preflight: failed before capture; inspect private initialization record");
  process.exitCode = 1;
});
