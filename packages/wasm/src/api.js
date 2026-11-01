import { runToStream, runToText } from "./engine.js";

import { bytesSource } from "./sources.js";

export async function probe(source, options = {}) {
  return JSON.parse(await probeText(source, options));
}

export async function ffprobe(source, options = {}) {
  return JSON.parse(await ffprobeText(source, options));
}

export async function ffprobeText(source, options = {}) {
  return runToText("ffprobe", source, plain(options));
}

export async function probeText(source, options = {}) {
  return runToText("probe", source, plain(options));
}

export async function extractAudio(source, options = {}) {
  return output("extractAudio", source, plain(options), "audio/mp4");
}

export async function clip(source, options = {}) {
  const {start: start, end: end, ...rest} = plain(options);
  return output("clip", source, {
    ...rest,
    start: seconds(start, "start"),
    end: seconds(end, "end")
  }, "video/mp4");
}

export async function hlsIndex(source, options = {}) {
  const {segmentDuration: segmentDuration, ...rest} = plain(options);
  const {body: body, summary: summary} = await runToStream("hlsIndex", source, {
    ...rest,
    segmentDuration: seconds(segmentDuration, "segmentDuration")
  });
  const index = new Uint8Array(await new Response(body).arrayBuffer());
  return {
    index: index,
    summary: await summary
  };
}

export async function hlsGet(index, name, source, options = {}) {
  const bytes = indexBytes(index);
  if (typeof name !== "string") throw new TypeError("name must be a resource name, such as master.m3u8");
  const segment = name.endsWith(".m4s");
  if (segment && (source === undefined || source === null)) {
    throw new TypeError("a media segment needs the source the index was built from");
  }
  const {body: body, summary: summary} = await runToStream("hlsGet", segment ? source : NO_SOURCE, {
    ...plain(options),
    name: name
  }, bytes);
  return {
    body: body,
    summary: summary,
    contentType: hlsContentType(name)
  };
}

const NO_SOURCE = bytesSource(new Uint8Array(0));

function indexBytes(index) {
  if (index instanceof Uint8Array) return index;
  if (index instanceof ArrayBuffer) return new Uint8Array(index);
  if (ArrayBuffer.isView(index)) return new Uint8Array(index.buffer, index.byteOffset, index.byteLength);
  throw new TypeError("index must be the bytes hlsIndex returned (a Uint8Array or an ArrayBuffer)");
}

function hlsContentType(name) {
  if (name.endsWith(".m3u8")) return "application/vnd.apple.mpegurl";
  return name.startsWith("audio") ? "audio/mp4" : "video/mp4";
}

export async function check(source, options = {}) {
  const {policy: policy, ...rest} = plain(options);
  if (policy === undefined || policy === null) throw new TypeError("check needs a policy");
  return JSON.parse(await runToText("check", source, {
    ...rest,
    policy: policy
  }));
}

export async function chapters(source, options = {}) {
  return JSON.parse(await runToText("chapters", source, plain(options)));
}

export async function extractSubtitles(source, options = {}) {
  return output("extractSubtitles", source, plain(options), "text/vtt; charset=utf-8");
}

export async function remux(source, options = {}) {
  return output("remux", source, plain(options), "video/mp4", WRITER_SESSION);
}

export async function sanitize(source, options = {}) {
  return output("sanitize", source, plain(options), "video/mp4", WRITER_SESSION);
}

const WRITER_SESSION = Object.freeze({
  memoryBytes: 40 * 1024 * 1024
});

async function output(operation, source, options, contentType, open = undefined) {
  const {body: body, summary: summary} = await runToStream(operation, source, options, null, open);
  return {
    body: body,
    summary: summary,
    contentType: contentType
  };
}

function plain(options) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("options must be an object");
  }
  return options;
}

function seconds(value, name) {
  if (value === undefined || value === null) return undefined;
  if (typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value) && value >= 0) {
    const text = String(value);
    if (!/e/i.test(text)) return text;
  }
  throw new TypeError(`${name} must be decimal seconds, such as 12 or "12.5"`);
}
