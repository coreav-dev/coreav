/**
 * In-memory R2 binding for Node tests and the workerd harness, without Node
 * imports. Values are Uint8Arrays or { bytes, etag?, size?, customMetadata? }.
 * A size override deliberately permits damaged stored-record tests.
 */
export class FakeBucket {
  /**
   * `load(key)` optionally supplies bytes or a record on a cache miss.
   * `calls` records ordered { method, key, options } calls; bytesPulled and
   * cancels count body consumption, not object creation or metadata reads.
   */
  constructor(objects = new Map(), { load, chunkSize = 65536 } = {}) {
    if (!Number.isSafeInteger(chunkSize) || chunkSize < 1) throw new TypeError("invalid chunkSize");
    this.objects = new Map(objects);
    this.load = load;
    this.chunkSize = chunkSize;
    this.calls = [];
    this.bytesPulled = 0;
    this.cancels = 0;
    this.revision = 0;
    this.deleted = new Set();
  }

  async record(key) {
    let record = this.objects.get(key);
    if (record === undefined && this.load && !this.deleted.has(key)) {
      record = await this.load(key);
      if (record != null) this.objects.set(key, record);
    }
    if (record == null) return null;
    if (record instanceof Uint8Array) record = { bytes: record };
    if (!(record.bytes instanceof Uint8Array)) throw new TypeError("fake object needs bytes");
    record.etag ??= `fake-${++this.revision}`;
    record.size ??= record.bytes.length;
    record.customMetadata ??= {};
    this.objects.set(key, record);
    return record;
  }

  metadata(key, record) {
    return {
      key, etag: record.etag, httpEtag: `"${record.etag}"`, size: record.size,
      customMetadata: { ...record.customMetadata },
    };
  }

  async head(key) {
    this.calls.push({ method: "head", key });
    const record = await this.record(key);
    return record === null ? null : this.metadata(key, record);
  }

  async get(key, options = {}) {
    this.calls.push({ method: "get", key, options: structuredClone(options) });
    const record = await this.record(key);
    if (record === null) return null;
    const object = this.metadata(key, record);
    if (options.onlyIf?.etagMatches !== undefined && options.onlyIf.etagMatches !== record.etag) return object;
    let offset = 0;
    let length = record.bytes.length;
    if (options.range) {
      const range = options.range;
      if (range.suffix !== undefined) {
        if (!Number.isSafeInteger(range.suffix) || range.suffix <= 0) throw new RangeError("invalid suffix");
        offset = Math.max(0, record.size - range.suffix);
        length = record.size - offset;
      } else {
        offset = range.offset ?? 0;
        length = range.length ?? record.size - offset;
        if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(length) || length <= 0) {
          throw new RangeError("invalid range");
        }
        if (offset >= record.size) throw new RangeError("unsatisfiable range");
        length = Math.min(length, record.size - offset);
      }
      object.range = { offset, length };
    }
    const bytes = record.bytes.subarray(offset, offset + length);
    let at = 0;
    object.body = new ReadableStream({
      pull: (controller) => {
        if (at === bytes.length) {
          controller.close();
          return;
        }
        const chunk = bytes.slice(at, at + this.chunkSize);
        at += chunk.length;
        this.bytesPulled += chunk.length;
        controller.enqueue(chunk);
      },
      cancel: () => { this.cancels += 1; },
    }, { highWaterMark: 0 });
    const forbidden = () => { throw new Error("read the fake R2 body as a stream"); };
    object.arrayBuffer = forbidden;
    object.text = forbidden;
    object.json = forbidden;
    return object;
  }

  async put(key, value, options = {}) {
    this.calls.push({ method: "put", key, options: structuredClone(options) });
    let bytes;
    if (value instanceof Uint8Array) bytes = value.slice();
    else if (value instanceof ArrayBuffer) bytes = new Uint8Array(value.slice(0));
    else if (typeof value === "string") bytes = new TextEncoder().encode(value);
    else if (value instanceof ReadableStream) bytes = new Uint8Array(await new Response(value).arrayBuffer());
    else throw new TypeError("unsupported fake R2 put value");
    let etag = `fake-${++this.revision}`;
    if (etag === this.objects.get(key)?.etag) etag = `fake-${++this.revision}`;
    const record = {
      bytes, size: bytes.length, etag,
      customMetadata: { ...options.customMetadata },
    };
    this.objects.set(key, record);
    this.deleted.delete(key);
    return this.metadata(key, record);
  }

  async delete(key) {
    this.calls.push({ method: "delete", key: Array.isArray(key) ? [...key] : key });
    for (const name of Array.isArray(key) ? key : [key]) {
      this.objects.delete(name);
      this.deleted.add(name);
    }
  }
}
