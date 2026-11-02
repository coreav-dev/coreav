import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { once } from "node:events";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, join, relative, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { gzipSync } from "node:zlib";
import { setTimeout as delay } from "node:timers/promises";
import { build, stop as stopBundler } from "esbuild";
import { fixtures } from "./synth.mjs";
import { indexIdentity } from "../hls/src/indexes.js";
import { archiveEntries, validateArchive, validateManifest, consumerEnv } from "../../scripts/check-package.mjs";
import { allowedLicenses } from "../../scripts/legal.mjs";
import { registeredBars } from "./registry.mjs";
import { GateLedger, residentMemory } from "./gates.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const platform = process.platform === "win32" ? "windows" : process.platform;
const architecture = process.arch === "x64" ? "64" : process.arch;
const workerd = createRequire(import.meta.url).resolve(`@cloudflare/workerd-${platform}-${architecture}/bin/workerd${process.platform === "win32" ? ".exe" : ""}`);
const output = join(root, ".harness");
const bars = registeredBars(readFileSync(resolve(root, "../../../docs/design/wasm-memory-bars.md"), "utf8"));
const args = process.argv.slice(2);
let native, tarball;
while (args.length) {
  const arg = args.shift();
  if (arg === "--edge-native" && args.length) native = resolve(args.shift());
  else if (arg === "--fixtures") throw new Error("memory gates use generated fixtures only");
  else if (arg === "--tarball" && args.length) tarball = resolve(args.shift());
  else throw new Error("usage: check --edge-native PATH --tarball PATH");
}
assert.ok(native && existsSync(native), "build edge-native and pass --edge-native PATH");
assert.ok(tarball && existsSync(tarball), "pack the WASM package and pass --tarball PATH");
const archive = readFileSync(tarball);
const packed = validateArchive(archiveEntries(archive));
validateManifest(JSON.parse(packed.get("package/package.json")), allowedLicenses(resolve(root, "../../..")));
const consumer = mkdtempSync(join(tmpdir(), "coreav-wasm-examples-"));
tarball = join(consumer, "checked.tgz");
writeFileSync(tarball, archive);
const packageRoot = join(consumer, "node_modules/@coreav/wasm");
function install(directory) {
  const manifest = existsSync(join(directory, "package.json")) ? JSON.parse(readFileSync(join(directory, "package.json"))) : { name: "installed-example", private: true, type: "module" };
  writeFileSync(join(directory, "package.json"), JSON.stringify({ ...manifest, dependencies: { ...manifest.dependencies, "@coreav/wasm": `file:${tarball}` } }));
  const result = spawnSync("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--package-lock=false"], { cwd: directory, env: consumerEnv(), encoding: "utf8", timeout: 30000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(realpathSync(join(directory, "node_modules/@coreav/wasm")), join(realpathSync(directory), "node_modules/@coreav/wasm"), "the package must be installed, not linked to the repository");
}
try {
  install(consumer);
  for (const name of ["media", "hls"]) cpSync(join(root, name, "src"), join(consumer, name, "src"), { recursive: true });
  mkdirSync(join(consumer, "test"));
  for (const name of ["harness.js", "fake-r2.mjs", "batch.mjs"]) cpSync(join(root, "test", name), join(consumer, "test", name));
} catch (error) {
  rmSync(consumer, { recursive: true, force: true });
  throw error;
}
rmSync(output, { recursive: true, force: true });
mkdirSync(join(output, "fixtures"), { recursive: true });
const files = new Map([...fixtures()].map(([name, bytes]) => [name, { bytes, etag: "fixture-v1" }]));
for (const [name, record] of files) writeFileSync(join(output, "fixtures", name), record.bytes);
const policy = JSON.parse(readFileSync(join(root, "media/src/policy.json")));
const records = new Map();
const compressedRequests = [];
let puts = 0, comparisons = 0, resources = 0;
const logs = [];
const workers = new Set();
const memory = [];
let completed = false;
const secret = /SECRETSIG|SECRETTOK|SECRETPW|SECRETKEY/;
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const ledger = new GateLedger(bars, sha(packed.get("package/wasm/coreav_bg.wasm")));
writeFileSync(join(output, "verdict.json"), JSON.stringify(ledger.verdict()) + "\n");

const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, "http://fixtures.test");
    if (url.pathname.startsWith("/__indexes/")) {
      const key = decodeURIComponent(url.pathname.slice("/__indexes/".length));
      if (request.method === "PUT") {
        const chunks = [];
        for await (const chunk of request) chunks.push(chunk);
        records.set(key, { bytes: Buffer.concat(chunks), metadata: JSON.parse(request.headers["x-metadata"]) });
        puts += 1;
        response.writeHead(204).end(); return;
      }
      const record = records.get(key);
      if (!record) { response.writeHead(404).end(); return; }
      response.writeHead(200, { "x-size": record.bytes.byteLength, "x-metadata": JSON.stringify(record.metadata) }).end(record.bytes); return;
    }
    if (url.pathname === "/redirect") { response.writeHead(302, { location: "https://fixtures.test/fixtures/av.mp4" }).end(); return; }
    if (url.pathname === "/failure") { response.writeHead(503).end(); return; }
    if (url.pathname === "/compressed") {
      compressedRequests.push(request.headers["accept-encoding"]);
      const encoded = gzipSync(files.get("av.mp4").bytes);
      response.writeHead(200, { "content-encoding": "gzip", "content-length": encoded.byteLength }).end(encoded);
      return;
    }
    const key = url.pathname.startsWith("/fixtures/") ? decodeURIComponent(url.pathname.slice(10)) : "";
    const record = files.get(key) ?? (key === "tenant/SECRETKEY.mp4" ? files.get("av.mp4") : null);
    if (!record) { response.writeHead(404).end(); return; }
    const { bytes, etag } = record;
    const tag = `"${etag}"`;
    if (request.headers["if-match"] && request.headers["if-match"] !== tag) { response.writeHead(412).end(); return; }
    const range = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? "");
    const start = range ? Number(range[1]) : 0;
    const end = range ? Math.min(Number(range[2]) + 1, bytes.length) : bytes.length;
    if (start >= bytes.length) { response.writeHead(416, { etag: tag }).end(); return; }
    const headers = { etag: tag, "content-length": end - start, "content-type": "application/octet-stream" };
    if (range) headers["content-range"] = `bytes ${start}-${end - 1}/${bytes.length}`;
    response.writeHead(range ? 206 : 200, headers).end(bytes.subarray(start, end));
  } catch { response.writeHead(500).end(); }
});
server.listen(0, "127.0.0.1");
await once(server, "listening");
const fixtureAddress = `127.0.0.1:${server.address().port}`;

