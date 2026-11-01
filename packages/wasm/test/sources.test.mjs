import assert from "node:assert/strict";

import { test } from "node:test";

import { RequestLimitError, SourceChangedError } from "../src/errors.js";

import { bytesSource, rangeSource } from "../src/sources.js";

import { fakeFetchRange, random } from "./helpers.mjs";

const MiB = 1 << 20;

function data(n) {
  const bytes = new Uint8Array(n);
  for (let i = 0; i < n; i++) bytes[i] = i * 2654435761 >>> 24;
  return bytes;
}

test("random reads return the right bytes", async () => {
  const bytes = data(3 * MiB + 17);
  const source = rangeSource(fakeFetchRange(bytes), {
    firstSpan: 4096,
    maxSkip: 1e5
  });
  const session = await source.open();
  assert.equal(session.size, bytes.length);
  const next = random(7);
  for (let i = 0; i < 400; i++) {
    const length = 1 + Math.floor(next() * 7e4);
    const offset = Math.floor(next() * (bytes.length - length));
    const got = await session.read(offset, length);
    assert.deepEqual(got, bytes.subarray(offset, offset + length), `read ${i} at ${offset}`);
  }
  session.close();
  assert.ok(source.lastStats.requests > 1);
});

test("a sequential scan runs through a pipeline in few requests", async () => {
  const bytes = data(40 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    segmentBytes: 4 * MiB
  });
  const session = await source.open();
  for (let at = 0; at < bytes.length; at += 65536) {
    const got = await session.read(at, Math.min(65536, bytes.length - at));
    assert.equal(got[0], bytes[at]);
    assert.equal(got[got.length - 1], bytes[at + got.length - 1]);
  }
  session.close();
  assert.ok(log.length <= 11, `${log.length} requests`);
  assert.equal(source.lastStats.pipelines, 1);
});

test("the pipeline keeps several requests in flight, within its buffer", async () => {
  const bytes = data(64 * MiB);
  let open = 0;
  let most = 0;
  const inner = fakeFetchRange(bytes);
  const source = rangeSource(async request => {
    open += 1;
    most = Math.max(most, open);
    const response = await inner(request);
    const reader = response.body.getReader();
    const body = new ReadableStream({
      async pull(c) {
        const {done: done, value: value} = await reader.read();
        if (done) {
          open -= 1;
          c.close();
        } else c.enqueue(value);
      },
      cancel() {
        open -= 1;
        return reader.cancel();
      }
    });
    return {
      ...response,
      body: body
    };
  }, {
    firstSpan: MiB,
    segmentBytes: MiB,
    parallel: 4,
    bufferBytes: 8 * MiB
  });
  const session = await source.open();
  for (let at = 0; at + 65536 <= bytes.length; at += 65536 + 1e3) {
    assert.deepEqual(await session.read(at, 65536), bytes.subarray(at, at + 65536));
  }
  session.close();
  assert.ok(most >= 3 && most <= 5, `${most} requests open at once`);
});

