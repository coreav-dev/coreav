const MiB = 1024 * 1024;
const TOO_LARGE = Symbol("too-large");
export const INDEX_LIMITS = Object.freeze({ storedBytes: 8 * MiB, cacheBytes: 8 * MiB, cacheEntries: 16, builds: 1 });
export const READER_MEMORY_BYTES = 40 * MiB;
export const SCHEME = "coreav-hls-cache/1";
export const OUTCOMES = new Set(["ok", "refused", "usage", "not_found", "source_unreadable", "rejected", "internal", "index_too_large", "busy", "store_put_failed"]);

export class Refusal extends Error {
  constructor(status, message, outcome = "refused") {
    super(message);
    this.name = "Refusal";
    this.status = status;
    this.outcome = outcome;
    this.headers = status === 503 ? { "retry-after": "1" } : status === 405 ? { allow: "GET" } : {};
  }
}

export function log(outcome) {
  console.log(JSON.stringify({ example: "hls", outcome: OUTCOMES.has(outcome) ? outcome : "internal" }));
}

export function readerOptions(config) {
  return { memoryBytes: READER_MEMORY_BYTES, maxRequests: config.maxRequests };
}

/** Replace with application authentication; return the tenant's allowed R2 prefix. */
export function authorize(request, config) {
  if (config.anonymous) return "";
  throw new Refusal(401, "authentication required");
}

export function parsePath(request, env) {
  if (request.method !== "GET") throw new Refusal(405, "GET only");
  let path;
  try { path = decodeURIComponent(new URL(request.url).pathname.slice(1)); }
  catch { throw new Refusal(400, "the path is not valid percent-encoded text"); }
  const split = path.lastIndexOf("/");
  const name = path.slice(split + 1);
  if (split < 1 || !/^(master\.m3u8|(?:video|audio)(?:\.m3u8|-init\.mp4|-[0-9]+\.m4s))$/.test(name)) throw new Refusal(404, "not found");
  const segmentDuration = env.SEGMENT_DURATION ?? "6";
  const rawRequests = env.MAX_SOURCE_REQUESTS ?? "1000";
  if (typeof segmentDuration !== "string" || !/^(0|[1-9][0-9]*)(\.[0-9]{1,6})?$/.test(segmentDuration) || !(Number(segmentDuration) > 0) || !Number.isFinite(Number(segmentDuration)) || typeof rawRequests !== "string" || !/^[1-9][0-9]*$/.test(rawRequests) || Number(rawRequests) > 1000 || (env.DEV_ANONYMOUS !== undefined && env.DEV_ANONYMOUS !== "1")) {
    throw new Refusal(500, "the Worker is misconfigured");
  }
  const config = { indexOptions: { segmentDuration }, maxRequests: Number(rawRequests), anonymous: env.DEV_ANONYMOUS === "1" };
  const prefix = authorize(request, config);
  const key = prefix + path.slice(0, split);
  if (typeof prefix !== "string" || /[\x00-\x1f\x7f]/.test(key) || new TextEncoder().encode(key).length > 1024) throw new Refusal(400, "the object key is invalid");
  return { key, name, config };
}

/** Hash the complete identity so distinct builds coexist under bounded R2 key names. */
export async function indexIdentity({ key, etag, build, options }) {
  if (typeof build !== "string" || !/^[a-f0-9]{64}$/.test(build)) throw new Refusal(500, "the Worker is misconfigured");
  const metadata = { scheme: SCHEME, etag, build, options: JSON.stringify(options) };
  const id = JSON.stringify([SCHEME, key, etag, build, metadata.options]);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(id)));
  const hex = Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return { id, storeKey: `coreav-hls-index/${hex}`, metadata };
}

function busy() {
  return new Refusal(503, "the Worker is busy; try again", "busy");
}

function tooLarge() {
  return new Refusal(422, "the HLS index is larger than this Worker keeps", "index_too_large");
}

/** Bounded retention and stored-read reservations, with one loading owner per identity. */
export class IndexCache {
  constructor(limits = INDEX_LIMITS) {
    this.limits = { ...INDEX_LIMITS, ...limits };
    if (!Object.values(this.limits).every((value) => Number.isSafeInteger(value) && value > 0)) throw new TypeError("cache limits must be positive safe integers");
    this.ready = new Map();
    this.loading = new Map();
    this.readyBytes = 0;
    this.reservedBytes = 0;
    this.building = 0;
  }

  stats() {
    return { entries: this.ready.size + this.loading.size, readyBytes: this.readyBytes, reservedBytes: this.reservedBytes, loading: this.loading.size, building: this.building };
  }

  drop(id) {
    const record = this.ready.get(id);
    if (!this.ready.delete(id)) return;
    if (record !== TOO_LARGE) this.readyBytes -= record.bytes.byteLength;
  }

