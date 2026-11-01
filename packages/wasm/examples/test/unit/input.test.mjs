import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { log, objectKey, OUTCOMES, parseRequest, readConfig, readUpload, readerOptions, remoteUrl, ROUTES } from "../../media/src/input.js";
import { parsePath, readerOptions as hlsReaderOptions } from "../../hls/src/indexes.js";

const env = { DEV_ANONYMOUS: "1", ALLOWED_HOSTS: "fixtures.test" };
const refused = (status) => (error) => { assert.equal(error.status, status); assert.doesNotMatch(error.message, /SECRETSIG|SECRETTOK|SECRETPW|SECRETKEY/); return true; };

function upload(length, headers = {}) {
  const state = { pulled: 0, cancelled: false };
  let offset = 0;
  const body = new ReadableStream({
    pull(controller) {
      if (offset === length) { controller.close(); return; }
      const size = Math.min(3, length - offset);
      state.pulled += size;
      offset += size;
      controller.enqueue(new Uint8Array(size).fill(7));
    },
    cancel() { state.cancelled = true; }
  }, { highWaterMark: 0 });
  return { state, request: new Request("https://worker.test/probe", { method: "POST", body, duplex: "half", headers }) };
}

for (const declared of [false, true]) {
  test(`uploads enforce received bytes with length header ${declared}`, async () => {
    for (const length of [9, 10, 11]) {
      const { request, state } = upload(length, declared ? { "content-length": String(length) } : {});
      if (length <= 10) assert.equal((await readUpload(request, 10)).byteLength, length);
      else { await assert.rejects(readUpload(request, 10), refused(413)); assert.ok(state.pulled <= 13); assert.equal(state.cancelled, true); }
    }
  });
}

test("length mismatches and encodings are refused without unbounded reads", async () => {
  for (const [actual, declared] of [[20, 10], [10, 20]]) {
    const { request, state } = upload(actual, { "content-length": String(declared) });
    await assert.rejects(readUpload(request, 30), refused(400));
    if (actual > declared) assert.equal(state.cancelled, true);
    else assert.equal(state.pulled, actual);
    assert.equal(request.body.locked, false);
  }
  for (const [headers, status] of [[{ "content-length": "11" }, 413], [{ "content-encoding": "gzip" }, 415], [{ "content-length": "1e2" }, 400]]) {
    const { request, state } = upload(20, headers);
    await assert.rejects(readUpload(request, 10), refused(status));
    assert.equal(state.pulled, 0);
    if (status === 413) assert.equal(state.cancelled, true);
  }
  for (const headers of [{}, { "content-length": "0" }]) await assert.rejects(readUpload(upload(0, headers).request, 10), refused(400));
});

test("configuration fails closed and preserves policy JSON text", () => {
  assert.equal(readConfig({}).uploadBytes, 8388608);
  for (const value of ["abc", "0", "-1", "1e6", "16777217"]) assert.throws(() => readConfig({ MAX_UPLOAD_BYTES: value }), refused(500));
  for (const value of ["a/b", "u@h", "h?x", "h\\x"]) assert.throws(() => readConfig({ ALLOWED_HOSTS: value }), refused(500));
  for (const value of ["0", "1001"]) assert.throws(() => readConfig({ MAX_SOURCE_REQUESTS: value }), refused(500));
  assert.throws(() => readConfig({ DEV_ANONYMOUS: "true" }), refused(500));
  assert.throws(() => readConfig({ POLICY_JSON: "{" }), refused(500));
  const text = '{"integer":9007199254740993}';
  assert.equal(readConfig({ POLICY_JSON: text }).policyText, text);
});

test("URL policy requires exact normalized HTTPS hosts without credentials", () => {
  const allowed = readConfig({ ALLOWED_HOSTS: "EXAMPLE.COM,bücher.example" }).allowedHosts;
  assert.equal(remoteUrl("https://EXAMPLE.COM/x#fragment", allowed).href, "https://example.com/x");
  assert.equal(remoteUrl("https://bücher.example/x", allowed).hostname, "xn--bcher-kva.example");
  for (const url of ["http://example.com", "https://user:SECRETPW@example.com", "https://other.test?token=SECRETTOK", "https://example.com:8443", "https://example.com/" + "x".repeat(2048)]) assert.throws(() => remoteUrl(url, allowed), refused(400));
  assert.throws(() => remoteUrl("https://example.com", new Set()), refused(400));
});