test("module advice starts the pipeline where the copy pass begins", async () => {
  const bytes = data(32 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: MiB,
    segmentBytes: 2 * MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  session.plan(20 * MiB, 4 * MiB);
  assert.ok(log.some(r => r.offset === 20 * MiB), JSON.stringify(log));
  const before = log.length;
  for (let at = 20 * MiB; at < 24 * MiB; at += 65536) {
    assert.deepEqual(await session.read(at, 65536), bytes.subarray(at, at + 65536));
  }
  assert.ok(log.length <= before + 1, `${log.length - before} more requests`);
  session.close();
});

function countingFetch(bytes, fake = {}) {
  const inner = fakeFetchRange(bytes, fake);
  let open = 0;
  let most = 0;
  const fetchRange = async request => {
    const response = await inner(request);
    open += 1;
    most = Math.max(most, open);
    let counted = true;
    const release = () => {
      if (counted) open -= 1;
      counted = false;
    };
    const reader = response.body.getReader();
    const body = new ReadableStream({
      async pull(c) {
        const {done: done, value: value} = await reader.read();
        if (done) {
          release();
          c.close();
        } else c.enqueue(value);
      },
      cancel() {
        release();
        return reader.cancel();
      }
    }, {
      highWaterMark: 0
    });
    return {
      ...response,
      body: body
    };
  };
  return {
    fetchRange: fetchRange,
    open: () => open,
    most: () => most
  };
}

test("a pipeline releases each segment's response once read to its end", async () => {
  const bytes = data(40 * MiB);
  const fake = countingFetch(bytes);
  const source = rangeSource(fake.fetchRange, {
    firstSpan: MiB,
    segmentBytes: 2 * MiB,
    parallel: 4,
    bufferBytes: 8 * MiB
  });
  const session = await source.open();
  for (let at = 0; at < bytes.length; at += 65536) {
    assert.deepEqual(await session.read(at, Math.min(65536, bytes.length - at)), bytes.subarray(at, at + 65536));
  }
  assert.ok(fake.most() <= 5, `${fake.most()} responses open at once`);
  session.close();
  assert.equal(fake.open(), 0);
});

test("two pipelines share the connections: at most five responses open", async () => {
  const bytes = data(64 * MiB);
  const fake = countingFetch(bytes);
  const source = rangeSource(fake.fetchRange, {
    firstSpan: 256 * 1024,
    maxSkip: MiB,
    segmentBytes: 2 * MiB,
    parallel: 4,
    bufferBytes: 8 * MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  session.plan(2 * MiB, 50 * MiB);
  for (let i = 0; i < 300; i++) {
    for (const base of [ 2 * MiB, 32 * MiB ]) {
      const at = base + i * 65536;
      assert.deepEqual(await session.read(at, 65536), bytes.subarray(at, at + 65536), `read at ${at}`);
    }
  }
  const stats = session.stats();
  assert.equal(stats.pipelines >= 2, true, JSON.stringify(stats));
  assert.ok(fake.most() <= 5, `${fake.most()} responses open at once`);
  session.close();
  assert.equal(fake.open(), 0);
});

test("a read past what a pipeline will request restarts it there", async () => {
  const bytes = data(32 * MiB);
  const source = rangeSource(fakeFetchRange(bytes), {
    firstSpan: MiB,
    segmentBytes: MiB,
    parallel: 2,
    retries: 0
  });
  const session = await source.open();
  for (let at = 0; at < 2 * MiB; at += 65536) await session.read(at, 65536);
  const at = 8 * MiB;
  assert.deepEqual(await session.read(at, 65536), bytes.subarray(at, at + 65536));
  session.close();
});

test("a pipeline segment that finds the object changed fails the read", async () => {
  const bytes = data(16 * MiB);
  let etag = "v1";
  const source = rangeSource(fakeFetchRange(bytes, {
    hooks: {
      validator: () => etag
    }
  }), {
    firstSpan: MiB,
    segmentBytes: MiB,
    parallel: 2,
    bufferBytes: 2 * MiB
  });
  const session = await source.open();
  for (let at = 0; at < 2 * MiB; at += 65536) await session.read(at, 65536);
  etag = "v2";
  await assert.rejects(async () => {
    for (let at = 2 * MiB; at < bytes.length; at += 65536) await session.read(at, 65536);
  }, SourceChangedError);
  session.close();
});

test("readNow copies only bytes already here", async () => {
  const bytes = data(16 * MiB);
  const source = rangeSource(fakeFetchRange(bytes), {
    firstSpan: MiB
  });
  const session = await source.open();
  await session.read(0, 1e3);
  const view = new Uint8Array(500);
  assert.equal(session.readNow(100, view), 500);
  assert.deepEqual(view, bytes.subarray(100, 600));
  assert.equal(session.readNow(10 * MiB, new Uint8Array(500)), 0);
  assert.deepEqual(await session.read(10 * MiB, 500), bytes.subarray(10 * MiB, 10 * MiB + 500));
  session.close();
  assert.equal(session.readNow(100, view), 0);
});

test("advised regions are read in spans that skip the large gaps", async () => {
  const bytes = data(64 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 64 * 1024,
    sparseGap: MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  const regions = [];
  for (let at = MiB; at < 60 * MiB; at += 3 * MiB) regions.push(at, 4096, at + 100 * 1024, 4096);
  const before = log.length;
  session.planRegions(Float64Array.from(regions));
  for (let i = 0; i < regions.length; i += 2) {
    const at = regions[i];
    assert.deepEqual(await session.read(at, 4096), bytes.subarray(at, at + 4096), `region at ${at}`);
  }
  const requested = log.slice(before).reduce((n, r) => n + r.length, 0);
  assert.ok(requested < 3 * MiB, `${requested} bytes requested`);
  assert.deepEqual(await session.read(30 * MiB + 7, 1e3), bytes.subarray(30 * MiB + 7, 30 * MiB + 1007));
  session.close();
});

test("advised regions fit the requests left: gaps widen until they do", async () => {
  const bytes = data(32 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 64 * 1024,
    sparseGap: 4096,
    maxRequests: 41
  });
  const session = await source.open();
  await session.read(0, 100);
  const regions = [];
  for (let at = 65536; at < 32 * MiB - 4096; at += 65536) regions.push(at, 4096);
  session.planRegions(Float64Array.from(regions));
  for (let i = 0; i < regions.length; i += 2) {
    const at = regions[i];
    assert.deepEqual(await session.read(at, 4096), bytes.subarray(at, at + 4096));
  }
  assert.ok(log.length <= 21, `${log.length} requests`);
  session.close();
});

test("a read that runs past an advised region into a skipped gap keeps the plan", async () => {
  const bytes = data(64 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 64 * 1024,
    sparseGap: MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  const regions = [];
  for (let at = MiB; at < 61 * MiB; at += 3 * MiB) regions.push(at, 100);
  const before = log.length;
  session.planRegions(Float64Array.from(regions));
  for (let i = 0; i < regions.length; i += 2) {
    const at = regions[i];
    assert.deepEqual(await session.read(at, 4096), bytes.subarray(at, at + 4096), `read at ${at}`);
  }
  const requested = log.slice(before).reduce((n, r) => n + r.length, 0);
  assert.ok(requested < 4 * MiB, `${requested} bytes requested`);
  session.close();
});

for (const ended of [ true, false ]) {
  test(`after advised regions${ended ? " end" : ""}, a pass over the gaps reads ahead again`, async () => {
    const bytes = data(64 * MiB);
    const log = [];
    const source = rangeSource(fakeFetchRange(bytes, {
      log: log
    }), {
      firstSpan: 64 * 1024,
      sparseGap: MiB
    });
    const session = await source.open();
    await session.read(0, 100);
    const regions = [];
    for (let at = MiB; at < 61 * MiB; at += 3 * MiB) regions.push(at, 4096);
    session.planRegions(Float64Array.from(regions));
    for (let i = 0; i < regions.length; i += 2) await session.read(regions[i], 4096);
    if (ended) session.planRegions(new Float64Array(0));
    const before = log.length;
    for (let at = 64 * 1024; at + 4096 <= bytes.length; at += 32 * 1024) {
      assert.deepEqual(await session.read(at, 4096), bytes.subarray(at, at + 4096), `read at ${at}`);
    }
    assert.ok(log.length - before < 40, `${log.length - before} requests`);
    session.close();
  });
}

test("advised spans larger than the requests allow use larger segments", async () => {
  const bytes = data(10 * MiB);
  const source = rangeSource(fakeFetchRange(bytes), {
    firstSpan: 64 * 1024,
    segmentBytes: 64 * 1024,
    maxRequests: 60,
    sparseGap: 4096
  });
  const session = await source.open();
  await session.read(0, 100);
  const regions = [];
  for (let i = 0; i < 100; i++) regions.push(i * 100 * 1024 + 4096, 4096);
  session.planRegions(Float64Array.from(regions));
  for (let i = 0; i < regions.length; i += 2) {
    const at = regions[i];
    assert.deepEqual(await session.read(at, 4096), bytes.subarray(at, at + 4096));
  }
  session.close();
});

test("gaps up to maxSkip are read through, larger ones start a new request", async () => {
  const bytes = data(8 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 8 * MiB,
    maxSkip: MiB,
    cacheBytes: 0
  });
  const session = await source.open();
  await session.read(0, 100);
  await session.read(512 * 1024, 100);
  assert.equal(log.length, 1);
  await session.read(4 * MiB, 100);
  assert.equal(log.length, 2);
  assert.equal(log[1].offset, 4 * MiB);
  await session.read(10, 10);
  assert.equal(log.length, 3);
  session.close();
});

test("recent bytes serve reads that go back", async () => {
  const bytes = data(8 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 4 * MiB,
    maxSkip: 0,
    cacheBytes: 2 * MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  await session.read(100, 6e5);
  assert.deepEqual(await session.read(10, 3e5), bytes.subarray(10, 300010));
  assert.deepEqual(await session.read(599e3, 3e5), bytes.subarray(599e3, 899e3));
  assert.equal(log.length, 1);
  await session.read(6 * MiB, 2 * MiB);
  await session.read(10, 10);
  assert.equal(log.length, 3);
  session.close();
});

test("a response stays open while another position is read", async () => {
  const bytes = data(32 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: MiB,
    maxSkip: 4096,
    cacheBytes: 0
  });
  const session = await source.open();
  assert.deepEqual(await session.read(0, 64), bytes.subarray(0, 64));
  assert.deepEqual(await session.read(31 * MiB, 4096), bytes.subarray(31 * MiB, 31 * MiB + 4096));
  assert.deepEqual(await session.read(1e3, 7e4), bytes.subarray(1e3, 71e3));
  assert.equal(log.length, 2);
  for (const at of [ 8 * MiB, 16 * MiB, 24 * MiB ]) await session.read(at, 10);
  await session.read(2e3, 10);
  assert.equal(log.length, 6);
  session.close();
});

test("the first object seen is pinned", async () => {
  const bytes = data(2 * MiB);
  let etag = "v1";
  const source = rangeSource(fakeFetchRange(bytes, {
    hooks: {
      validator: () => etag
    }
  }), {
    firstSpan: 1024
  });
  const session = await source.open();
  await session.read(0, 10);
  etag = "v2";
  await assert.rejects(session.read(MiB, 10), SourceChangedError);
  session.close();
});

test("a body that fails mid-transfer is requested again", async () => {
  const bytes = data(4 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log,
    hooks: {
      fail: n => n === 2 ? 7e4 : undefined
    }
  }), {
    firstSpan: 64 * 1024
  });
  const session = await source.open();
  for (let at = 0; at < 2 * MiB; at += 5e4) {
    assert.deepEqual(await session.read(at, 5e4), bytes.subarray(at, at + 5e4));
  }
  session.close();
  assert.ok(log.length >= 3);
});

test("retries are bounded", async () => {
  const bytes = data(MiB);
  const source = rangeSource(fakeFetchRange(bytes, {
    hooks: {
      fail: n => n > 1 ? 0 : undefined
    }
  }), {
    firstSpan: 1024,
    retries: 2
  });
  const session = await source.open();
  await assert.rejects(session.read(5e5, 10), /connection reset/);
  assert.equal(session.stats().requests, 4);
  session.close();
});

test("the request cap is enforced before the request", async () => {
  const bytes = data(MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 1024,
    maxSkip: 0,
    maxRequests: 3
  });
  const session = await source.open();
  await session.read(1e5, 10);
  await session.read(2e5, 10);
  await assert.rejects(session.read(3e5, 10), RequestLimitError);
  assert.equal(log.length, 3);
  session.close();
});

