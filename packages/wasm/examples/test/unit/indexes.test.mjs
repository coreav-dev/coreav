import assert from "node:assert/strict";
import { test } from "node:test";
import { setImmediate as turn } from "node:timers/promises";
import { IndexCache, indexIdentity, INDEX_LIMITS, log, OUTCOMES } from "../../hls/src/indexes.js";
import { FakeBucket } from "../fake-r2.mjs";

const a = "a".repeat(64);
const b = "b".repeat(64);
const options = { segmentDuration: "1" };
const identity = (key = "clip.mp4", build = a) => indexIdentity({ key, etag: "v1", build, options });
const bytes = (size = 16, value = 7) => new Uint8Array(size).fill(value);
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const refused = (status) => (error) => { assert.equal(error.status, status); if (status === 503) assert.equal(error.headers["retry-after"], "1"); return true; };
const limits = { storedBytes: 32, cacheBytes: 32 };
function idle(cache) {
  const stats = cache.stats();
  assert.equal(stats.loading, 0);
  assert.equal(stats.reservedBytes, 0);
  assert.equal(stats.building, 0);
  assert.ok(stats.entries <= cache.limits.cacheEntries);
  assert.ok(stats.readyBytes <= cache.limits.cacheBytes);
}
function storedRecord(id, data, changes = {}) {
  const state = { pulled: 0, cancelled: 0 };
  const body = new ReadableStream({
    pull(controller) { state.pulled += data.byteLength; controller.enqueue(data); controller.close(); },
    cancel() { state.cancelled += 1; }
  }, { highWaterMark: 0 });
  return { object: { size: data.byteLength, customMetadata: { ...id.metadata, state: "ready" }, body, ...changes }, state };
}
function tasks() {
  const pending = [];
  return { waitUntil: (promise) => pending.push(promise), done: () => Promise.all(pending) };
}

test("index identity separates key ETag options and exact WASM bytes", async () => {
  const base = await identity();
  assert.deepEqual(await identity(), base);
  for (const input of [{ key: "other" }, { etag: "v2" }, { build: b }, { options: { segmentDuration: "2" } }]) {
    const changed = await indexIdentity({ key: "clip.mp4", etag: "v1", build: a, options, ...input });
    assert.notEqual(base.id, changed.id);
    assert.notEqual(base.storeKey, changed.storeKey);
  }
  for (const build of [undefined, "0.0.1", "a".repeat(63)]) await assert.rejects(indexIdentity({ key: "x", etag: "v1", build, options }), refused(500));
  assert.notEqual((await indexIdentity({ key: "a\nb", etag: "c", build: a, options })).id, (await indexIdentity({ key: "a", etag: "b\nc", build: a, options })).id);
  assert.equal((await identity("x".repeat(1024))).storeKey.length, 81);
  assert.deepEqual(INDEX_LIMITS, { storedBytes: 8388608, cacheBytes: 8388608, cacheEntries: 16, builds: 1 });
});

test("stored hits read bounded streams without arrayBuffer or builds", async () => {
  const id = await identity();
  const record = storedRecord(id, bytes());
  record.object.arrayBuffer = () => assert.fail("stream the bounded stored body");
  const cache = new IndexCache(limits);
  const got = await cache.get(id, { stored: { get: async () => record.object }, buildIndex: () => assert.fail("stored hit"), waitUntil: () => {} });
  assert.deepEqual(got, bytes());
  assert.equal(record.state.pulled, 16);
  assert.equal(record.state.cancelled, 0);
  idle(cache);
});

test("the R2 stand-in preserves metadata, range and conditional-read semantics", async () => {
  const bucket = new FakeBucket(new Map([["clip", { bytes: bytes(32), etag: "v1", customMetadata: { test: "yes" } }]]), { chunkSize: 3 });
  const head = await bucket.head("clip");
  assert.equal(head.size, 32); assert.equal(head.etag, "v1"); assert.equal(head.httpEtag, '"v1"');
  const denied = await bucket.get("clip", { onlyIf: { etagMatches: "v2" } });
  assert.equal(denied.body, undefined);
  const range = await bucket.get("clip", { onlyIf: { etagMatches: "v1" }, range: { offset: 8, length: 7 } });
  assert.equal(bucket.bytesPulled, 0); assert.deepEqual(range.range, { offset: 8, length: 7 });
  assert.deepEqual(new Uint8Array(await new Response(range.body).arrayBuffer()), bytes(7));
  assert.equal(bucket.bytesPulled, 7);
  const unused = await bucket.get("clip"); await unused.body.cancel(); assert.equal(bucket.cancels, 1);
  const updated = await bucket.put("clip", bytes(16, 8), { customMetadata: { test: "new" } });
  assert.notEqual(updated.etag, "v1");
  assert.deepEqual((await bucket.head("clip")).customMetadata, { test: "new" });
  await bucket.delete("clip"); assert.equal(await bucket.get("clip"), null);
});

