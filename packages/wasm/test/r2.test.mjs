import assert from "node:assert/strict";

import { test } from "node:test";

import { SourceChangedError, SourceNotFoundError } from "../src/errors.js";

import { r2Source, R2_DEFAULTS } from "../src/r2.js";

test("R2 binding and key validation happens before opening a session", () => {
  assert.throws(() => r2Source({}, "a"), TypeError);
  assert.throws(() => r2Source({ get() {} }, 42), TypeError);
});

test("a missing object's error names no key", async () => {
  await assert.rejects(r2Source({ get: async () => null }, "private-key").open(), (error) => {
    assert.ok(error instanceof SourceNotFoundError);
    assert.doesNotMatch(error.message + error.stack, /private-key/);
    return true;
  });
});

test("only requests after the first pin the object's ETag", async () => {
  const bucket = fakeBucket(new Map([["a", { bytes: new Uint8Array(4096), etag: "e1" }]]));
  const requests = [];
  const source = r2Source({ get(key, options) {
    requests.push(options);
    return bucket.get(key, options);
  } }, "a", { firstSpan: 1024, maxSkip: 0 });
  const session = await source.open();
  try {
    await session.read(2048, 256);
    assert.equal(requests[0].onlyIf, undefined);
    assert.ok(requests.length > 1);
    for (const request of requests.slice(1)) assert.deepEqual(request.onlyIf, { etagMatches: "e1" });
  } finally {
    session.close();
  }
});

const MiB = 1 << 20;

function data(n) {
  const bytes = new Uint8Array(n);
  for (let i = 0; i < n; i++) bytes[i] = i * 2654435761 >>> 24;
  return bytes;
}

function fakeBucket(objects) {
  return {
    async get(key, options = {}) {
      const object = objects.get(key);
      if (object === undefined) return null;
      const meta = {
        key: key,
        size: object.bytes.length,
        etag: object.etag
      };
      if (options.onlyIf?.etagMatches !== undefined && options.onlyIf.etagMatches !== object.etag) return meta;
      const offset = options.range?.offset ?? 0;
      const end = Math.min(offset + (options.range?.length ?? object.bytes.length), object.bytes.length);
      const slice = object.bytes.slice(offset, end);
      return {
        ...meta,
        range: {
          offset: offset,
          length: slice.length
        },
        body: new ReadableStream({
          start: c => {
            c.enqueue(slice);
            c.close();
          }
        })
      };
    }
  };
}

test("r2: a forward pass reads 8 MiB segments (R2_DEFAULTS)", async () => {
  const bytes = data(48 * MiB);
  const bucket = fakeBucket(new Map([ [ "a", {
    bytes: bytes,
    etag: "e1"
  } ] ]));
  const lengths = [];
  const logged = {
    get: (key, options) => (lengths.push(options?.range?.length), bucket.get(key, options))
  };
  const session = await r2Source(logged, "a").open();
  for (let at = 0; at < bytes.length; at += 65536) {
    const got = await session.read(at, 65536);
    assert.equal(got[100], bytes[at + 100]);
  }
  session.close();
  assert.ok(lengths.slice(1).every(n => n <= 8 * MiB), JSON.stringify(lengths));
  assert.deepEqual({
    ...R2_DEFAULTS
  }, {
    parallel: 6,
    segmentBytes: 8 * MiB,
    bufferBytes: 48 * MiB
  });
});

test("r2: under R2_DEFAULTS no segment is larger than its cap, up to 3.1 GiB", async () => {
  const cap = R2_DEFAULTS.bufferBytes / R2_DEFAULTS.parallel;
  assert.equal(cap, 8 * MiB);
  const chunk = new Uint8Array(MiB).fill(7);
  for (const size of [ 48 * MiB + 17, 1 << 30, Math.round(3.1 * (1 << 30)) ]) {
    const lengths = [];
    const bucket = {
      async get(key, options = {}) {
        const offset = options.range?.offset ?? 0;
        const end = Math.min(offset + (options.range?.length ?? size), size);
        lengths.push(end - offset);
        let at = offset;
        return {
          key: key,
          size: size,
          etag: "e1",
          range: {
            offset: offset,
            length: end - offset
          },
          body: new ReadableStream({
            pull(controller) {
              if (at >= end) return controller.close();
              const n = Math.min(MiB, end - at);
              controller.enqueue(chunk.subarray(0, n));
              at += n;
            }
          }, {
            highWaterMark: 0
          })
        };
      }
    };
    const source = r2Source(bucket, "a");
    const session = await source.open();
    for (let at = 0; at < size; at += 4 * MiB) await session.read(at, Math.min(4 * MiB, size - at));
    session.close();
    assert.ok(lengths.slice(1).every(n => n <= cap), `${size}: ${Math.max(...lengths.slice(1))} > ${cap}`);
    assert.equal(source.lastStats.pipelines, 1);
    assert.ok(source.lastStats.requests < 1e3, `${source.lastStats.requests} requests`);
  }
});

test("r2: reads, pinning and a missing key", async () => {
  const bytes = data(2 * MiB);
  const objects = new Map([ [ "a", {
    bytes: bytes,
    etag: "e1"
  } ] ]);
  const bucket = fakeBucket(objects);
  const session = await r2Source(bucket, "a", {
    firstSpan: 1024,
    maxSkip: 0
  }).open();
  assert.equal(session.size, bytes.length);
  assert.deepEqual(await session.read(MiB, 100), bytes.subarray(MiB, MiB + 100));
  objects.set("a", {
    bytes: bytes,
    etag: "e2"
  });
  await assert.rejects(session.read(MiB + MiB / 2, 10), SourceChangedError);
  session.close();
  await assert.rejects(r2Source(bucket, "missing").open(), SourceNotFoundError);
});
