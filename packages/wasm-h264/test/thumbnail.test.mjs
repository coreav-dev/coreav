import assert from "node:assert/strict";
import { test } from "node:test";
import { thumbnailRequest } from "../src/thumbnail-options.js";

test("thumbnail keeps signal and unknown options, and defaults width only when both dimensions are absent", () => {
  const signal = new AbortController().signal;
  const shaped = thumbnailRequest({ at: 1.5, signal, quality: 91, unknown: true });
  assert.equal(shaped.request.at, "1.5");
  assert.equal(shaped.request.width, 320);
  assert.equal(shaped.request.quality, 91);
  assert.equal(shaped.request.unknown, true);
  assert.equal(shaped.request.signal, signal);
  assert.equal(shaped.contentType, "image/jpeg");
  assert.equal(thumbnailRequest({ at: "2", height: 10 }).request.width, undefined);
  assert.equal(thumbnailRequest({ at: "2", width: 8 }).request.height, undefined);
  assert.deepEqual(
    thumbnailRequest({ at: "2", width: 8, height: 9, format: "png" }).request,
    { at: "2", width: 8, height: 9, format: "png" },
  );
  assert.equal(thumbnailRequest({ at: "2", format: "webp" }).contentType, "image/webp");
  assert.equal(thumbnailRequest({ at: "2", format: "jpg" }).contentType, "image/jpeg");
  assert.equal(thumbnailRequest({ at: "2", format: "nope" }).request.format, "nope");
  assert.throws(() => thumbnailRequest({}), /at must be decimal seconds/);
  assert.throws(() => thumbnailRequest({ at: -1 }), /at must be decimal seconds/);
  assert.throws(() => thumbnailRequest(null), /options must be an object/);
});
