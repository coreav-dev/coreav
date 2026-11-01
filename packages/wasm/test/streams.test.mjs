import assert from "node:assert/strict";

import { readFileSync } from "node:fs";

import { join } from "node:path";

import { test } from "node:test";

import { CoreAVError, bytesSource, clip, extractAudio, moduleInfo, probe, rangeSource, remux, sanitize } from "../src/node.js";

import { collect, fakeFetchRange, fakeRangeSource } from "./helpers.mjs";

const fixtures = process.env.COREAV_FIXTURES;

assert.ok(fixtures, "set COREAV_FIXTURES to the generated fixtures directory");

const read = name => new Uint8Array(readFileSync(join(fixtures, name)));

test("an unread output holds the operation, reading it lets it finish", async () => {
  const bytes = read("fragmented_many.mp4");
  const expected = await collect((await clip(bytesSource(bytes), {
    start: 0
  })).body);
  assert.ok(expected.length > 1 << 20, "needs an output larger than the queue");
  const {body: body, summary: summary} = await clip(bytesSource(bytes), {
    start: 0
  });
  let settled = false;
  summary.then(() => {
    settled = true;
  });
  await new Promise(r => setTimeout(r, 50));
  assert.equal(settled, false, "the operation ran ahead of its reader");
  assert.deepEqual(await collect(body), expected);
  assert.equal((await summary).output.bytes, expected.length);
});

test("cancelling the output stops the operation", async () => {
  const bytes = read("fragmented_many.mp4");
  const source = fakeRangeSource(bytes, {
    firstSpan: 1024,
    maxSkip: 0
  });
  const {body: body, summary: summary} = await clip(source, {
    start: 0
  });
  const reader = body.getReader();
  await reader.read();
  await reader.cancel(new Error("client went away"));
  const error = await summary.then(() => null, e => e);
  assert.ok(error instanceof CoreAVError);
  assert.equal(error.kind, "io");
  assert.match(error.cause.message, /client went away/);
});

test("cancelling the output ends a read that is waiting", async () => {
  const bytes = read("fragmented_many.mp4");
  let stall = false;
  const inner = fakeFetchRange(bytes);
  const source = rangeSource(async request => {
    if (!stall) return inner(request);
    return new Promise((_, reject) => request.signal.addEventListener("abort", () => reject(request.signal.reason)));
  }, {
    firstSpan: 2048,
    segmentBytes: 4096,
    parallel: 1,
    bufferBytes: 16384,
    maxSkip: 0,
    retries: 0
  });
  const {body: body, summary: summary} = await clip(source, {
    start: 0
  });
  stall = true;
  const reader = body.getReader();
  await reader.read();
  await reader.cancel(new Error("client went away"));
  const outcome = await Promise.race([ summary.then(() => "resolved", () => "rejected"), new Promise(r => setTimeout(() => r("still running"), 2e3)) ]);
  assert.equal(outcome, "rejected");
});

test("a source that fails before any output rejects, even for a small output", async () => {
  for (const name of [ "basic.mp4", "basic.mov", "faststart.mp4" ]) {
    const bytes = read(name);
    const ok = await clip(fakeRangeSource(bytes, {
      firstSpan: 1024,
      maxSkip: 0,
      segmentBytes: 4096,
      parallel: 1,
      bufferBytes: 16384
    }), {
      start: 0
    });
    const size = (await collect(ok.body)).length;
    assert.ok(size < 256 * 1024, `${name}: needs an output smaller than one chunk`);
    for (const fromRequest of [ 3, 4, 5, 6, 8 ]) {
      let count = 0;
      const inner = fakeFetchRange(bytes);
      const source = rangeSource(async request => {
        count += 1;
        if (count >= fromRequest) throw new Error("connection reset");
        return inner(request);
      }, {
        firstSpan: 1024,
        maxSkip: 0,
        segmentBytes: 4096,
        parallel: 1,
        bufferBytes: 16384,
        retries: 0
      });
      const outcome = await clip(source, {
        start: 0
      }).then(async output => ({
        output: output,
        bytes: await collect(output.body).catch(() => null)
      }), error => ({
        error: error
      }));
      if (outcome.error) {
        assert.ok(outcome.error instanceof CoreAVError, `${name} ${fromRequest}`);
        assert.equal(outcome.error.kind, "io");
      } else {
        assert.equal(outcome.bytes?.length, size, `${name} ${fromRequest}: resolved with a broken body`);
      }
    }
  }
});

