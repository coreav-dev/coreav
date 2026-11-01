import assert from "node:assert/strict";

import { serve } from "./http-server.mjs";

import { test } from "node:test";

import { SourceChangedError, SourceNotFoundError } from "../src/errors.js";

import { httpSource } from "../src/sources.js";
import { errorResponse } from "../src/errors.js";
import { createServer } from "node:http";
import { gzipSync } from "node:zlib";

test("HTTP rejects userinfo without echoing it or starting a request", async () => {
  let requests = 0;
  let error;
  try {
    await httpSource("https://user:credential-marker@media.invalid/file", {
      fetch: async () => { requests += 1; throw new Error("unexpected request"); }
    }).open();
  } catch (caught) { error = caught; }
  assert.ok(error instanceof TypeError);
  assert.equal(requests, 0);
  assert.doesNotMatch(error.message, /credential-marker|media\.invalid/);
  assert.doesNotMatch(await errorResponse(error).text(), /credential-marker|media\.invalid/);
});

for (const status of [200, 206]) {
  test(`HTTP refuses gzip-encoded ${status} responses rather than truncating decoded bytes`, async () => {
    const bytes = new Uint8Array(201000).fill(7);
    const encoded = gzipSync(bytes);
    const encodings = [];
    const server = createServer((request, response) => {
      encodings.push(request.headers["accept-encoding"]);
      const headers = { "content-encoding": "gzip", "content-length": encoded.byteLength };
      if (status === 206) headers["content-range"] = `bytes 0-${encoded.byteLength - 1}/${encoded.byteLength}`;
      response.writeHead(status, headers).end(encoded);
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    try {
      await assert.rejects(httpSource(`http://127.0.0.1:${server.address().port}/file`).open(), /content.encoding|compressed/i);
      assert.equal(encodings.length, 1);
      assert.ok(encodings[0].split(",").every((encoding) => encoding.trim() === "identity"));
    } finally {
      await new Promise((resolve) => server.close(resolve));
    }
  });
}

test("cross-origin redirects strip caller headers before contacting the target", async () => {
  const keys = [];
  const target = await serve(new Uint8Array(16), { onRequest: (request) => keys.push(request.headers["x-api-key"]) });
  const originKeys = [];
  const origin = await serve(new Uint8Array(), { redirect: target.url, onRequest: (request) => originKeys.push(request.headers["x-api-key"]) });
  try {
    const session = await httpSource(origin.url, { followRedirects: true, headers: { "x-api-key": "header-marker" } }).open();
    session.close();
    assert.deepEqual(originKeys, ["header-marker"]);
    assert.deepEqual(keys, [undefined]);
  } finally {
    await origin.close();
    await target.close();
  }
});

test("same-origin redirects retain caller headers", async () => {
  const keys = [];
  const mode = { onRequest(request) {
    keys.push(request.headers["x-api-key"]);
    mode.redirect = request.url.startsWith("/object") ? "/final" : undefined;
  } };
  const server = await serve(new Uint8Array(16), mode);
  try {
    const session = await httpSource(server.url, { followRedirects: true, headers: { "x-api-key": "header-marker" } }).open();
    session.close();
    assert.deepEqual(keys, ["header-marker", "header-marker"]);
  } finally { await server.close(); }
});

test("caller headers are not restored when a redirect returns to the original origin", async () => {
  const originalHeaders = [];
  const otherHeaders = [];
  const targetMode = { onRequest: (request) => otherHeaders.push([request.headers["x-api-key"], request.headers.authorization]) };
  const target = await serve(new Uint8Array(16), targetMode);
  const originMode = { onRequest(request) {
    originalHeaders.push([request.headers["x-api-key"], request.headers.authorization]);
    originMode.redirect = request.url.startsWith("/object") ? target.url : undefined;
  } };
  const origin = await serve(new Uint8Array(16), originMode);
  targetMode.redirect = new URL("/returned", origin.url).href;
  try {
    const session = await httpSource(origin.url, { followRedirects: true, headers: { "x-api-key": "header-marker", authorization: "Bearer header-marker" } }).open();
    session.close();
    assert.deepEqual(originalHeaders, [["header-marker", "Bearer header-marker"], [undefined, undefined]]);
    assert.deepEqual(otherHeaders, [[undefined, undefined]]);
  } finally {
    await origin.close();
    await target.close();
  }
});

test("redirect loops stop after six platform requests without retrying", async () => {
  let requests = 0;
  const source = httpSource("https://media.invalid/file", { followRedirects: true, fetch: async () => {
    requests += 1;
    return new Response(null, { status: 302, headers: { location: "/again" } });
  } });
  await assert.rejects(source.open(), /too many/);
  assert.equal(requests, 6);
  assert.equal(source.lastStats, null);
});

for (const [status, headers] of [[300, { location: "/next" }], [304, {}], [302, {}]]) {
  test(`HTTP ${status} without a supported redirect target is refused`, async () => {
    let requests = 0;
    const source = httpSource("https://media.invalid/file", { followRedirects: true, fetch: async () => {
      requests += 1;
      return new Response(null, { status, headers });
    } });
    await assert.rejects(source.open(), (error) => {
      assert.equal(error.name, "ProtocolError");
      assert.match(error.message, /unsupported redirect|no Location/);
      return true;
    });
    assert.equal(requests, 1);
  });
}

for (const [kind, location] of [["userinfo", "https://user:credential-marker@other.invalid/file"], ["scheme", "ftp://other.invalid/file?signature=query-marker"], ["syntax", "http://["], ["downgrade", "http://other.invalid/file?signature=query-marker"]]) {
  test(`unsafe redirect ${kind} fails before a request and does not echo the target`, async () => {
    let requests = 0;
    const source = httpSource("https://media.invalid/file", { followRedirects: true, fetch: async () => {
      requests += 1;
      return new Response(null, { status: 302, headers: { location } });
    } });
    await assert.rejects(source.open(), (error) => {
      assert.equal(error.name, "ProtocolError");
      const expected = { userinfo: /credentials/, scheme: /http/, syntax: /invalid/, downgrade: /downgrade/ };
      assert.match(error.message, expected[kind]);
      assert.doesNotMatch(error.message, /credential-marker|query-marker|other\.invalid/);
      return true;
    });
    assert.equal(requests, 1);
  });
}

test("every redirect hop asks for identity and encoded final responses are refused", async () => {
  const encodings = [];
  const source = httpSource("https://media.invalid/file", { followRedirects: true, headers: { "accept-encoding": "gzip" }, fetch: async (url, options) => {
    encodings.push(options.headers.get("accept-encoding"));
    return encodings.length === 1 ? new Response(null, { status: 302, headers: { location: "/final" } })
      : new Response(new Uint8Array(8), { headers: { "content-length": "8", "content-encoding": "gzip" } });
  } });
  await assert.rejects(source.open(), /compressed/);
  assert.deepEqual(encodings, ["identity", "identity"]);
});

test("redirected range reads pin the final server's ETag", async () => {
  const bytes = new Uint8Array(128).fill(7);
  let etag = '"v1"';
  const pinned = [];
  const source = httpSource("https://origin.invalid/file", {
    firstSpan: 16, maxSkip: 0, cacheBytes: 0, parallel: 1, segmentBytes: 16, bufferBytes: 32,
    followRedirects: true,
    fetch: async (url, options) => {
      if (new URL(url).hostname === "origin.invalid") return new Response(null, { status: 302, headers: { location: "https://target.invalid/file" } });
      const validator = options.headers.get("if-match");
      pinned.push(validator);
      if (validator !== null && validator !== etag) return new Response(null, { status: 412 });
      const range = /^bytes=(\d+)-(\d+)$/.exec(options.headers.get("range"));
      const start = Number(range[1]);
      const end = Math.min(bytes.length, Number(range[2]) + 1);
      return new Response(bytes.slice(start, end), { status: 206, headers: { etag, "content-range": `bytes ${start}-${end - 1}/${bytes.length}` } });
    }
  });
  const session = await source.open();
  try {
    assert.deepEqual(await session.read(64, 8), bytes.slice(64, 72));
    etag = '"v2"';
    await assert.rejects(session.read(96, 8), SourceChangedError);
    assert.equal(pinned[0], null);
    assert.ok(pinned.slice(1).every((value) => value === '"v1"'));
  } finally { session.close(); }
  assert.equal(source.lastStats.requests, 3);
});

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
