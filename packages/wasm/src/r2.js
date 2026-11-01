import { SourceChangedError, SourceNotFoundError } from "./errors.js";

import { rangeSource } from "./sources.js";

const MiB = 1024 * 1024;

export function r2Source(bucket, key, options = {}) {
  if (typeof bucket?.get !== "function") throw new TypeError("bucket must be an R2 bucket binding");
  if (typeof key !== "string") throw new TypeError("key must be a string");
  return rangeSource(async ({offset: offset, length: length, validator: validator}) => {
    const object = await bucket.get(key, {
      range: {
        offset: offset,
        length: length
      },
      ...validator === undefined ? {} : {
        onlyIf: {
          etagMatches: validator
        }
      }
    });
    if (object === null) throw new SourceNotFoundError;
    if (!("body" in object) || !object.body) throw new SourceChangedError;
    return {
      body: object.body,
      offset: object.range?.offset ?? offset,
      size: object.size,
      validator: object.etag
    };
  }, {
    ...R2_DEFAULTS,
    ...options
  });
}

export const R2_DEFAULTS = Object.freeze({
  parallel: 6,
  segmentBytes: 8 * MiB,
  bufferBytes: 48 * MiB
});