async function freePort() {
  const socket = createServer();
  socket.listen(0, "127.0.0.1"); await once(socket, "listening");
  const port = socket.address().port;
  await new Promise((resolve) => socket.close(resolve));
  return port;
}

let workerNumber = 0;
function checkInputs(result, directory) {
  for (const path of Object.keys(result.metafile.inputs)) {
    const suffix = relative(directory, resolve(directory, path));
    assert.ok(!isAbsolute(suffix) && suffix !== ".." && !suffix.startsWith("../"), "bundle input escaped its installed consumer");
  }
}

async function start(example, pkg = packageRoot, settings = {}, defaultReaders = false) {
  const directory = join(output, `worker-${workerNumber++}-${example}`);
  mkdirSync(directory, { recursive: true });
  let wasm;
  const result = await build({
    absWorkingDir: consumer, metafile: true,
    entryPoints: [join(consumer, "test/harness.js")], outfile: join(directory, "bundle.js"), bundle: true, format: "esm", platform: "neutral",
    alias: { EXAMPLE: join(consumer, example, "src/index.js"), ...(pkg === packageRoot ? {} : { "@coreav/wasm/workers": join(pkg, "src/workers.js") }) },
    plugins: [{ name: "wasm-module", setup(builder) {
      builder.onResolve({ filter: /\.wasm$/ }, (args) => {
        wasm = resolve(args.resolveDir, args.path);
        cpSync(wasm, join(directory, "coreav_bg.wasm"));
        return { path: "./coreav_bg.wasm", external: true };
      });
      if (defaultReaders) builder.onLoad({ filter: /\/(?:input|indexes)\.js$/ }, ({ path }) => {
        const contents = readFileSync(path, "utf8");
        assert.equal(contents.split("memoryBytes: READER_MEMORY_BYTES").length, 2, "the test-only reader variant must change exactly one bound");
        return { contents: contents.replace("memoryBytes: READER_MEMORY_BYTES", "memoryBytes: 0"), loader: "js" };
      });
    } }]
  });
  checkInputs(result, consumer);
  assert.ok(wasm, "the bundle must contain a compiled WASM module");
  const port = await freePort();
  const variables = { DEV_ANONYMOUS: "1", ALLOWED_HOSTS: "fixtures.test", MAX_UPLOAD_BYTES: "16777216", SEGMENT_DURATION: "1", ...settings };
  const bindings = Object.entries(variables).filter(([, text]) => text !== null).map(([name, text]) => `(name = ${JSON.stringify(name)}, text = ${JSON.stringify(text)})`);
  bindings.push('(name = "FIXTURES", service = "fixtures")');
  writeFileSync(join(directory, "config.capnp"), `using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [
    (name = "example", worker = (compatibilityDate = "2026-11-01", modules = [(name = "bundle.js", esModule = embed "bundle.js"), (name = "coreav_bg.wasm", wasm = embed "coreav_bg.wasm")], bindings = [${bindings.join(",")}], globalOutbound = "fixtures")),
    (name = "fixtures", external = (address = "${fixtureAddress}", http = ()))
  ], sockets = [(name = "http", address = "127.0.0.1:${port}", http = (), service = "example")]
);
`);
  const inspectorPort = await freePort();
  const child = spawn(workerd, ["serve", "--inspector-addr", `127.0.0.1:${inspectorPort}`, join(directory, "config.capnp")], { stdio: ["ignore", "pipe", "pipe"] });
  let launchError;
  child.on("error", (error) => { launchError = error; });
  child.stdout.on("data", (chunk) => logs.push(chunk.toString()));
  child.stderr.on("data", (chunk) => logs.push(chunk.toString()));
  const worker = { child, base: `http://127.0.0.1:${port}`, inspector: `http://127.0.0.1:${inspectorPort}`, build: sha(readFileSync(wasm)), defaultReaders };
  workers.add(worker);
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (launchError) throw launchError;
    if (child.exitCode !== null || child.signalCode !== null) throw new Error(`workerd exited: ${logs.join("")}`);
    let response;
    try {
      response = await fetch(worker.base + "/__harness/memory", { signal: AbortSignal.timeout(500) });
    } catch {}
    if (response?.ok) {
      const info = await response.json();
      assert.equal(info.build, worker.build, "moduleInfo fingerprint must describe the embedded WASM");
      ledger.start(worker, info);
      worker.isolate = info.isolate;
      worker.version = info.version;
      await recordMemory(worker, `${example}-boot`);
      return worker;
    }
    await delay(50);
  }
  throw new Error(`workerd did not start: ${logs.join("")}`);
}

