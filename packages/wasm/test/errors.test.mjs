import assert from "node:assert/strict";

import { test } from "node:test";

import { CoreAVError, SourceNotFoundError, errorResponse } from "../src/errors.js";

test("source and usage errors map to distinct HTTP responses", async () => {
  const missing = errorResponse(new SourceNotFoundError());
  assert.equal(missing.status, 404);
  assert.deepEqual(await missing.json(), { error: "the source object does not exist" });
  const usage = errorResponse(new TypeError("invalid option"));
  assert.equal(usage.status, 400);
  assert.deepEqual(await usage.json(), { error: "invalid option" });
  assert.throws(() => errorResponse(new Error("unexpected")), /unexpected/);
});

test("rejection responses preserve the original JSON, including 64-bit offsets", async () => {
  for (const [kind, status] of [ [ "invalid_data", 422 ], [ "io", 502 ] ]) {
    const text = `{"schema":"coreav.error/1","kind":"${kind}","limit":null,"offset":9007199254740993,"message":"rejected"}`;
    const response = errorResponse(new CoreAVError(text));
    assert.equal(response.status, status);
    assert.equal(response.headers.get("content-type"), "application/json");
    assert.equal(await response.text(), text);
  }
});

test("errors created from JavaScript objects retain their document and cause", async () => {
  const document = {
    schema: "coreav.error/1",
    kind: "io",
    limit: null,
    offset: null,
    message: "read failed"
  };
  const cause = new Error("connection reset");
  const error = new CoreAVError(document, {
    cause: cause
  });
  assert.equal(error.cause, cause);
  assert.equal(error.document, document);
  assert.deepEqual(await errorResponse(error).json(), document);
});
