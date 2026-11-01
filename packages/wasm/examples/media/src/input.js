export const UPLOAD_DEFAULT_BYTES = 8 * 1024 * 1024;
export const UPLOAD_CEILING_BYTES = 16 * 1024 * 1024;
export const READER_MEMORY_BYTES = 40 * 1024 * 1024;
export const OUTCOMES = new Set(["ok", "refused", "usage", "not_found", "source_unreadable", "rejected", "internal"]);

export class Refusal extends Error {
  constructor(status, message, headers = {}) {
    super(message);
    this.name = "Refusal";
    this.status = status;
    this.headers = headers;
  }
}

export const ROUTES = {
  probe: { params: ["scan"], options: (p) => ({ scanFragments: p.has("scan") || undefined }) },
  ffprobe: { params: [], options: () => ({}) },
  check: { params: [], options: () => ({}) },
  chapters: { params: [], options: () => ({}) },
  "extract-audio": { params: ["stream"], options: (p) => ({ stream: streamIndex(p, "stream") }) },
  "extract-subtitles": { params: ["stream"], options: (p) => ({ stream: streamIndex(p, "stream") }) },
  clip: { params: ["start", "end", "video", "audio"], options: (p) => ({
    start: p.get("start") ?? undefined, end: p.get("end") ?? undefined,
    video: streamIndex(p, "video"), audio: streamIndex(p, "audio")
  }) },
  remux: { params: ["streams", "dropUnsupported"], options: (p) => ({
    streams: streamList(p), dropUnsupported: p.has("dropUnsupported") || undefined
  }) },
  sanitize: { params: ["streams", "keep"], options: (p) => ({
    streams: streamList(p), keep: p.has("keep") ? p.get("keep").split(",") : undefined
  }) }
};

function integerSetting(value, fallback, ceiling) {
  if (value === undefined) return fallback;
  if (typeof value !== "string" || !/^[1-9][0-9]*$/.test(value)) throw new Refusal(500, "the Worker is misconfigured");
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number > ceiling) throw new Refusal(500, "the Worker is misconfigured");
  return number;
}