test("a foreign session's reads are checked", async () => {
  const bytes = read("basic.mp4");
  for (const answer of [ () => null, n => new Uint8Array(n + 1), () => "text", n => new Uint8Array(1 << 20) ]) {
    const source = {
      open: () => ({
        size: bytes.length,
        read: async (offset, length) => answer(length),
        close() {}
      })
    };
    const before = moduleInfo().memoryBytes;
    await assert.rejects(probe(source), error => error instanceof CoreAVError && error.kind === "io");
    assert.equal(moduleInfo().memoryBytes, before);
  }
});

test("a rejection before any output rejects the call", async () => {
  const bytes = read("audio_mp3.mp4");
  await assert.rejects(extractAudio(bytesSource(bytes)), error => {
    assert.ok(error instanceof CoreAVError);
    assert.equal(error.document.schema, "coreav.error/1");
    return true;
  });
  await assert.rejects(clip(bytesSource(bytes), {}), TypeError);
  await assert.rejects(clip(bytesSource(bytes), {
    start: -1
  }), TypeError);
  await assert.rejects(probe(bytesSource(bytes), {
    limits: {
      maxDepth: "deep"
    }
  }), TypeError);
});

test("a source that fails after the first byte errors the body", async () => {
  const bytes = read("fragmented_many.mp4");
  let failing = false;
  const inner = fakeFetchRange(bytes);
  const source = rangeSource(async request => {
    if (failing) throw new Error("connection reset");
    return inner(request);
  }, {
    firstSpan: 2048,
    segmentBytes: 4096,
    parallel: 1,
    bufferBytes: 16384,
    maxSkip: 0,
    retries: 0
  });
  const result = await clip(source, {
    start: 0
  });
  failing = true;
  await assert.rejects(collect(result.body), /connection reset/);
  await assert.rejects(result.summary, error => {
    assert.ok(error instanceof CoreAVError);
    assert.equal(error.kind, "io");
    assert.match(error.message, /connection reset/);
    assert.match(error.cause.message, /connection reset/);
    return true;
  });
});

test("concurrent operations on one instance give the same results", async () => {
  const names = [ "basic.mp4", "basic.mov", "fragmented.mp4", "multitrack.mp4", "clip_open_gop.mp4" ];
  const inputs = names.map(read);
  const solo = [];
  for (const bytes of inputs) {
    solo.push(await collect((await extractAudio(bytesSource(bytes)).catch(() => ({
      body: null
    }))).body ?? new Blob([]).stream()));
  }
  const together = await Promise.all(inputs.map(async (bytes, i) => {
    const source = fakeRangeSource(bytes, {
      firstSpan: 1024,
      maxSkip: 4096
    }, {
      seed: i + 3
    });
    const output = await extractAudio(source).catch(() => null);
    return output === null ? new Uint8Array : collect(output.body);
  }));
  for (let i = 0; i < inputs.length; i++) assert.deepEqual(together[i], solo[i], names[i]);
  const documents = await Promise.all(inputs.map((b, i) => probe(fakeRangeSource(b, {
    firstSpan: 512
  }, {
    seed: i
  }))));
  for (let i = 0; i < inputs.length; i++) assert.deepEqual(documents[i], await probe(bytesSource(inputs[i])));
  assert.ok(moduleInfo().memoryBytes < 64 << 20);
});

test("remux and sanitize open the session with a lower memory bound", async () => {
  const bytes = read("basic.mp4");
  const seen = [];
  const spy = inner => ({
    get lastStats() {
      return inner.lastStats;
    },
    open(options) {
      seen.push(options);
      return inner.open(options);
    }
  });
  const run = async operation => {
    seen.length = 0;
    const result = await operation(spy(fakeRangeSource(bytes, {
      firstSpan: 1024
    })));
    if (result?.body !== undefined) {
      await collect(result.body);
      await result.summary;
    }
    return seen;
  };
  const bound = [ {
    memoryBytes: 40 * 1024 * 1024
  } ];
  assert.deepEqual(await run(source => remux(source)), bound);
  assert.deepEqual(await run(source => sanitize(source)), bound);
  assert.deepEqual(await run(source => probe(source)), [ undefined ]);
  assert.deepEqual(await run(source => extractAudio(source)), [ undefined ]);
  assert.deepEqual(await run(source => clip(source, {
    start: 0
  })), [ undefined ]);
  const stricter = rangeSource(fakeFetchRange(bytes), {
    firstSpan: 1024,
    memoryBytes: 8 * 1024 * 1024
  });
  const session = await stricter.open({
    memoryBytes: 40 * 1024 * 1024
  });
  session.close();
  assert.deepEqual(await collect((await remux(stricter)).body), await collect((await remux(bytesSource(bytes))).body));
});