test("authentication and parameter checks happen before reading uploads", async () => {
  for (const route of Object.keys(ROUTES)) {
    const { request, state } = upload(10);
    const other = new Request(`https://worker.test/${route}`, request);
    await assert.rejects(parseRequest(other, {}), refused(401));
    assert.equal(state.pulled, 0);
  }
  for (const [path, settings, status] of [["/missing", { MAX_UPLOAD_BYTES: "bad" }, 404], ["/probe", { MAX_UPLOAD_BYTES: "bad" }, 500], ["/probe?format=ffprobe-compat", env, 400], ["/probe?key=SECRETKEY", env, 400]]) {
    const { request, state } = upload(10);
    await assert.rejects(parseRequest(new Request(`https://worker.test${path}`, request), settings), refused(status));
    assert.equal(state.pulled, 0);
  }
});

test("route parameters keep the stream-copy operation options", async () => {
  const cases = [
    ["probe?scan", { scanFragments: true }],
    ["extract-audio?stream=2", { stream: 2 }],
    ["clip?start=0.9&end=1&audio=none", { start: "0.9", end: "1", video: undefined, audio: "none" }],
    ["remux?streams=0,1&dropUnsupported", { streams: [0, 1], dropUnsupported: true }],
    ["sanitize?keep=none", { streams: undefined, keep: ["none"] }],
    ["sanitize?keep=rotation,language", { streams: undefined, keep: ["rotation", "language"] }]
  ];
  for (const [path, options] of cases) assert.deepEqual((await parseRequest(new Request(`https://worker.test/${path}&key=object`), env)).options, options);
  for (const path of ["probe", "probe?key=x&url=https://fixtures.test/x", "extract-audio?key=x&stream=-1", "probe?key=x&key=y"]) await assert.rejects(parseRequest(new Request(`https://worker.test/${path}`), env), refused(400));
});

test("tenant prefixes and HLS paths are validated before I/O", () => {
  assert.equal(objectKey("tenant/", "clip.mp4"), "tenant/clip.mp4");
  for (const key of ["", "bad\nkey", "é".repeat(513)]) assert.throws(() => objectKey("", key), refused(400));
  assert.throws(() => objectKey("tenant/", "x".repeat(1024)), refused(400));
  const parse = (path, settings = env) => parsePath(new Request(`https://worker.test${path}`), settings);
  assert.equal(parse("/tenant/clip.mp4/master.m3u8").key, "tenant/clip.mp4");
  assert.throws(() => parse("/x/master.m3u8", {}), refused(401));
  assert.throws(() => parse("/%FF/master.m3u8"), refused(400));
  assert.throws(() => parse("/x/index.html"), refused(404));
  assert.throws(() => parse("/x/master.m3u8", { ...env, SEGMENT_DURATION: "0" }), refused(500));
});

test("logs contain allowlisted constants only", () => {
  const records = [];
  const original = console.log;
  console.log = (line) => records.push(line);
  try {
    log("SECRETKEY", "SECRETTOK");
    for (const outcome of OUTCOMES) log("probe", outcome);
  } finally { console.log = original; }
  assert.deepEqual(JSON.parse(records[0]), { example: "media", route: "unknown", outcome: "internal" });
  assert.doesNotMatch(records.join("\n"), /SECRET/);
  assert.deepEqual(records.slice(1).map((line) => JSON.parse(line).outcome), [...OUTCOMES]);
});

test("every remote source uses readerOptions rather than library memory defaults", () => {
  for (const options of [readerOptions, hlsReaderOptions]) assert.deepEqual(options({ maxRequests: 17 }), { memoryBytes: 41943040, maxRequests: 17 });
  for (const entry of ["media", "hls"]) {
    const code = readFileSync(new URL(`../../${entry}/src/index.js`, import.meta.url), "utf8");
    assert.match(code, /const reader = readerOptions\(config\)/, "construct remote options through readerOptions");
    const calls = [...code.matchAll(/\b(?:r2Source|httpSource)\(([^()]*)\)/g)];
    assert.ok(calls.length > 0);
    for (const call of calls) assert.match(call[1], /, reader$/, "pass readerOptions to every remote source");
  }
});