async function stop(worker) {
  if (worker.child.pid && worker.child.exitCode === null && worker.child.signalCode === null) {
    const done = once(worker.child, "exit");
    const timer = setTimeout(() => worker.child.kill("SIGKILL"), 1000).unref();
    worker.child.kill("SIGTERM");
    try { await done; } finally { clearTimeout(timer); }
  }
  workers.delete(worker);
}

function reference(operation, name, options, index) {
  const path = join(output, "fixtures", name);
  const out = join(output, "native-output");
  const args = index ? ["run-indexed", operation, index, path, JSON.stringify(options), out] : ["run", operation, path, JSON.stringify(options), out];
  const result = spawnSync(native, args, { encoding: "utf8", timeout: 10000, maxBuffer: 8 * 1024 * 1024 });
  assert.ifError(result.error);
  assert.ok(result.status === 0 || result.status === 1, result.stderr);
  const text = result.stdout.replace(/\r?\n$/, "");
  return { status: result.status, text, document: JSON.parse(text), bytes: new Uint8Array(readFileSync(out)) };
}

async function get(worker, path, init) {
  const response = await fetch(worker.base + path, { ...init, signal: AbortSignal.timeout(15000) });
  const bytes = new Uint8Array(await response.arrayBuffer());
  assert.doesNotMatch(new TextDecoder().decode(bytes), secret);
  assert.doesNotMatch(JSON.stringify([...response.headers]), secret);
  return { status: response.status, bytes, text: new TextDecoder().decode(bytes), headers: response.headers };
}

