import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer } from "node:http";
import { once } from "node:events";
import {
  BackingBudget,
  BoundedRangeSession,
  boundedOpeners,
} from "../src/bounded-source.js";
import {
  adapterRangeSource,
  bytesSource,
  httpSource,
  rangeSource,
} from "../src/sources.js";
import { r2Source } from "../src/r2.js";

const MiB = 1024 * 1024;

function deferred() {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function object(size, { cancel, reading } = {}) {
  const trace = { requests: 0, reads: 0, active: 0, maxActive: 0 };
  return {
    trace,
    async fetch({ offset, length, validator }) {
      assert.ok(validator === undefined || validator === "version");
      trace.requests += 1;
      trace.active += 1;
      trace.maxActive = Math.max(trace.maxActive, trace.active);
      const end = Math.min(size, offset + length);
      let pos = offset;
      let settled = false;
      function finish() {
        if (settled) return;
        settled = true;
        trace.active -= 1;
      }
      return {
        offset,
        end,
        size,
        validator: "version",
        body: new ReadableStream({
          type: "bytes",
          async pull(controller) {
            trace.reads += 1;
            await reading;
            const request = controller.byobRequest;
            const count = Math.min(request.view.byteLength, end - pos);
            for (let i = 0; i < count; i += 1)
              request.view[i] = (pos + i) % 251;
            pos += count;
            request.respond(count);
            if (pos === end) {
              controller.close();
              finish();
            }
          },
          async cancel() {
            await cancel;
            finish();
          },
        }),
      };
    },
  };
}

function sourceFor(input, options) {
  return adapterRangeSource(input.fetch.bind(input), options);
}

test("abort reaches a bounded range request before opening settles", async () => {
  const entered = deferred();
  let requestSignal;
  const source = adapterRangeSource(({ signal }) => {
    requestSignal = signal;
    entered.resolve();
    return new Promise((_resolve, reject) => {
      signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")), { once: true });
    });
  });
  const aborts = new AbortController();
  const opening = boundedOpeners.get(source)({ signal: aborts.signal });
  await entered.promise;
  aborts.abort();
  assert.equal(requestSignal.aborted, true);
  await assert.rejects(opening, (error) => error.name === "AbortError");
  assert.equal(source.lastStats.requests, 1);
  assert.equal(source.lastStats.maxBackingBytes, 0);
});

test("bounded reads use owned BYOB buffers and one response", async () => {
  const input = object(8 * MiB);
  const source = sourceFor(input);
  const session = await boundedOpeners.get(source)();
  for (const [offset, length] of [
    [0, 300000],
    [300000, 200],
    [4 * MiB, 900000],
    [3, 127],
  ]) {
    const bytes = await session.read(offset, length);
    assert.equal(bytes.byteLength, length);
    for (let i = 0; i < length; i += 1)
      assert.equal(bytes[i], (offset + i) % 251);
  }
  await session.close();
  assert.equal(input.trace.maxActive, 1);
  assert.equal(input.trace.active, 0);
  assert.ok(source.lastStats.maxBackingBytes <= 4 * MiB + 256 * 1024);
  assert.ok(source.lastStats.maxChargedBytes <= 4 * MiB + 256 * 1024);
});

test("a giant read is refused before a response body is read", async () => {
  const input = object(8 * MiB);
  const session = await boundedOpeners.get(sourceFor(input))();
  await assert.rejects(
    session.read(0, 4 * MiB + 1),
    (error) => error.kind === "limit",
  );
  assert.equal(input.trace.reads, 0);
  assert.equal(input.trace.requests, 1);
  assert.equal(session.stats().maxChargedBytes, 0);
  await session.close();
});

test("a tiny memory view cannot hide a giant backing buffer", async () => {
  const input = bytesSource(new Uint8Array(new ArrayBuffer(4 * MiB + 1), 0, 1));
  assert.equal((await input.open()).size, 1);
  assert.throws(
    () => boundedOpeners.get(input)(),
    (error) => error.kind === "limit",
  );
  const maximum = bytesSource(new Uint8Array(4 * MiB));
  const session = await boundedOpeners.get(maximum)();
  assert.equal(session.stats().maxBackingBytes, 4 * MiB);
});

test("foreign range producers are not admitted by a memory option", () => {
  const input = rangeSource(
    () => {
      throw new Error("must not open");
    },
    { memoryBytes: 4 * MiB },
  );
  assert.equal(boundedOpeners.has(input), false);
});

test("an adapter without BYOB support is refused before reading chunks", async () => {
  let read = false;
  const source = adapterRangeSource(async () => ({
    offset: 0,
    size: 1,
    body: new ReadableStream(
      {
        pull(controller) {
          read = true;
          controller.enqueue(new Uint8Array(1));
        },
      },
      { highWaterMark: 0 },
    ),
  }));
  await assert.rejects(boundedOpeners.get(source)(), /byte stream|BYOB/i);
  assert.equal(read, false);
});

test("response replacement waits for cancellation before another request", async () => {
  const cancel = deferred();
  const input = object(8 * MiB, { cancel: cancel.promise });
  const session = await boundedOpeners.get(sourceFor(input))();
  const read = session.read(4 * MiB, 1);
  await Promise.resolve();
  assert.equal(input.trace.requests, 1);
  cancel.resolve();
  assert.equal((await read)[0], (4 * MiB) % 251);
  await session.close();
  assert.equal(input.trace.maxActive, 1);
});

test("closing retains ownership of a late range response until it is cancelled", async () => {
  const arriving = deferred();
  const entered = deferred();
  const input = object(100);
  const session = new BoundedRangeSession(
    async (args) => {
      entered.resolve();
      await arriving.promise;
      return input.fetch(args);
    },
    { maxRequests: 10 },
    () => {},
  );
  const started = session.start();
  const rejected = assert.rejects(started, /closed/);
  await entered.promise;
  const closed = session.close();
  arriving.resolve();
  await closed;
  await rejected;
  assert.equal(input.trace.active, 0);
  assert.equal(input.trace.requests, 1);
});

test("request caps and changed objects are refused without overlap", async () => {
  const input = object(8 * MiB);
  const session = await boundedOpeners.get(
    sourceFor(input, { maxRequests: 1 }),
  )();
  await assert.rejects(session.read(4 * MiB, 1), /maxRequests/);
  assert.equal(input.trace.requests, 1);
  await session.close();
  let calls = 0;
  const changed = adapterRangeSource(async (args) => {
    const response = await input.fetch(args);
    if (++calls > 1) response.validator = "changed";
    return response;
  });
  const other = await boundedOpeners.get(changed)();
  await assert.rejects(other.read(4 * MiB, 1), /changed/);
  await other.close();
  assert.equal(input.trace.active, 0);
});

test("backing charges reject growth before allocation and are released once", () => {
  const budget = new BackingBudget();
  const token = budget.allocate("reader", 4 * MiB);
  assert.throws(
    () => budget.allocate("reader", 1),
    (error) => error.kind === "limit",
  );
  assert.equal(budget.stats().maxBackingBytes, 4 * MiB);
  budget.release(token);
  budget.release(token);
  assert.equal(budget.used.reader, 0);
  const next = budget.allocate("reader", 4 * MiB);
  budget.release(next);
});

test("platform HTTP bodies support the bounded BYOB route", async () => {
  const server = createServer((request, response) => {
    assert.ok(
      request.headers["accept-encoding"]
        .split(",")
        .every((value) => value.trim() === "identity"),
    );
    response.writeHead(206, {
      "Content-Range": "bytes 0-31/32",
      "Content-Length": "32",
      ETag: '"version"',
    });
    response.end(Buffer.from(Array.from({ length: 32 }, (_, i) => i)));
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    const input = httpSource(`http://127.0.0.1:${server.address().port}/input`);
    const session = await boundedOpeners.get(input)();
    assert.deepEqual(
      await session.read(0, 32),
      Uint8Array.from({ length: 32 }, (_, i) => i),
    );
    await session.close();
    assert.ok(input.lastStats.maxBackingBytes <= 64);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});

test("custom HTTP producers cannot assert bounded production", () => {
  const input = httpSource("https://example.invalid/input", {
    fetch() {
      throw new Error("must not run");
    },
  });
  assert.equal(boundedOpeners.has(input), false);
});

test("cancellation failures retain host exceptions only as their cause", async () => {
  const source = adapterRangeSource(async () => ({
    offset: 0,
    size: 1,
    body: new ReadableStream({
      type: "bytes",
      cancel() {
        throw new Error("credential-marker");
      },
    }),
  }));
  const session = await boundedOpeners.get(source)();
  await assert.rejects(session.close(), (error) => {
    assert.equal(error.kind, "io");
    assert.ok(!error.documentText.includes("credential-marker"));
    assert.equal(error.cause.message, "credential-marker");
    return true;
  });
});

test("a foreign R2 reader cannot impersonate a platform BYOB stream", async () => {
  let invoked = false;
  const input = r2Source(
    {
      async get() {
        return {
          size: 1,
          etag: "version",
          body: {
            getReader() {
              invoked = true;
              return {
                read: async () => ({ value: new Uint8Array(1), done: false }),
              };
            },
            cancel() {
              invoked = true;
            },
          },
        };
      },
    },
    "input",
  );
  await assert.rejects(boundedOpeners.get(input)(), /native byte stream/);
  assert.equal(invoked, false);
});

test("own getReader overrides cannot replace a native stream reader", async () => {
  const input = object(32);
  let invoked = false;
  const source = adapterRangeSource(async (args) => {
    const response = await input.fetch(args);
    response.body.getReader = () => {
      invoked = true;
      throw new Error("foreign reader");
    };
    return response;
  });
  const session = await boundedOpeners.get(source)();
  assert.equal((await session.read(0, 1))[0], 0);
  await session.close();
  assert.equal(invoked, false);
});

test("resizable and growable memory cannot outgrow admission", () => {
  const buffer = new ArrayBuffer(1, { maxByteLength: 8 * MiB });
  Object.defineProperty(buffer, "resizable", { value: false });
  const input = bytesSource(new Uint8Array(buffer));
  assert.throws(
    () => boundedOpeners.get(input)(),
    (error) => error.kind === "limit",
  );
  const shared = bytesSource(
    new Uint8Array(new SharedArrayBuffer(1, { maxByteLength: 8 * MiB })),
  );
  assert.throws(
    () => boundedOpeners.get(shared)(),
    (error) => error.kind === "limit",
  );
});

test("an overridden buffer property cannot hide the backing capacity", () => {
  const bytes = new Uint8Array(new ArrayBuffer(4 * MiB + 1), 0, 1);
  Object.defineProperty(bytes, "buffer", { value: new ArrayBuffer(1) });
  const input = bytesSource(bytes);
  assert.throws(
    () => boundedOpeners.get(input)(),
    (error) => error.kind === "limit",
  );
});
