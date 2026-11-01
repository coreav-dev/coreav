import assert from "node:assert/strict";

import { serve } from "./http-server.mjs";

import { test } from "node:test";

import { SourceChangedError, SourceNotFoundError } from "../src/errors.js";

import { httpSource } from "../src/sources.js";

test("a missing object's error names no signed URL", async () => {
  const source = httpSource("https://media.invalid/clip.mp4?signature=private-value", {
    fetch: async () => new Response(null, { status: 404 })
  });
  await assert.rejects(source.open(), (error) => {
    assert.ok(error instanceof SourceNotFoundError);
    assert.doesNotMatch(error.message + error.stack, /private-value|media\.invalid/);
    return true;
  });
});

test("an unparsable URL is rejected before a request", () => {
  let requests = 0;
  assert.throws(() => httpSource("not a URL", {
    fetch: async () => { requests += 1; return new Response(); }
  }), TypeError);
  assert.equal(requests, 0);
});

const MiB = 1 << 20;

function data(n) {
  const bytes = new Uint8Array(n);
  for (let i = 0; i < n; i++) bytes[i] = i * 2654435761 >>> 24;
  return bytes;
}


test("http: ranged reads return the object's bytes", async () => {
  const bytes = data(3 * MiB);
  const server = await serve(bytes);
  try {
    const source = httpSource(server.url, {
      firstSpan: 64 * 1024,
      maxSkip: 0
    });
    const session = await source.open();
    assert.equal(session.size, bytes.length);
    for (const [offset, length] of [ [ 0, 10 ], [ 2 * MiB, 7e4 ], [ 100, 5 ], [ 3 * MiB - 9, 9 ] ]) {
      assert.deepEqual(await session.read(offset, length), bytes.subarray(offset, offset + length));
    }
    session.close();
    assert.ok(server.log.slice(1).every(r => r.ifMatch === '"v1"'));
  } finally {
    await server.close();
  }
});

test("http: a changed object fails the read", async () => {
  const bytes = data(2 * MiB);
  let etag = '"v1"';
  const server = await serve(bytes, {
    etag: () => etag
  });
  try {
    const session = await httpSource(server.url, {
      firstSpan: 4096
    }).open();
    etag = '"v2"';
    await assert.rejects(session.read(MiB, 10), SourceChangedError);
    session.close();
  } finally {
    await server.close();
  }
});

test("http: a server that ignores ranges is read from the start only", async () => {
  const bytes = data(MiB);
  const server = await serve(bytes, {
    ignoreRanges: true
  });
  try {
    const session = await httpSource(server.url, {
      firstSpan: 4096,
      maxSkip: 2 * MiB
    }).open();
    assert.deepEqual(await session.read(5e5, 10), bytes.subarray(5e5, 500010));
    session.close();
    const again = await httpSource(server.url, {
      firstSpan: 4096,
      maxSkip: 0,
      cacheBytes: 0
    }).open();
    await again.read(10, 10);
    await assert.rejects(again.read(5, 10), /does not answer range requests/);
    again.close();
  } finally {
    await server.close();
  }
});

test("http: copy advice does not ask a server that ignores ranges for a range", async () => {
  const bytes = data(40 * MiB);
  const server = await serve(bytes, {
    ignoreRanges: true
  });
  try {
    const session = await httpSource(server.url, {
      firstSpan: MiB
    }).open();
    await session.read(0, 10);
    session.plan(20 * MiB, 4 * MiB);
    for (let at = 20 * MiB; at < 21 * MiB; at += 65536) {
      assert.deepEqual(await session.read(at, 65536), bytes.subarray(at, at + 65536));
    }
    session.close();
  } finally {
    await server.close();
  }
});

test("http: redirects are refused unless followed", async () => {
  const bytes = data(MiB);
  const target = await serve(bytes);
  const server = await serve(bytes, {
    redirect: target.url
  });
  try {
    await assert.rejects(httpSource(server.url).open(), /redirected/);
    const session = await httpSource(server.url, {
      followRedirects: true
    }).open();
    assert.equal(session.size, bytes.length);
    session.close();
  } finally {
    await server.close();
    await target.close();
  }
});

test("http: a 200 without a length is refused", async () => {
  const server = await serve(data(MiB), {
    chunked: true
  });
  try {
    await assert.rejects(httpSource(server.url).open(), /Content-Length/);
  } finally {
    await server.close();
  }
});

test("http: an empty object has size 0", async () => {
  const server = await serve(new Uint8Array);
  try {
    const session = await httpSource(server.url).open();
    assert.equal(session.size, 0);
    session.close();
  } finally {
    await server.close();
  }
});