  /** A replacement accepts only newly built bytes, not a new copy of a damaged store record. */
  async get(identity, options) {
    for (let restart = 0; restart < 3; restart += 1) {
      const found = this.ready.get(identity.id);
      if (found !== undefined) {
        if (found === TOO_LARGE) throw tooLarge();
        if (options.replace === undefined || (found.built && found.bytes !== options.replace)) {
          this.ready.delete(identity.id);
          this.ready.set(identity.id, found);
          return found.bytes;
        }
        this.drop(identity.id);
      }
      const running = this.loading.get(identity.id);
      if (running) {
        if (options.replace === undefined || running.fresh) return running.promise;
        const produced = await running.promise.catch(() => undefined);
        if (running.built && produced !== undefined && produced !== options.replace) return produced;
        continue;
      }
      while (this.ready.size + this.loading.size >= this.limits.cacheEntries && this.ready.size) this.drop(this.ready.keys().next().value);
      if (this.ready.size + this.loading.size >= this.limits.cacheEntries) throw busy();
      const entry = { fresh: options.replace !== undefined, built: false, reserved: 0, promise: null };
      entry.promise = Promise.resolve().then(() => this.load(identity, options, entry)).finally(() => {
        this.release(entry);
        if (this.loading.get(identity.id) === entry) this.loading.delete(identity.id);
      });
      this.loading.set(identity.id, entry);
      return entry.promise;
    }
    throw busy();
  }

  release(entry) {
    this.reservedBytes -= entry.reserved;
    entry.reserved = 0;
  }

  reserve(bytes, entry) {
    while (this.readyBytes + this.reservedBytes + bytes > this.limits.cacheBytes && this.ready.size) this.drop(this.ready.keys().next().value);
    if (this.readyBytes + this.reservedBytes + bytes > this.limits.cacheBytes) throw busy();
    entry.reserved = bytes;
    this.reservedBytes += bytes;
  }

  commit(identity, bytes, entry) {
    this.release(entry);
    if (this.loading.get(identity.id) === entry) this.loading.delete(identity.id);
    const size = bytes === TOO_LARGE ? 0 : bytes.byteLength;
    while (this.readyBytes + this.reservedBytes + size > this.limits.cacheBytes && this.ready.size) this.drop(this.ready.keys().next().value);
    if (this.readyBytes + this.reservedBytes + size > this.limits.cacheBytes) return;
    this.ready.set(identity.id, bytes === TOO_LARGE ? TOO_LARGE : { bytes, built: entry.built });
    this.readyBytes += size;
  }

  async load(identity, { stored, buildIndex, waitUntil }, entry) {
    if (!entry.fresh && stored) {
      const object = await stored.get(identity.storeKey);
      if (object) {
        const matches = Object.entries(identity.metadata).every(([name, value]) => object.customMetadata?.[name] === value);
        const state = object.customMetadata?.state;
        if (matches && state === "too-large") {
          await object.body?.cancel();
          this.commit(identity, TOO_LARGE, entry);
          throw tooLarge();
        }
        if (matches && state === "ready" && Number.isSafeInteger(object.size) && object.size >= 14 && object.size <= this.limits.storedBytes && object.body) {
          try { this.reserve(object.size, entry); }
          catch (error) { await object.body.cancel(); throw error; }
          const bytes = new Uint8Array(object.size);
          const reader = object.body.getReader();
          let received = 0;
          let damaged = false;
          try {
            while (true) {
              const { value, done } = await reader.read();
              if (done) break;
              if (!(value instanceof Uint8Array) || value.byteLength > bytes.byteLength - received) { damaged = true; break; }
              bytes.set(value, received);
              received += value.byteLength;
            }
            if (received !== bytes.byteLength) damaged = true;
          } catch { damaged = true; }
          finally {
            if (damaged) await reader.cancel().catch(() => {});
            reader.releaseLock();
          }
          if (!damaged) { this.commit(identity, bytes, entry); return bytes; }
          this.release(entry);
        } else {
          await object.body?.cancel();
        }
      }
    }
    if (this.building >= this.limits.builds) throw busy();
    this.building += 1;
    let index;
    try { index = await buildIndex(); entry.built = true; }
    finally { this.building -= 1; }
    if (!(index instanceof Uint8Array)) throw new TypeError("the index builder must return bytes");
    const large = index.byteLength > this.limits.storedBytes;
    if (stored) {
      const write = Promise.resolve().then(() => stored.put(identity.storeKey, large ? new Uint8Array() : index, {
        customMetadata: { ...identity.metadata, state: large ? "too-large" : "ready" }
      })).catch(() => log("store_put_failed"));
      waitUntil(write);
    }
    this.commit(identity, large ? TOO_LARGE : index, entry);
    if (large) throw tooLarge();
    return index;
  }
}

export const cache = new IndexCache();