test("a response for the wrong offset is refused", async () => {
  const bytes = data(MiB);
  const inner = fakeFetchRange(bytes);
  const source = rangeSource(async request => ({
    ...await inner(request),
    offset: request.offset + 1
  }));
  await assert.rejects(source.open(), /starts at 1/);
});

test("the first request is retried, and a session cancels every body it ends", async () => {
  const bytes = data(8 * MiB);
  let first = true;
  const inner = fakeFetchRange(bytes);
  let cancels = 0;
  const source = rangeSource(async request => {
    if (first) {
      first = false;
      throw new TypeError("fetch failed");
    }
    const response = await inner(request);
    const reader = response.body.getReader();
    let ended = false;
    const end = () => {
      if (!ended) cancels += 1;
      ended = true;
    };
    const body = new ReadableStream({
      pull: async c => {
        const {done: done, value: value} = await reader.read();
        if (done) {
          end();
          c.close();
        } else c.enqueue(value);
      },
      cancel: () => {
        end();
        return reader.cancel();
      }
    });
    return {
      ...response,
      body: body
    };
  }, {
    firstSpan: 64 * 1024,
    maxSkip: 0
  });
  const session = await source.open();
  for (let at = 0; at < 4 * MiB; at += 65536) await session.read(at, 65536);
  await session.read(7 * MiB, 10);
  session.close();
  const {requests: requests} = source.lastStats;
  assert.equal(cancels, requests - 1, `${cancels} ended for ${requests} requests`);
});