const routes = [
  ["probe", "probe", {}, ""], ["ffprobe", "ffprobe", {}, ""], ["check", "check", { policy }, ""], ["chapters", "chapters", {}, ""],
  ["extract-audio", "extractAudio", {}, ""], ["extract-subtitles", "extractSubtitles", {}, ""],
  ["clip", "clip", { start: "0", end: "1" }, "start=0&end=1"], ["remux", "remux", {}, ""], ["sanitize", "sanitize", {}, ""]
];
const streamed = new Set(["extractAudio", "extractSubtitles", "clip", "remux", "sanitize", "hlsGet"]);

async function compareMedia(worker, name, [route, operation, options, query], kind) {
  const expected = reference(operation, name, options);
  const params = new URLSearchParams(query);
  const input = files.get(name).bytes;
  let init;
  if (kind === "key") params.set("key", name);
  else if (kind === "url") params.set("url", `https://fixtures.test/fixtures/${encodeURIComponent(name)}?X-Amz-Signature=SECRETSIG&token=SECRETTOK`);
  else init = { method: "POST", body: input };
  const actual = await get(worker, `/${route}?${params}`, init);
  const status = expected.status === 1 ? (expected.document.usage ? 400 : 422) : operation === "check" && expected.document.verdict === "reject" ? 422 : 200;
  assert.equal(actual.status, status, `${route} ${kind} ${name}: ${actual.text}`);
  if (expected.status === 0 && streamed.has(operation)) assert.deepEqual(actual.bytes, expected.bytes, `${route} bytes`);
  else if (operation === "probe" || operation === "ffprobe" || expected.status === 1) assert.equal(actual.text, expected.text, `${route} exact JSON`);
  else assert.deepEqual(JSON.parse(actual.text), expected.document, `${route} document`);
  if (operation === "ffprobe" && expected.status === 0) assert.equal(JSON.parse(actual.text).format.filename, "-");
  comparisons += 1;
  return actual;
}

async function crawl(worker, name) {
  const indexed = reference("hlsIndex", name, { segmentDuration: "1", tag: files.get(name).etag });
  assert.equal(indexed.status, 0, indexed.text);
  const path = join(output, "native-index");
  writeFileSync(path, indexed.bytes);
  const pending = ["master.m3u8"], seen = new Set(pending);
  while (pending.length) {
    const resource = pending.shift();
    const expected = reference("hlsGet", name, { name: resource }, path);
    assert.equal(expected.status, 0, expected.text);
    const actual = await get(worker, `/${encodeURIComponent(name)}/${resource}`);
    assert.equal(actual.status, 200, actual.text);
    assert.deepEqual(actual.bytes, expected.bytes, `${name} ${resource}`);
    assert.equal(actual.headers.get("access-control-allow-origin"), "*");
    resources += 1;
    if (resource.endsWith(".m3u8")) {
      const links = [...actual.text.matchAll(/URI="([^"]+)"/g)].map((match) => match[1]);
      links.push(...actual.text.split(/\r?\n/).filter((line) => line && !line.startsWith("#")));
      for (const link of links) if (!seen.has(link)) { seen.add(link); pending.push(link); }
    }
  }
  assert.ok([...seen].some((name) => name.endsWith(".m4s")), "crawl must reach media segments");
  await reading(worker);
}

