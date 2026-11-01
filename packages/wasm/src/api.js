import { runToStream, runToText } from "./engine.js";

import { bytesSource } from "./sources.js";

/** The probe document. Integers above 2^53 (Matroska's `track_uid`) come back rounded; `probeText` keeps them exact. */
export async function probe(source, options = {}) {
  return JSON.parse(await probeText(source, options));
}

/** The fixed ffprobe document parsed as JavaScript values. ffprobeText preserves integers above 2^53 exactly. */
export async function ffprobe(source, options = {}) {
  return JSON.parse(await ffprobeText(source, options));
}

/** Fixed ffprobe JSON text, unchanged. Options are filename (default "-") and limits; never use a signed URL as the filename. */
export async function ffprobeText(source, options = {}) {
  return runToText("ffprobe", source, plain(options));
}

/** The probe document as the command's JSON text, every integer exact. */
export async function probeText(source, options = {}) {
  return runToText("probe", source, plain(options));
}

/**
 * Extracts one AAC track to M4A by stream copy. Options:
 * `stream` (the stream index; the first audio stream by default), `allow`,
 * `strict`, `limits`.
 */
export async function extractAudio(source, options = {}) {
  return output("extractAudio", source, plain(options), "audio/mp4");
}

/**
 * Cuts a keyframe-aligned clip to MP4 by stream copy, as
 * `coreav clip`. Options: `start` (required) and `end` in seconds, as a
 * decimal string (`"12.5"`) or a number; `video` (stream index); `audio`
 * (stream index or `"none"`); `allow`, `strict`, `limits`. The summary is
 * the `coreav.clip/1` document.
 */
export async function clip(source, options = {}) {
  const {start: start, end: end, ...rest} = plain(options);
  return output("clip", source, {
    ...rest,
    start: seconds(start, "start"),
    end: seconds(end, "end")
  }, "video/mp4");
}

/** The HLS index of `source` and its `coreav.hls-index/2` summary. */
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

/**
 * One HLS resource (`master.m3u8`, `video.m3u8`, `video-init.mp4`, `video-N.m4s`, or the same for
 * `audio`) cut from an index; `source` is read for media segments only.
 */
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

/** What playlists and init segments read instead of a source: nothing. */
const NO_SOURCE = bytesSource(new Uint8Array(0));

function indexBytes(index) {
  if (index instanceof Uint8Array) return index;
  if (index instanceof ArrayBuffer) return new Uint8Array(index);
  if (ArrayBuffer.isView(index)) return new Uint8Array(index.buffer, index.byteOffset, index.byteLength);
  throw new TypeError("index must be the bytes hlsIndex returned (a Uint8Array or an ArrayBuffer)");
}

/** The media type of an HLS resource, as `coreav hls` serves it. */
function hlsContentType(name) {
  if (name.endsWith(".m3u8")) return "application/vnd.apple.mpegurl";
  return name.startsWith("audio") ? "audio/mp4" : "video/mp4";
}

/**
 * Checks an input against an upload policy, as `coreav check`: the
 * `coreav.verdict/1` verdict (`docs/schema/verdict-v1.md`). `policy` is a
 * `coreav.policy/1` policy (`docs/schema/policy-v1.md`), as an object or
 * JSON text (read as `coreav check` reads a policy file); `limits` as for
 * `probe`. A file the probe rejects gets a reject verdict; only a source
 * that cannot be read throws.
 */
export async function check(source, options = {}) {
  const {policy: policy, ...rest} = plain(options);
  if (policy === undefined || policy === null) throw new TypeError("check needs a policy");
  return JSON.parse(await runToText("check", source, {
    ...rest,
    policy: policy
  }));
}

/**
 * The chapter list of an MP4/MOV input (QuickTime chapter track or Nero
 * `chpl`): the `coreav.chapters/1` document, as `coreav chapters`.
 * Options: `limits`.
 */
export async function chapters(source, options = {}) {
  return JSON.parse(await runToText("chapters", source, plain(options)));
}

/**
 * Extracts a 3GPP timed text track to WebVTT, as `coreav
 * extract-subtitles`. Options: `stream` (the stream index; the first text
 * track that is not a chapter track by default), `limits`. The summary is
 * `coreav.extract-subtitles/1`.
 */
export async function extractSubtitles(source, options = {}) {
  return output("extractSubtitles", source, plain(options), "text/vtt; charset=utf-8");
}

/** Opens the source with `memoryBytes` of 40 MiB: it holds the output's sample tables itself. */
export async function remux(source, options = {}) {
  return output("remux", source, plain(options), "video/mp4", WRITER_SESSION);
}

/** As `remux`, with the same 40 MiB session bound. */
export async function sanitize(source, options = {}) {
  return output("sanitize", source, plain(options), "video/mp4", WRITER_SESSION);
}

/** Reader bound for writers that also retain output sample tables. Not an isolate-wide memory limit. */
const WRITER_SESSION = Object.freeze({
  memoryBytes: 40 * 1024 * 1024
});

/**
 * The result of a primitive that writes a file: `body` streams the output,
 * `summary` resolves to its summary document after the last byte, and
 * `contentType` is the output's media type. `open` is passed to the
 * source's `open()`.
 */
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

/** A time for the module: decimal seconds as text, exactly as written. */
function seconds(value, name) {
  if (value === undefined || value === null) return undefined;
  if (typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value) && value >= 0) {
    const text = String(value);
    if (!/e/i.test(text)) return text;
  }
  throw new TypeError(`${name} must be decimal seconds, such as 12 or "12.5"`);
}