test("a response that holds less than asked is read as far as it goes", async () => {
  const bytes = data(2 * MiB);
  const inner = fakeFetchRange(bytes);
  const log = [];
  const source = rangeSource(async request => {
    log.push(request.offset);
    const response = await inner({
      ...request,
      length: Math.max(1, request.length - 1e3)
    });
    return {
      ...response,
      end: Math.min(request.offset + request.length - 1e3, bytes.length)
    };
  }, {
    firstSpan: 64 * 1024
  });
  const session = await source.open();
  for (let at = 0; at < MiB; at += 5e4) {
    assert.deepEqual(await session.read(at, 5e4), bytes.subarray(at, at + 5e4));
  }
  session.close();
});

test("the gap between advised regions follows the first response's wait", async () => {
  const bytes = data(48 * MiB);
  const regions = [];
  for (let at = 4 * MiB; at < 44 * MiB; at += 2 * MiB + 4096) regions.push(at, 4096);
  const requested = async (wait, options = {}) => {
    const log = [];
    const inner = fakeFetchRange(bytes, {
      log: log
    });
    let first = true;
    const fetchRange = async request => {
      if (first) {
        first = false;
        await new Promise(resolve => setTimeout(resolve, wait));
      }
      return inner(request);
    };
    const source = rangeSource(fetchRange, {
      firstSpan: 64 * 1024,
      ...options
    });
    const session = await source.open();
    await session.read(0, 100);
    const before = log.length;
    session.planRegions(Float64Array.from(regions));
    for (let i = 0; i < regions.length; i += 2) {
      assert.deepEqual(await session.read(regions[i], 4096), bytes.subarray(regions[i], regions[i] + 4096));
    }
    const gap = session.sparseGap();
    session.close();
    return {
      gap: gap,
      bytes: log.slice(before).reduce((n, r) => n + r.length, 0)
    };
  };
  const near = await requested(5, {
    bandwidth: 1e6
  });
  assert.equal(near.gap, MiB);
  assert.ok(near.bytes < MiB, `${near.bytes} bytes requested`);
  const far = await requested(200);
  assert.ok(far.gap >= 29e5 && far.gap <= 8 * MiB, `gap ${far.gap}`);
  assert.ok(far.bytes > 30 * MiB, `${far.bytes} bytes requested`);
  assert.equal((await requested(200, {
    sparseGap: MiB
  })).gap, MiB);
});