/** Parse deployment settings before authentication, body reads or source I/O. */
export function readConfig(env) {
  const uploadBytes = integerSetting(env.MAX_UPLOAD_BYTES, UPLOAD_DEFAULT_BYTES, UPLOAD_CEILING_BYTES);
  const maxRequests = integerSetting(env.MAX_SOURCE_REQUESTS, 1000, 1000);
  const allowedHosts = new Set();
  if (env.ALLOWED_HOSTS !== undefined) {
    if (typeof env.ALLOWED_HOSTS !== "string") throw new Refusal(500, "the Worker is misconfigured");
    for (const part of env.ALLOWED_HOSTS.split(",")) {
      const host = part.trim();
      if (!host) continue;
      if (/[\s\\/@?#]/.test(host) || !URL.canParse(`https://${host}`)) throw new Refusal(500, "the Worker is misconfigured");
      allowedHosts.add(new URL(`https://${host}`).host);
    }
  }
  if (env.DEV_ANONYMOUS !== undefined && env.DEV_ANONYMOUS !== "1") throw new Refusal(500, "the Worker is misconfigured");
  const policyText = env.POLICY_JSON ?? null;
  if (policyText !== null) {
    try { if (typeof policyText !== "string") throw new Error(); JSON.parse(policyText); }
    catch { throw new Refusal(500, "the Worker is misconfigured"); }
  }
  return { uploadBytes, maxRequests, allowedHosts, policyText, anonymous: env.DEV_ANONYMOUS === "1" };
}

/** Replace with application authentication; return the tenant's allowed R2 prefix. */
export function authorize(request, config) {
  if (config.anonymous) return "";
  throw new Refusal(401, "authentication required");
}

/** Bound each remote reader explicitly; this is not an isolate-wide limit. */
export function readerOptions(config) {
  return { memoryBytes: READER_MEMORY_BYTES, maxRequests: config.maxRequests };
}

export function objectKey(prefix, key) {
  if (typeof prefix !== "string" || typeof key !== "string" || !key || /[\x00-\x1f\x7f]/.test(prefix + key) || new TextEncoder().encode(prefix + key).length > 1024) {
    throw new Refusal(400, "the object key is invalid");
  }
  return prefix + key;
}

/** Accept only configured HTTPS hosts, without userinfo or fragments. */
export function remoteUrl(text, allowedHosts) {
  if (!allowedHosts.size) throw new Refusal(400, "URL input is not enabled");
  if (text.length > 2048) throw new Refusal(400, "the URL is too long");
  if (!URL.canParse(text)) throw new Refusal(400, "the URL is invalid");
  const url = new URL(text);
  if (url.protocol !== "https:") throw new Refusal(400, "the URL must use https");
  if (url.username || url.password) throw new Refusal(400, "the URL must not carry credentials");
  if (!allowedHosts.has(url.host)) throw new Refusal(400, "the URL host is not allowed");
  url.hash = "";
  return url;
}

export function streamIndex(params, name) {
  if (!params.has(name)) return undefined;
  const value = params.get(name);
  if (name === "audio" && value === "none") return "none";
  if (!/^[0-9]+$/.test(value) || !Number.isSafeInteger(Number(value))) throw new Refusal(400, "the stream index is invalid");
  return Number(value);
}

export function streamList(params) {
  if (!params.has("streams")) return undefined;
  return params.get("streams").split(",").map((part) => streamIndex(new URLSearchParams({ stream: part }), "stream"));
}

/** Count received bytes before copying them; unknown-length uploads briefly keep two copies. */
export async function readUpload(request, cap) {
  if (!Number.isSafeInteger(cap) || cap < 1 || cap > UPLOAD_CEILING_BYTES) throw new Refusal(500, "the Worker is misconfigured");
  const encoding = request.headers.get("content-encoding");
  if (encoding !== null && encoding !== "identity") throw new Refusal(415, "send the file without content encoding");
  const length = request.headers.get("content-length");
  let declared;
  if (length !== null) {
    if (!/^[0-9]+$/.test(length)) throw new Refusal(400, "the content length is invalid");
    declared = Number(length);
    if (declared > cap) {
      await request.body?.cancel();
      throw new Refusal(413, "the upload is larger than MAX_UPLOAD_BYTES");
    }
    if (declared === 0) throw new Refusal(400, "the upload is empty");
  }
  if (request.body === null) throw new Refusal(400, "the upload is empty");
  const buffer = declared === undefined ? null : new Uint8Array(declared);
  const chunks = [];
  const reader = request.body.getReader();
  let received = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      if (!(value instanceof Uint8Array)) throw new Refusal(400, "the upload is not a byte stream");
      const next = received + value.byteLength;
      if (next > cap) throw new Refusal(413, "the upload is larger than MAX_UPLOAD_BYTES");
      if (declared !== undefined && next > declared) throw new Refusal(400, "the body is longer than its content length");
      if (buffer) buffer.set(value, received);
      else chunks.push(value);
      received = next;
    }
    if (!received) throw new Refusal(400, "the upload is empty");
    if (declared !== undefined && received !== declared) throw new Refusal(400, "the body does not match its content length");
    if (buffer) return buffer;
    const bytes = new Uint8Array(received);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return bytes;
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  } finally {
    reader.releaseLock();
  }
}

/** Keep route, configuration, authentication and parameter checks ahead of body consumption. */
export async function parseRequest(request, env) {
  const url = new URL(request.url);
  const route = url.pathname.slice(1);
  if (!Object.hasOwn(ROUTES, route)) throw new Refusal(404, "unknown route");
  if (request.method !== "GET" && request.method !== "POST") throw new Refusal(405, "GET or POST only", { allow: "GET, POST" });
  const config = readConfig(env);
  const prefix = authorize(request, config);
  const params = url.searchParams;
  const allowed = new Set(["key", "url", ...ROUTES[route].params]);
  for (const name of params.keys()) {
    if (!allowed.has(name) || params.getAll(name).length !== 1) throw new Refusal(400, "unknown or duplicate query parameter");
  }
  const options = ROUTES[route].options(params);
  if ((request.method === "POST" && (params.has("key") || params.has("url"))) || (request.method === "GET" && params.has("key") === params.has("url"))) {
    throw new Refusal(400, "give a key, a URL, or POST the file");
  }
  const input = request.method === "POST" ? { kind: "upload", bytes: await readUpload(request, config.uploadBytes) }
    : params.has("key") ? { kind: "key", key: objectKey(prefix, params.get("key")) }
    : { kind: "url", url: remoteUrl(params.get("url"), config.allowedHosts) };
  return { route, options, input, config };
}

export function log(route, outcome) {
  console.log(JSON.stringify({ example: "media", route: Object.hasOwn(ROUTES, route) ? route : "unknown", outcome: OUTCOMES.has(outcome) ? outcome : "internal" }));
}
