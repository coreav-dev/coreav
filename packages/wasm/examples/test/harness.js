import example from "EXAMPLE";
import { moduleInfo } from "@coreav/wasm/workers";
import { cache } from "../hls/src/indexes.js";
import { FakeBucket } from "./fake-r2.mjs";

const pending = new Set();

function stored(bucket) {
  return {
    async get(key) {
      const response = await bucket.fetch(`https://fixtures.test/__indexes/${encodeURIComponent(key)}`);
      if (response.status === 404) return null;
      if (!response.ok) throw new Error("the fixture store failed");
      return { size: Number(response.headers.get("x-size")), customMetadata: JSON.parse(response.headers.get("x-metadata")), body: response.body };
    },
    async put(key, bytes, options) {
      const response = await bucket.fetch(`https://fixtures.test/__indexes/${encodeURIComponent(key)}`, { method: "PUT", body: bytes, headers: { "x-metadata": JSON.stringify(options.customMetadata) } });
      if (!response.ok) throw new Error("the fixture store failed");
    }
  };
}

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    if (url.pathname === "/__harness/memory") {
      await Promise.all([...pending]);
      return Response.json({ ...moduleInfo(), cache: cache.stats() });
    }
    const media = new FakeBucket(new Map(), { load: async (key) => {
      const response = await env.FIXTURES.fetch(`https://fixtures.test/fixtures/${encodeURIComponent(key)}`);
      if (response.status === 404) return null;
      if (!response.ok) throw new Error("the fixture source failed");
      return { bytes: new Uint8Array(await response.arrayBuffer()), etag: response.headers.get("etag").replace(/^"|"$/g, "") };
    } });
    const bindings = { ...env, INDEXES: env.NO_INDEXES === "1" ? undefined : stored(env.FIXTURES) };
    Object.defineProperty(bindings, "MEDIA", { get() {
      if (decodeURIComponent(url.pathname).includes("__throw__") || url.searchParams.get("key")?.includes("__throw__")) {
        const error = new TypeError("SECRETKEY SECRETSIG SECRETTOK");
        error.name = "SECRETPW";
        throw error;
      }
      return media;
    } });
    return example.fetch(request, bindings, { waitUntil(promise) {
      pending.add(promise);
      const tracked = promise.finally(() => pending.delete(promise));
      ctx.waitUntil(tracked);
    } });
  }
};