async function heapUsage(worker) {
  if (worker.heapUnavailable) return { heap: null, heapError: "inspector_unavailable" };
  let socket;
  try {
    const targets = await fetch(worker.inspector + "/json/list", { signal: AbortSignal.timeout(1000) }).then((response) => response.json());
    const target = targets.find((target) => target.webSocketDebuggerUrl);
    assert.ok(target, "inspector has no worker target");
    socket = new WebSocket(target.webSocketDebuggerUrl);
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("inspector timeout")), 1000);
      const finish = (error, value) => { clearTimeout(timer); error ? reject(error) : resolve(value); };
      socket.addEventListener("error", () => finish(new Error("inspector connection failed")));
      socket.addEventListener("close", () => finish(new Error("inspector closed")));
      socket.addEventListener("open", () => socket.send(JSON.stringify({ id: 1, method: "HeapProfiler.collectGarbage" })));
      socket.addEventListener("message", (event) => {
        try {
          const message = JSON.parse(event.data);
          if (message.error) throw new Error("inspector method unavailable");
          if (message.id === 1) socket.send(JSON.stringify({ id: 2, method: "Runtime.getHeapUsage" }));
          if (message.id === 2) {
            const { usedSize, totalSize } = message.result;
            assert.ok(Number.isFinite(usedSize) && usedSize >= 0 && Number.isFinite(totalSize) && totalSize >= usedSize);
            finish(null, { heap: { usedBytes: usedSize, totalBytes: totalSize }, heapError: null });
          }
        } catch (error) { finish(error); }
      });
    });
  } catch {
    worker.heapUnavailable = true;
    return { heap: null, heapError: "inspector_unavailable" };
  } finally { socket?.close(); }
}

async function reading(worker) {
  assert.ok(worker.child.exitCode === null && worker.child.signalCode === null, "memory runtime exited before its reading");
  const response = await fetch(worker.base + "/__harness/memory", { signal: AbortSignal.timeout(15000) });
  assert.ok(response.ok, "memory reading is unavailable");
  const info = await response.json();
  assert.ok(worker.child.exitCode === null && worker.child.signalCode === null, "memory runtime exited during its reading");
  assert.equal(info.isolate, worker.isolate, "memory isolate identity changed");
  assert.equal(info.build, worker.build, "memory build identity changed");
  assert.equal(info.inFlight, 0, "memory reading preceded response completion");
  assert.equal(info.pending, 0, "memory reading preceded deferred work completion");
  return info;
}

async function recordMemory(worker, step, expect) {
  if (expect === undefined) ledger.diagnostic(step);
  const info = await reading(worker);
  if (expect !== undefined) ledger.gate(worker, step, info, expect);
  const { memoryBytes, ...details } = info;
  assert.ok(Number.isSafeInteger(memoryBytes) && memoryBytes > 0, "moduleInfo must report linear memory");
  const rss = residentMemory(process.platform === "linux" ? () => readFileSync(`/proc/${worker.child.pid}/status`, "utf8") : null);
  memory.push({ step, classification: expect === undefined ? "diagnostic" : "gate", wasm: memoryBytes, ...await heapUsage(worker), ...rss, readerMemoryBytes: worker.defaultReaders ? 0 : 40 * 1024 * 1024, ...details });
}

async function arm(worker, size, total) {
  const response = await fetch(worker.base + "/__harness/batch", { method: "POST", body: JSON.stringify({ size, total }), signal: AbortSignal.timeout(15000) });
  assert.equal(response.status, 204, "memory batch did not arm");
  await response.arrayBuffer();
}

async function standalone(example) {
  const directory = join(consumer, `standalone-${example}`);
  cpSync(join(root, example), directory, { recursive: true, filter: (path) => !["node_modules", "dist", ".wrangler"].some((name) => path.split(/[\\/]/).includes(name)) });
  install(directory);
  let modules = 0;
  const result = await build({
    absWorkingDir: directory, entryPoints: ["src/index.js"], outfile: join(directory, "bundle.js"), bundle: true, format: "esm", platform: "neutral", metafile: true,
    plugins: [{ name: "installed-wasm", setup(builder) {
      builder.onResolve({ filter: /\.wasm$/ }, (args) => {
        modules += 1;
        assert.equal(sha(readFileSync(resolve(args.resolveDir, args.path))), sha(readFileSync(join(packageRoot, "wasm/coreav_bg.wasm"))));
        return { path: "./coreav_bg.wasm", external: true };
      });
    } }]
  });
  assert.equal(modules, 1, "the standalone bundle must import one compiled module");
  checkInputs(result, directory);
}