test("advised regions an open response holds need no request", async () => {
  const bytes = data(12 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 4 * MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  const before = log.length;
  const regions = [];
  for (let at = MiB; at < 11 * MiB; at += 300 * 1024) regions.push(at, 64 * 1024);
  session.planRegions(Float64Array.from(regions));
  for (let i = 0; i < regions.length; i += 2) {
    assert.deepEqual(await session.read(regions[i], 64 * 1024), bytes.subarray(regions[i], regions[i] + 64 * 1024));
  }
  const requests = log.slice(before);
  assert.ok(requests.length > 0 && requests.every(r => r.offset >= 4 * MiB), JSON.stringify(requests));
  session.close();
});

test("a header scan's regions in the first response need no request there", async () => {
  const bytes = data(24 * MiB);
  const regions = [];
  for (let at = 64 * 1024; at < 23 * MiB; at += 1.5 * MiB) regions.push(at, 4096);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 4 * MiB,
    sparseGap: MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  const before = log.length;
  session.planRegions(Float64Array.from(regions));
  for (let i = 0; i < regions.length; i += 2) {
    assert.deepEqual(await session.read(regions[i], 4096), bytes.subarray(regions[i], regions[i] + 4096));
  }
  const requests = log.slice(before);
  const after = regions.filter((at, i) => i % 2 === 0 && at >= 4 * MiB).length;
  assert.ok(requests.every(r => r.offset >= 4 * MiB), JSON.stringify(requests));
  assert.equal(requests.length, after);
  session.close();
});

test("a read past the open response before the advised pipeline asks for its own bytes", async () => {
  const bytes = data(32 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 4 * MiB,
    sparseGap: MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  const before = log.length;
  const regions = [ 64 * 1024, 4 * MiB - 8192, 6 * MiB, 9 * MiB, 12 * MiB, 20 * MiB ];
  session.planRegions(Float64Array.from(regions.flatMap(at => [ at, 4096 ])));
  for (const [i, at] of regions.entries()) {
    const n = i === 1 ? 16 * 1024 : 4096;
    assert.deepEqual(await session.read(at, n), bytes.subarray(at, at + n));
  }
  const requested = log.slice(before).reduce((n, r) => n + r.length, 0);
  assert.ok(requested < MiB, JSON.stringify(log.slice(before)));
  assert.equal(session.stats().pipelines, 1);
  session.close();
});

test("advice an open response serves whole ends the advice before", async () => {
  const bytes = data(64 * MiB);
  const log = [];
  const source = rangeSource(fakeFetchRange(bytes, {
    log: log
  }), {
    firstSpan: 4 * MiB,
    sparseGap: MiB
  });
  const session = await source.open();
  await session.read(0, 100);
  session.planRegions(Float64Array.from([ 30 * MiB, 4096, 40 * MiB, 4096 ]));
  assert.equal(session.forward.length, 1);
  session.planRegions(Float64Array.from([ MiB, 4096, 2 * MiB, 4096 ]));
  assert.equal(session.forward.length, 0);
  const before = log.length;
  for (const at of [ MiB, 2 * MiB ]) assert.deepEqual(await session.read(at, 4096), bytes.subarray(at, at + 4096));
  assert.equal(log.length, before);
  session.close();
});

test("options are checked", () => {
  assert.throws(() => rangeSource(async () => {}, {
    sparseGap: "always"
  }), TypeError);
  assert.throws(() => rangeSource(async () => {}, {
    firstSpan: 0
  }), TypeError);
  assert.throws(() => rangeSource(async () => {}, {
    spans: 3
  }), TypeError);
  assert.throws(() => bytesSource("text"), TypeError);
});

test("the read-ahead stops at parallel segments, read or running", async () => {
  const KiB = 1024;
  const bytes = data(4 * MiB);
  const log = [];
  let release;
  const gate = new Promise(resolve => {
    release = resolve;
  });
  const inner = fakeFetchRange(bytes, {
    log: log
  });
  const fetchRange = async request => {
    const response = await inner(request);
    if (request.offset !== 64 * KiB) return response;
    const reader = response.body.getReader();
    const body = new ReadableStream({
      async pull(controller) {
        await gate;
        const {done: done, value: value} = await reader.read();
        if (done) controller.close(); else controller.enqueue(value);
      }
    });
    return {
      ...response,
      body: body
    };
  };
  const source = rangeSource(fetchRange, {
    firstSpan: 64 * KiB,
    segmentBytes: 256 * KiB,
    parallel: 2,
    bufferBytes: 2 * MiB,
    maxSkip: 64 * KiB
  });
  const session = await source.open();
  await session.read(0, 100);
  const pending = session.read(64 * KiB, 1e3);
  await new Promise(resolve => setTimeout(resolve, 50));
  const segments = log.filter(request => request.offset >= 64 * KiB);
  assert.equal(segments.length, 2, JSON.stringify(segments));
  release();
  assert.deepEqual(await pending, bytes.subarray(64 * KiB, 64 * KiB + 1e3));
  for (let at = 64 * KiB + 1e3; at < bytes.length; at += 5e4) {
    const length = Math.min(5e4, bytes.length - at);
    assert.deepEqual(await session.read(at, length), bytes.subarray(at, at + length), `read at ${at}`);
  }
  session.close();
});

test("memoryBytes bounds every byte the reader holds", async () => {
  const bytes = data(96 * MiB);
  const r2Like = {
    parallel: 6,
    segmentBytes: 8 * MiB,
    bufferBytes: 48 * MiB
  };
  for (const [options, openOptions, bound] of [ [ r2Like, {}, 64 * MiB ], [ {
    ...r2Like,
    memoryBytes: 24 * MiB
  }, {}, 24 * MiB ], [ r2Like, {
    memoryBytes: 16 * MiB
  }, 16 * MiB ], [ r2Like, {
    memoryBytes: 40 * MiB
  }, 40 * MiB ], [ {
    ...r2Like,
    memoryBytes: 24 * MiB
  }, {
    memoryBytes: 32 * MiB
  }, 24 * MiB ] ]) {
    const source = rangeSource(fakeFetchRange(bytes), options);
    const session = await source.open(openOptions);
    for (let at = 0; at < bytes.length; at += 65536) {
      const got = await session.read(at, 65536);
      if (at % (8 * MiB) === 0) assert.deepEqual(got, bytes.subarray(at, at + 65536), `read at ${at}`);
    }
    session.close();
    const held = source.lastStats.maxHeldBytes;
    assert.ok(held > 0 && held <= bound, `${held} bytes held, bound ${bound}`);
  }
  assert.throws(() => rangeSource(fakeFetchRange(bytes), {
    memoryBytes: MiB
  }), /memoryBytes/);
  await assert.rejects(rangeSource(fakeFetchRange(bytes)).open({
    memoryBytes: MiB
  }), /memoryBytes/);
});

test("segments larger than their share of the buffer still run in parallel", async () => {
  const KiB = 1024;
  const bytes = data(32 * MiB);
  const log = [];
  let release;
  const gate = new Promise(resolve => {
    release = resolve;
  });
  const inner = fakeFetchRange(bytes, {
    log: log
  });
  const fetchRange = async request => {
    const response = await inner(request);
    if (request.offset !== 64 * KiB) return response;
    const reader = response.body.getReader();
    const body = new ReadableStream({
      async pull(controller) {
        await gate;
        const {done: done, value: value} = await reader.read();
        if (done) controller.close(); else controller.enqueue(value);
      }
    });
    return {
      ...response,
      body: body
    };
  };
  const source = rangeSource(fetchRange, {
    firstSpan: 64 * KiB,
    segmentBytes: 4 * MiB,
    parallel: 4,
    bufferBytes: 4 * MiB,
    maxSkip: 64 * KiB
  });
  const session = await source.open();
  await session.read(0, 100);
  const pending = session.read(64 * KiB, 1e3);
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(log.filter(request => request.offset >= 64 * KiB).length, 4);
  release();
  assert.deepEqual(await pending, bytes.subarray(64 * KiB, 64 * KiB + 1e3));
  for (let at = 64 * KiB + 1e3; at < bytes.length; at += 1e5) {
    const length = Math.min(1e5, bytes.length - at);
    assert.deepEqual(await session.read(at, length), bytes.subarray(at, at + length), `read at ${at}`);
  }
  session.close();
  assert.ok(source.lastStats.maxHeldBytes <= 4 * MiB + 8 * MiB + 8 * MiB);
});

function sixConnections(bytes) {
  let open = 0;
  let most = 0;
  const queue = [];
  const release = () => {
    open -= 1;
    const next = queue.shift();
    if (next) {
      open += 1;
      most = Math.max(most, open);
      next();
    }
  };
  const fetchRange = async ({offset: offset, length: length}) => {
    if (open < 6) {
      open += 1;
      most = Math.max(most, open);
    } else {
      await new Promise(resolve => queue.push(resolve));
    }
    const end = Math.min(offset + length, bytes.length);
    let at = offset;
    let closed = false;
    const done = () => {
      if (!closed) {
        closed = true;
        release();
      }
    };
    const body = new ReadableStream({
      pull(controller) {
        if (at >= end) {
          done();
          return controller.close();
        }
        const n = Math.min(end - at, 4096);
        controller.enqueue(bytes.slice(at, at + n));
        at += n;
      },
      cancel: done
    }, {
      highWaterMark: 0
    });
    return {
      body: body,
      offset: offset,
      size: bytes.length,
      validator: "e"
    };
  };
  return {
    fetchRange: fetchRange,
    most: () => most
  };
}

test("two forward regions and reads elsewhere stay within six connections and the memory bound", async () => {
  const bytes = data(160 * MiB);
  const r2 = {
    parallel: 6,
    segmentBytes: 8 * MiB,
    bufferBytes: 48 * MiB
  };
  for (const [options, bound] of [ [ r2, 48 * MiB + 2 * 8 * MiB + 8 * MiB ], [ {
    ...r2,
    memoryBytes: 16 * MiB
  }, 16 * MiB ], [ {
    ...r2,
    memoryBytes: 24 * MiB
  }, 24 * MiB ], [ {}, 24 * MiB + 2 * 8 * MiB + 8 * MiB ] ]) {
    const server = sixConnections(bytes);
    const source = rangeSource(server.fetchRange, options);
    const session = await source.open();
    const run = (async () => {
      await session.read(0, 1e3);
      let a = 4 * MiB + 10;
      let b = 80 * MiB;
      for (let i = 0; i < 500; i++) {
        assert.deepEqual(await session.read(a, 65536), bytes.subarray(a, a + 65536));
        assert.deepEqual(await session.read(b, 65536), bytes.subarray(b, b + 65536));
        a += 65536;
        b += 65536;
        if (i % 100 === 50) {
          const at = 150 * MiB + i * 1e3;
          assert.deepEqual(await session.read(at, 2e3), bytes.subarray(at, at + 2e3));
        }
      }
      return "done";
    })();
    const outcome = await Promise.race([ run, new Promise(resolve => setTimeout(() => resolve("waiting"), 2e4)) ]);
    session.close();
    assert.equal(outcome, "done", JSON.stringify(options));
    assert.ok(server.most() <= 6, `${server.most()} responses open at once`);
    const held = source.lastStats.maxHeldBytes;
    assert.ok(held <= bound, `${held} bytes held, bound ${bound}`);
  }
});