test("stale metadata and oversized bodies are cancelled before allocation", async () => {
  const id = await identity();
  for (const changes of [{ customMetadata: { ...id.metadata, state: "ready", etag: "v2" } }, { customMetadata: { ...id.metadata, state: "ready", build: b } }, { customMetadata: { ...id.metadata, state: "ready", options: "{}" } }, { customMetadata: { ...id.metadata, state: "ready", scheme: "old" } }, { size: 33 }]) {
    const record = storedRecord(id, bytes(), changes);
    const writes = [];
    const background = tasks();
    const cache = new IndexCache(limits);
    let builds = 0;
    const result = await cache.get(id, { stored: { get: async () => record.object, put: async (...args) => writes.push(args) }, buildIndex: async () => { builds += 1; return bytes(16, 9); }, waitUntil: background.waitUntil });
    await background.done();
    assert.equal(builds, 1);
    assert.deepEqual(result, bytes(16, 9));
    assert.equal(record.state.pulled, 0);
    assert.equal(record.state.cancelled, 1);
    assert.deepEqual(writes[0][2].customMetadata, { ...id.metadata, state: "ready" });
    idle(cache);
  }
});

test("short or oversized stored streams release reservations and rebuild", async () => {
  const id = await identity();
  for (const size of [15, 17]) {
    const record = storedRecord(id, bytes(size), { size: 16 });
    const cache = new IndexCache(limits);
    let builds = 0;
    await cache.get(id, { stored: { get: async () => record.object, put: async () => {} }, buildIndex: async () => { builds += 1; return bytes(); }, waitUntil: () => {} });
    assert.equal(builds, 1);
    idle(cache);
  }
});

test("the exact size bound is retained and an oversize sentinel survives a fresh cache", async () => {
  for (const stored of [false, true]) {
    const id = await identity();
    let saved;
    const background = tasks();
    const bucket = stored ? { get: async () => saved && storedRecord(id, saved.data, { customMetadata: saved.metadata }).object, put: async (key, data, options) => { saved = { data, metadata: options.customMetadata }; } } : null;
    const cache = new IndexCache(limits);
    assert.equal((await cache.get(id, { stored: bucket, buildIndex: async () => bytes(32), waitUntil: background.waitUntil })).byteLength, 32);
    const other = await identity("large");
    await assert.rejects(cache.get(other, { stored: bucket, replace: bytes(), buildIndex: async () => bytes(33), waitUntil: background.waitUntil }), refused(422));
    await background.done();
    await assert.rejects(cache.get(other, { buildIndex: () => assert.fail("sentinel avoids rebuild") }), refused(422));
    if (stored) {
      assert.equal(saved.data.byteLength, 0);
      assert.equal(saved.metadata.state, "too-large");
      const fresh = new IndexCache(limits);
      await assert.rejects(fresh.get(other, { stored: { get: async () => storedRecord(other, bytes(0), { customMetadata: saved.metadata }).object }, buildIndex: () => assert.fail("stored sentinel") }), refused(422));
      idle(fresh);
    }
    idle(cache);
  }
});

test("same-identity builds join, failures release ownership, and retries build again", async () => {
  const id = await identity();
  const gate = deferred();
  const entered = deferred();
  const cache = new IndexCache(limits);
  let builds = 0;
  const request = { stored: null, waitUntil: () => {}, buildIndex: async () => { builds += 1; entered.resolve(); return gate.promise; } };
  const calls = Array.from({ length: 5 }, () => cache.get(id, request));
  const settled = Promise.allSettled(calls);
  await entered.promise;
  assert.equal(builds, 1);
  assert.equal(cache.stats().entries, 1);
  gate.reject(new Error("build failed"));
  assert.ok((await settled).every((item) => item.status === "rejected"));
  idle(cache);
  const value = bytes();
  const retry = await Promise.all(Array.from({ length: 5 }, () => cache.get(id, { ...request, buildIndex: async () => { builds += 1; return value; } })));
  assert.equal(builds, 2);
  assert.ok(retry.every((item) => item === value));
  idle(cache);
});

