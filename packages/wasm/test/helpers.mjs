import { rangeSource } from "../src/sources.js";

export function random(seed) {
  let x = seed >>> 0 || 1;
  return () => {
    x ^= x << 13;
    x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5;
    x >>>= 0;
    return x / 4294967296;
  };
}

export function fakeFetchRange(bytes, {seed: seed = 1, log: log = [], hooks: hooks = {}} = {}) {
  const next = random(seed);
  let count = 0;
  return async ({offset: offset, length: length, validator: validator}) => {
    count += 1;
    const etag = hooks.validator?.() ?? "v1";
    log.push({
      offset: offset,
      length: length,
      validator: validator
    });
    if (validator !== undefined && validator !== etag) {
      const {SourceChangedError: SourceChangedError} = await import("../src/errors.js");
      throw new SourceChangedError;
    }
    const end = Math.min(offset + length, bytes.length);
    const failAfter = hooks.fail?.(count);
    let at = offset;
    const body = new ReadableStream({
      pull(controller) {
        if (failAfter !== undefined && at - offset >= failAfter) {
          controller.error(new Error("connection reset"));
          return;
        }
        if (at >= end) {
          controller.close();
          return;
        }
        const n = Math.min(end - at, 1 + Math.floor(next() * 4e4));
        controller.enqueue(bytes.slice(at, at + n));
        at += n;
      }
    });
    return {
      body: body,
      offset: offset,
      size: bytes.length,
      validator: etag
    };
  };
}

export function fakeRangeSource(bytes, options = {}, fake = {}) {
  return rangeSource(fakeFetchRange(bytes, fake), options);
}

export async function collect(stream) {
  return new Uint8Array(await new Response(stream).arrayBuffer());
}