async function memoryMatrix() {
  for (const route of routes) {
    const worker = await start("media");
    await compareMedia(worker, "av-text.mp4", route, "key");
    await recordMemory(worker, `fresh-${route[0]}`, { served: 1 });
    await stop(worker);
  }
  const warm = await start("media");
  for (let pass = 1; pass <= 3; pass += 1) {
    for (const route of [routes[7], routes[8], ...routes.slice(0, 7)]) {
      await compareMedia(warm, "av-text.mp4", route, "key");
      await recordMemory(warm, `warm-${pass}-${route[0]}`);
    }
    const upload = new Uint8Array(16 * 1024 * 1024); upload.set(files.get("av.mp4").bytes);
    assert.equal((await get(warm, "/probe", { method: "POST", body: upload })).status, 200);
    await recordMemory(warm, `warm-${pass}-upload`);
  }
  await stop(warm);
  for (const [name, list] of [["read-only", routes.slice(0, 7)], ["with-writers", routes]]) {
    for (const concurrency of [1, 4, 6]) {
      const worker = await start("media");
      const requests = [...list, ...list];
      if (name === "with-writers") await arm(worker, concurrency, requests.length);
      let at = 0;
      await Promise.all(Array.from({ length: concurrency }, async () => {
        while (at < requests.length) {
          const route = requests[at++];
          await compareMedia(worker, "av-text.mp4", route, "key");
        }
      }));
      await recordMemory(worker, `concurrent-${name}-${concurrency}`, name === "with-writers" ? { served: requests.length, concurrency, total: requests.length } : undefined);
      await stop(worker);
    }
  }
  for (const route of routes) {
    const worker = await start("media", packageRoot, {}, true);
    await compareMedia(worker, "av-text.mp4", route, "key");
    await recordMemory(worker, `library-default-reader-${route[0]}`);
    await stop(worker);
  }
}