test("admission stops at sixteen stored loads before the seventeenth get", async () => {
  const cache = new IndexCache({ storedBytes: 32, cacheBytes: 1024 });
  const ids = await Promise.all(Array.from({ length: 17 }, (_, i) => identity(String(i))));
  const gate = deferred();
  const entered = deferred();
  let gets = 0;
  const request = (id) => ({ stored: { get: async () => { gets += 1; assert.ok(cache.stats().entries <= 16); if (gets === 16) entered.resolve(); await gate.promise; return storedRecord(id, bytes()).object; } }, buildIndex: () => assert.fail("stored hit"), waitUntil: () => {} });
  const calls = ids.slice(0, 16).map((id) => cache.get(id, request(id)));
  await assert.rejects(cache.get(ids[16], request(ids[16])), refused(503));
  await entered.promise;
  assert.equal(gets, 16);
  assert.equal(cache.stats().entries, 16);
  gate.resolve();
  await Promise.all(calls);
  await cache.get(ids[16], request(ids[16]));
  assert.equal(gets, 17);
  idle(cache);
});

test("byte reservations shed concurrent stored loads before reading a body", async () => {
  const cache = new IndexCache(limits);
  const first = await identity("first");
  const second = await identity("second");
  const pulling = deferred();
  let controller;
  const body = new ReadableStream({ start(value) { controller = value; }, pull() { pulling.resolve(); } }, { highWaterMark: 0 });
  const firstCall = cache.get(first, { stored: { get: async () => ({ size: 24, customMetadata: { ...first.metadata, state: "ready" }, body }) }, buildIndex: () => assert.fail("stored hit") });
  await pulling.promise;
  assert.equal(cache.stats().reservedBytes, 24);
  const record = storedRecord(second, bytes(24));
  await assert.rejects(cache.get(second, { stored: { get: async () => record.object }, buildIndex: () => assert.fail("no capacity") }), refused(503));
  assert.equal(record.state.pulled, 0);
  assert.equal(record.state.cancelled, 1);
  controller.enqueue(bytes(24)); controller.close();
  await firstCall;
  await cache.get(second, { stored: { get: async () => storedRecord(second, bytes(24)).object }, buildIndex: () => assert.fail("stored hit") });
  assert.equal(cache.stats().readyBytes, 24);
  idle(cache);
});

test("ready hits refresh LRU and both byte and entry pressure evict", async () => {
  for (const [cacheBytes, count] of [[1024, 16], [32, 2]]) {
    const cache = new IndexCache({ storedBytes: 32, cacheBytes });
    const ids = await Promise.all(Array.from({ length: count + 1 }, (_, i) => identity(String(i))));
    let builds = 0;
    const request = { buildIndex: async () => { builds += 1; return bytes(); } };
    for (const id of ids.slice(0, count)) await cache.get(id, request);
    await cache.get(ids[0], request);
    await cache.get(ids[count], request);
    assert.equal(builds, count + 1);
    await cache.get(ids[0], request);
    assert.equal(builds, count + 1);
    await cache.get(ids[1], request);
    assert.equal(builds, count + 2);
    idle(cache);
  }
});

test("a second identity cannot build while the one build slot is occupied", async () => {
  const cache = new IndexCache(limits);
  const one = await identity("one"), two = await identity("two");
  const gate = deferred(), entered = deferred();
  const first = cache.get(one, { buildIndex: async () => { entered.resolve(); return gate.promise; } });
  await entered.promise;
  await assert.rejects(cache.get(two, { replace: bytes(), buildIndex: () => assert.fail("busy") }), refused(503));
  assert.equal(cache.stats().loading, 1);
  gate.resolve(bytes()); await first;
  idle(cache);
});