try {
  await standalone("media");
  await standalone("hls");
  const media = await start("media");
  for (const name of files.keys()) for (const route of routes) for (const kind of ["key", "url", "post"]) await compareMedia(media, name, route, kind);
  for (const route of [
    ["probe", "probe", { scanFragments: true }, "scan"], ["extract-audio", "extractAudio", { stream: 1 }, "stream=1"],
    ["clip", "clip", { start: "0.9", audio: "none" }, "start=0.9&audio=none"], ["clip", "clip", { start: "0", video: 0 }, "start=0&video=0"],
    ["remux", "remux", { streams: [0, 1], dropUnsupported: true }, "streams=0,1&dropUnsupported"],
    ["sanitize", "sanitize", { keep: ["none"] }, "keep=none"], ["sanitize", "sanitize", { keep: ["rotation", "language"] }, "keep=rotation,language"],
    ["extract-subtitles", "extractSubtitles", { stream: 2 }, "stream=2"]
  ]) await compareMedia(media, "av-text.mp4", route, "key");
  const large = await get(media, "/probe", { method: "POST", body: files.get("large-duration.mp4").bytes });
  assert.match(large.text, /9007199254740993/);
  assert.match(large.headers.get("content-type"), /application\/json/);
  for (const [path, status] of [
    ["/probe?key=missing-SECRETKEY", 404], ["/probe?key=__throw__SECRETKEY", 500],
    ["/probe?url=" + encodeURIComponent("https://user:SECRETPW@fixtures.test/fixtures/av.mp4"), 400],
    ...["redirect", "failure", "missing", "compressed"].map((path) => ["/probe?url=" + encodeURIComponent(`https://fixtures.test/${path}?token=SECRETTOK`), path === "missing" ? 404 : 502])
  ]) assert.equal((await get(media, path)).status, status);
  assert.equal(compressedRequests.length, 1);
  assert.ok(compressedRequests[0].split(",").every((value) => value.trim() === "identity"));
  assert.equal((await get(media, "/ffprobe?key=tenant%2FSECRETKEY.mp4")).status, 200);
  await recordMemory(media, "media-start");
  for (const route of [routes[7], routes[0], routes[6]]) { await compareMedia(media, "av.mp4", route, "key"); await recordMemory(media, `media-${route[0]}`); }
  const cap = new Uint8Array(16 * 1024 * 1024); cap.set(files.get("av.mp4").bytes);
  assert.equal((await get(media, "/probe", { method: "POST", body: cap })).status, 200);
  await recordMemory(media, "media-upload-cap");
  const locked = await start("media", packageRoot, { DEV_ANONYMOUS: null });
  assert.equal((await get(locked, "/probe?key=av.mp4")).status, 401);
  await stop(locked);
  const override = JSON.stringify({ ...policy, streams: { ...policy.streams, video: { ...policy.streams.video, min_count: 0 } } });
  const configured = await start("media", packageRoot, { POLICY_JSON: override });
  const verdict = await get(configured, "/check?key=audio.m4a");
  assert.equal(verdict.status, 200);
  assert.deepEqual(JSON.parse(verdict.text), reference("check", "audio.m4a", { policy: override }).document);
  await stop(configured);
  assert.equal((await get(media, "/probe", { method: "POST", body: new Uint8Array(8), headers: { "content-encoding": "gzip" } })).status, 415);
  await stop(media);

  await memoryMatrix();

  const video = reference("remux", "av.mp4", { streams: [0] });
  assert.equal(video.status, 0);
  files.set("video-only.mp4", { bytes: video.bytes, etag: "fixture-v1" });
  writeFileSync(join(output, "fixtures", "video-only.mp4"), video.bytes);
  let hls = await start("hls");
  await recordMemory(hls, "hls-start");
  await crawl(hls, "av.mp4");
  await crawl(hls, "audio.m4a");
  await crawl(hls, "video-only.mp4");
  await recordMemory(hls, "hls-built", { served: resources });
  const errors = [
    ["/av.mp4/video-99999.m4s", 404], ["/av.mp4/audio-99999.m4s", 404], ["/audio.m4a/video-init.mp4", 404],
    ["/video-only.mp4/audio-init.mp4", 404],
    ["/av.mp4/index.html", 404], ["/%FF/master.m3u8", 400], ["/av.mp4/master.m3u8", 405, { method: "POST" }],
    ["/not-a-movie.bin/master.m3u8", 422], ["/missing-SECRETKEY/master.m3u8", 404], ["/__throw__SECRETKEY/master.m3u8", 500]
  ];
  for (const [path, status, init] of errors) {
    const actual = await get(hls, path, init);
    assert.equal(actual.status, status, actual.text);
    assert.equal(actual.headers.get("access-control-allow-origin"), "*");
    if (status === 405) assert.equal(actual.headers.get("allow"), "GET");
  }
  await arm(hls, 4, 4);
  const parallel = await Promise.all(Array.from({ length: 4 }, () => get(hls, "/av.mp4/video-0.m4s")));
  assert.ok(parallel.every((item) => item.status === 200));
  for (const item of parallel) assert.deepEqual(item.bytes, parallel[0].bytes);
  await recordMemory(hls, "hls-concurrent", { served: resources + errors.length + parallel.length, concurrency: 4, total: 4, sameAs: "hls-built" });
  const aBuild = hls.build, version = hls.version;
  const storedCount = records.size, writes = puts;
  await stop(hls); hls = await start("hls");
  const beforeReload = resources;
  await crawl(hls, "av.mp4"); assert.equal(puts, writes, "restart should use the stored index");
  await recordMemory(hls, "hls-reloaded", { served: resources - beforeReload });
  const damaged = await indexIdentity({ key: "av.mp4", etag: "fixture-v1", build: aBuild, options: { segmentDuration: "1" } });
  assert.ok(records.has(damaged.storeKey));
  records.get(damaged.storeKey).bytes = new Uint8Array(16).fill(7);
  await stop(hls); hls = await start("hls");
  const beforeRebuild = resources;
  await crawl(hls, "av.mp4"); assert.equal(puts, writes + 1, "a damaged stored index rebuilds once");
  await recordMemory(hls, "hls-rebuilt", { served: resources - beforeRebuild });
  const changed = files.get("av.mp4"); changed.etag = "fixture-v2";
  await crawl(hls, "av.mp4"); assert.equal(records.size, storedCount + 1, "new ETag uses a new identity");
  changed.etag = "fixture-v1";
  await stop(hls);
  const copy = join(consumer, "package-b"); mkdirSync(copy);
  for (const name of ["src", "wasm", "package.json"]) cpSync(join(packageRoot, name), join(copy, name), { recursive: true });
  const original = readFileSync(join(copy, "wasm/coreav_bg.wasm"));
  writeFileSync(join(copy, "wasm/coreav_bg.wasm"), Buffer.concat([original, Buffer.from([0, 2, 1, 120])]));
  const fingerprint = spawnSync(process.execPath, [join(root, "../scripts/fingerprint.mjs"), copy], { encoding: "utf8" });
  assert.equal(fingerprint.status, 0, fingerprint.stderr);
  hls = await start("hls", copy);
  assert.equal(hls.version, version); assert.notEqual(hls.build, aBuild);
  const prior = records.size;
  await crawl(hls, "av.mp4"); assert.equal(records.size, prior + 1, "different WASM bytes need a separate stored index");
  await stop(hls); hls = await start("hls");
  const before = puts; await crawl(hls, "av.mp4"); assert.equal(puts, before, "returning to build A reuses A's record");
  await stop(hls);
  hls = await start("hls", packageRoot, { NO_INDEXES: "1" });
  const previous = puts; await crawl(hls, "av.mp4"); assert.equal(puts, previous, "memory-only mode does not write the store");
  await stop(hls);
  hls = await start("hls", packageRoot, { DEV_ANONYMOUS: null });
  const unauthorized = await get(hls, "/av.mp4/master.m3u8");
  assert.equal(unauthorized.status, 401); assert.equal(unauthorized.headers.get("access-control-allow-origin"), "*");
  await stop(hls);
  assert.ok(comparisons > 0 && resources > 0 && files.size > 0, "empty verification is not a pass");
  assert.doesNotMatch(logs.join(""), secret);
  const memoryVerdict = ledger.finish();
  assert.equal(memoryVerdict.state, "passed", `memory gates failed: ${memoryVerdict.failures.map((gate) => `${gate.step} (bar ${gate.bar} bytes)`).join(", ")}`);
  console.log(JSON.stringify({ fixtures: files.size, comparisons, resources, storedIndexes: records.size }));
  console.log(`memory: ${memoryVerdict.gatesPassed} gates passed (synthetic fixtures; not a platform isolate measurement)`);
  if (!process.env.CI) console.log("Local diagnostic records: .harness/memory.json");
  completed = true;
} finally {
  const verdict = { ...ledger.finish(), ...(!completed && { state: "failed" }) };
  writeFileSync(join(output, "verdict.json"), JSON.stringify(verdict) + "\n");
  writeFileSync(join(output, "memory.json"), JSON.stringify({ gates: verdict, records: memory, limitations: ["Small generated fixtures, not a held-out or platform isolate measurement", "The fake bucket retains whole fixture bodies in JavaScript", "Heap and process RSS are diagnostic; RSS is unavailable on macOS", "Zero readerMemoryBytes means library defaults, not a zero-byte reader", "Warm, read-only concurrency and library-default-reader observations have no acceptance bar"] }, null, 2) + "\n");
  writeFileSync(join(output, "runtime.log"), logs.join(""));
  for (const worker of [...workers]) await stop(worker);
  await new Promise((resolve) => { server.close(resolve); server.closeAllConnections(); });
  stopBundler();
  rmSync(consumer, { recursive: true, force: true });
}