test("rebuilds during a stored load replace its bytes once and keep one owner", async () => {
  const id = await identity();
  const cache = new IndexCache(limits);
  const gate = deferred(), entered = deferred(), finish = deferred();
  let gets = 0, builds = 0, puts = 0;
  const old = bytes(16, 1), fresh = bytes(16, 2);
  const request = { stored: { get: async () => { gets += 1; entered.resolve(); await gate.promise; return storedRecord(id, old.slice()).object; }, put: async () => { puts += 1; } }, buildIndex: async () => { builds += 1; await finish.promise; return fresh; }, waitUntil: () => {} };
  const normal = cache.get(id, request);
  await entered.promise;
  const rebuild = cache.get(id, { ...request, replace: old });
  const another = cache.get(id, { ...request, replace: old });
  gate.resolve();
  const loaded = await normal;
  await turn();
  assert.equal(builds, 1);
  assert.equal(cache.stats().loading, 1);
  finish.resolve();
  assert.equal(await rebuild, fresh);
  assert.equal(await another, fresh);
  assert.equal(await cache.get(id, { ...request, replace: loaded }), fresh);
  assert.equal(gets, 1); assert.equal(builds, 1); assert.equal(puts, 1);
  idle(cache);
});

test("a normal load that builds already satisfies waiting rebuilds even if uncached", async () => {
  for (const cacheBytes of [8, 32]) {
    const id = await identity();
    const cache = new IndexCache({ storedBytes: 32, cacheBytes });
    const gate = deferred(), entered = deferred();
    const fresh = bytes();
    let builds = 0;
    const request = { stored: { get: async () => { entered.resolve(); await gate.promise; return null; }, put: async () => {} }, buildIndex: async () => { builds += 1; return fresh; }, waitUntil: () => {} };
    const normal = cache.get(id, request);
    await entered.promise;
    const rebuilding = cache.get(id, { ...request, replace: bytes() });
    gate.resolve();
    assert.equal(await normal, fresh);
    assert.equal(await rebuilding, fresh);
    assert.equal(builds, 1);
    idle(cache);
  }
});

test("a failed rebuild drops damaged ready bytes and allows another load", async () => {
  const id = await identity();
  const cache = new IndexCache(limits);
  const old = bytes(16, 1), fresh = bytes(16, 2);
  let gets = 0, builds = 0;
  const request = { stored: { get: async () => { gets += 1; return storedRecord(id, old).object; }, put: async () => {} }, buildIndex: async () => { builds += 1; throw new Error("build failed"); }, waitUntil: () => {} };
  const loaded = await cache.get(id, request);
  await assert.rejects(cache.get(id, { ...request, replace: loaded }), /build failed/);
  assert.equal(cache.stats().entries, 0);
  assert.deepEqual(await cache.get(id, request), old);
  const rebuilt = await cache.get(id, { ...request, replace: loaded, buildIndex: async () => { builds += 1; return fresh; } });
  assert.equal(rebuilt, fresh);
  assert.equal(await cache.get(id, { ...request, replace: loaded }), fresh);
  assert.equal(gets, 2); assert.equal(builds, 2);
  const newer = bytes(16, 3);
  assert.equal(await cache.get(id, { ...request, replace: fresh, buildIndex: async () => { builds += 1; return newer; } }), newer);
  assert.equal(builds, 3);
  idle(cache);
});

test("different builds coexist in one store and put failures log no error text", async () => {
  const records = new Map(), logs = [], pending = tasks();
  const ids = [await identity("same", a), await identity("same", b)];
  const stored = { get: async (key) => { const r = records.get(key); return r ? storedRecord(r.id, r.data).object : null; }, put: async (key, data, opts) => { records.set(key, { id: { metadata: opts.customMetadata }, data }); } };
  let builds = 0;
  for (const id of [ids[0], ids[1], ids[0]]) {
    const cache = new IndexCache(limits);
    await cache.get(id, { stored, buildIndex: async () => { builds += 1; return bytes(); }, waitUntil: pending.waitUntil });
    await pending.done(); idle(cache);
  }
  assert.equal(builds, 2); assert.equal(records.size, 2);
  const original = console.log;
  console.log = (line) => logs.push(line);
  try {
    const cache = new IndexCache(limits);
    await cache.get(ids[0], { stored: { get: async () => null, put: async () => { const error = new Error("SECRETTOK"); error.name = "SECRETKEY"; throw error; } }, buildIndex: async () => bytes(), waitUntil: pending.waitUntil });
    await pending.done();
    log("SECRETSIG");
    for (const value of OUTCOMES) log(value);
  } finally { console.log = original; }
  assert.doesNotMatch(logs.join("\n"), /SECRET/);
  assert.equal(JSON.parse(logs[0]).outcome, "store_put_failed");
  assert.equal(JSON.parse(logs[1]).outcome, "internal");
});
