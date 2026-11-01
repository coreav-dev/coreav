/** A `coreav.error/1` object, as `coreav` prints it. */
export interface ErrorDocument {
  schema: "coreav.error/1";
  kind:
    | "invalid_structure"
    | "truncated_structure"
    | "limit"
    | "unsupported_container"
    | "unsupported_feature"
    | "codec_not_allowed"
    | "invalid_index"
    | "source_mismatch"
    | "io";
  limit: string | null;
  offset: number | null;
  message: string;
}

/** Overrides of single fields of the library's limits. */
export interface Limits {
  maxTopLevelBoxes?: number;
  maxDepth?: number;
  maxTotalBoxes?: number;
  maxBoxBytes?: number;
  maxRetainedBytes?: number;
  maxTableBytes?: number;
  maxPacketBytes?: number;
  maxSampleWork?: number;
  maxFragments?: number;
  maxTracks?: number;

  /** Bytes a compressed movie box (QuickTime `cmov`) may inflate to. */
  maxMovieBytes?: number;

  /** Inflated bytes per compressed byte of a compressed movie box. */
  maxMovieRatio?: number;
}

interface CommonOptions {

  /** Codec names to accept, as `--allow`. */
  allow?: string[];

  /** Reject the whole file when a stream's codec is not allowed. */
  strict?: boolean;
  limits?: Limits;
}

export interface ProbeOptions extends CommonOptions {
  scanFragments?: boolean;
  scanFrameSizes?: boolean;

}

export interface FfprobeOptions {
  /** Display label only; defaults to "-". Never pass a signed URL. */
  filename?: string;
  /** Overrides apply to every supported container. */
  limits?: Limits;
}

export interface FfprobeDocument {
  streams: Array<Record<string, unknown>>;
  format: Record<string, unknown>;
}

export interface ExtractAudioOptions extends CommonOptions {

  /** The stream index of the audio track; the first audio track by default. */
  stream?: number;
}

export interface ClipOptions extends CommonOptions {

  /** Decimal seconds, such as `"12.5"` or `12.5`. */
  start: string | number;
  end?: string | number;
  video?: number;
  audio?: number | "none";
}

export interface HlsIndexOptions extends CommonOptions {

  /** Target segment duration in decimal seconds; 6 by default. */
  segmentDuration?: string | number;

  /** The video stream index, or `"none"` for audio alone; the first video stream by default. */
  video?: number | "none";

  /** The audio stream index, or `"none"`; the first audio stream by default. */
  audio?: number | "none";

  /** Text kept in the index to identify the source's version, such as its ETag; at most 255 bytes. */
  tag?: string;
}

export interface HlsGetOptions {
  limits?: Limits;
}

export interface CheckOptions {

  /** A `coreav.policy/1` policy, as an object or JSON text (read exactly, as `coreav check` reads a policy file). */
  policy: Record<string, unknown> | string;

  /** Limits for MP4 and MOV inputs, as for `probe`. */
  limits?: Limits;
}

export interface SubtitleOptions {

  /** The stream index; the first text track that is not a chapter track by default. */
  stream?: number;
  limits?: Limits;
}

export interface ChapterOptions {
  limits?: Limits;
}

export interface RemuxOptions extends CommonOptions {

  /** Stream indexes to keep. */
  streams?: number[];
  failOnUnsupported?: boolean;

  /** Matroska and WebM: also drop a video stream MP4 cannot carry. */
  dropUnsupported?: boolean;
}

export interface SanitizeOptions extends RemuxOptions {
  keep?: Array<"rotation" | "language" | "dates" | "gapless" | "kinds" | "none">;
}

/** A file being written: return `body` as the response. */
export interface Output<Summary> {
  body: ReadableStream<Uint8Array>;

  /** Resolves after the last byte; rejects if the operation fails. */
  summary: Promise<Summary>;
  contentType: string;
}

/** One operation's view of a source (see `rangeSource` for remote ones). */
export type Session =
  | {
      size: number;
      inMemory: true;

      /** Copies `view.length` bytes at `offset`; returns the count. */
      readInto(offset: number, view: Uint8Array): number;
      close?(): void;
    }
  | {
      size: number;
      inMemory?: false;
      read(offset: number, length: number): Promise<Uint8Array>;
      readNow?(offset: number, view: Uint8Array): number;
      plan?(offset: number, length: number): void;
      planRegions?(regions: Float64Array | number[]): void;
      close?(): void;
    };

export interface Source {

  /**
     * `memoryBytes` lowers the bytes a session may hold (the range sources'
     * `memoryBytes` option), for an operation that needs much memory of its
     * own; a source may ignore it.
     */
  open(options?: { memoryBytes?: number }): Session | Promise<Session>;
}

export interface RangeOptions {
  firstSpan?: number;
  maxSkip?: number;
  cacheBytes?: number;
  parallel?: number;
  segmentBytes?: number;
  bufferBytes?: number;
  maxRequests?: number;
  retries?: number;

  /** Bytes, or "auto" (the default): from the first response's wait and `bandwidth`. */
  sparseGap?: number | "auto";

  /** Bytes per second assumed for the requests in flight, for `sparseGap: "auto"`. */
  bandwidth?: number;

  /**
     * Bounds every byte the reader holds (ahead of the reader, behind it and
     * the cache); 0, the default, leaves their sum as the bound. 0 or at
     * least 4 MiB.
     */
  memoryBytes?: number;
}

export interface ReadStats {
  requests: number;
  pipelines: number;
  bytesRequested: number;
  bytesReceived: number;

  /** The most bytes the reader held at once. */
  maxHeldBytes: number;
}

export interface RangeSource extends Source {

  /** The read statistics of the latest session, once it is closed. */
  lastStats: ReadStats | null;
}

export interface FetchRangeRequest {
  offset: number;
  length: number;

  /** Aborts when the session closes. */
  signal: AbortSignal;

  /** The ETag the session pinned; the request must fail if it changed. */
  validator?: string;
}

export interface FetchRangeResponse {
  body: ReadableStream<Uint8Array>;

  /** The first byte the body holds; must equal the requested offset. */
  offset: number;

  /** Just past the last byte the body holds, if less than asked. */
  end?: number;

  /** The object's size. */
  size: number;
  validator?: string;

  /** The body is the whole object from byte 0 (a server that ignores ranges). */
  whole?: boolean;
}

/**
 * Defaults of the range reader (docs/design/wasm.md, "Sources").
 *
 * - A read at a new position opens a response of `firstSpan` bytes, read as
 *   a stream: a span bounds what a request may transfer, not what is held.
 *   A read up to `maxSkip` bytes ahead of an open response is served by
 *   reading through the gap rather than by a new request.
 * - Reading on past the end of a response, or the module's advice that a
 *   copy pass follows, starts a pipeline: `parallel` range requests of
 *   `segmentBytes` each, started early so that their round trips are paid
 *   before their bytes are needed, and handed out in order. Each is read
 *   into memory at most `bufferBytes / parallel` ahead of the reader; the
 *   rest waits in its connection. Segments grow for objects that would
 *   otherwise need more than `maxRequests` requests.
 * - The last `cacheBytes` received outside the pipeline are kept and serve
 *   reads that go back, such as the passes over the sample tables.
 * - `memoryBytes`, when not 0, bounds all the bytes the reader holds: those
 *   ahead of the reader, the 8 MiB kept behind each pipeline's reader and
 *   the cache. Where they add up to more, they are lowered to fit (`open`
 *   takes a lower bound for one session, for operations that need more
 *   memory of their own). 0 leaves their sum as the bound. Besides it, each
 *   open response holds the piece it is reading, at most 256 KiB.
 * - `maxRequests` caps one operation's requests, well below the subrequest
 *   limit of Workers' paid plan (set it to your plan's).
 * - A request or a body that fails is requested again from the failed
 *   read's offset, up to `retries` times per read (and for the first
 *   request), unless the failure is one a new request cannot fix.
 * - Advice that the reads touch only some regions (the headers of ProRes
 *   samples, the audio of an extraction) reads them in a pipeline that
 *   skips gaps of `sparseGap` bytes or more. Skipping a gap pays above
 *   round trip × bandwidth / `parallel` (docs/design/range-extraction.md);
 *   with `sparseGap: "auto"` the round trip is the wait for the session's
 *   first response and the bandwidth is `bandwidth` (bytes per second, an
 *   assumed value, not a measurement), the gap kept within 1 to 8 MiB.
 */
export const RANGE_DEFAULTS: Readonly<Required<RangeOptions>>;

/** Initialize once with the package module or its bytes. Needs JSPI and exception references; later calls do not replace the instance. */
export function init(module: WebAssembly.Module | BufferSource): void;
/** The fixed ffprobe document parsed as JavaScript values. ffprobeText preserves integers above 2^53 exactly. */
export function ffprobe(source: Source, options?: FfprobeOptions): Promise<FfprobeDocument>;
/** Fixed ffprobe JSON text, unchanged. Options are filename (default "-") and limits; never use a signed URL as the filename. */
export function ffprobeText(source: Source, options?: FfprobeOptions): Promise<string>;
/** The probe document. Integers above 2^53 (Matroska's `track_uid`) come back rounded; `probeText` keeps them exact. */
export function probe(source: Source, options?: ProbeOptions): Promise<Record<string, unknown>>;

/** The probe document as the command's JSON text, every integer exact. */
export function probeText(source: Source, options?: ProbeOptions): Promise<string>;
/**
 * Extracts one AAC track to M4A by stream copy. Options:
 * `stream` (the stream index; the first audio stream by default), `allow`,
 * `strict`, `limits`.
 */
export function extractAudio(source: Source, options?: ExtractAudioOptions): Promise<Output<Record<string, unknown>>>;
/**
 * Cuts a keyframe-aligned clip to MP4 by stream copy, as
 * `coreav clip`. Options: `start` (required) and `end` in seconds, as a
 * decimal string (`"12.5"`) or a number; `video` (stream index); `audio`
 * (stream index or `"none"`); `allow`, `strict`, `limits`. The summary is
 * the `coreav.clip/1` document.
 */
export function clip(source: Source, options: ClipOptions): Promise<Output<Record<string, unknown>>>;

/** The HLS index of `source` and its `coreav.hls-index/2` summary. */
export function hlsIndex(
  source: Source,
  options?: HlsIndexOptions,
): Promise<{ index: Uint8Array; summary: Record<string, unknown> }>;

/**
 * One HLS resource (`master.m3u8`, `video.m3u8`, `video-init.mp4`, `video-N.m4s`, or the same for
 * `audio`) cut from an index; `source` is read for media segments only.
 */
export function hlsGet(
  index: Uint8Array | ArrayBuffer | ArrayBufferView,
  name: string,
  source?: Source | null,
  options?: HlsGetOptions,
): Promise<Output<{ bytes: number }>>;
/**
 * Checks an input against an upload policy, as `coreav check`: the
 * `coreav.verdict/1` verdict (`docs/schema/verdict-v1.md`). `policy` is a
 * `coreav.policy/1` policy (`docs/schema/policy-v1.md`), as an object or
 * JSON text (read as `coreav check` reads a policy file); `limits` as for
 * `probe`. A file the probe rejects gets a reject verdict; only a source
 * that cannot be read throws.
 */
export function check(source: Source, options: CheckOptions): Promise<Record<string, unknown>>;
/**
 * The chapter list of an MP4/MOV input (QuickTime chapter track or Nero
 * `chpl`): the `coreav.chapters/1` document, as `coreav chapters`.
 * Options: `limits`.
 */
export function chapters(source: Source, options?: ChapterOptions): Promise<Record<string, unknown>>;
/**
 * Extracts a 3GPP timed text track to WebVTT, as `coreav
 * extract-subtitles`. Options: `stream` (the stream index; the first text
 * track that is not a chapter track by default), `limits`. The summary is
 * `coreav.extract-subtitles/1`.
 */
export function extractSubtitles(source: Source, options?: SubtitleOptions): Promise<Output<Record<string, unknown>>>;

/** Opens the source with `memoryBytes` of 40 MiB: it holds the output's sample tables itself. */
export function remux(source: Source, options?: RemuxOptions): Promise<Output<Record<string, unknown>>>;

/** As `remux`, with the same 40 MiB session bound. */
export function sanitize(source: Source, options?: SanitizeOptions): Promise<Output<Record<string, unknown>>>;

/** An input already in memory: a `Uint8Array`, `ArrayBuffer` or view. */
export function bytesSource(bytes: Uint8Array | ArrayBuffer | ArrayBufferView): Source;
/**
 * A remote object read by byte ranges. `fetchRange({ offset, length,
 * validator, signal })` starts one request and resolves to `{ body, offset,
 * end, size, validator }`: `body` streams the bytes from `offset` (the
 * first byte the response holds, which must equal the requested offset)
 * for at most `length` bytes (or, with `whole: true`, the whole object
 * from byte 0); `end`, if given, is the offset just past the last byte it
 * holds, `size` is the object's length and `validator` its ETag, if any.
 * When `validator` is given, the request must fail with
 * `SourceChangedError` if the object no longer has it. `signal` aborts when
 * the session closes; a request still waiting should then fail.
 *
 * The returned source keeps the read statistics of its latest session in
 * `lastStats`.
 */
export function rangeSource(
  fetchRange: (request: FetchRangeRequest) => Promise<FetchRangeResponse>,
  options?: RangeOptions,
): RangeSource;
/**
 * An object behind an HTTP(S) URL that answers range requests, such as a
 * presigned object-storage URL. `options.headers` are sent with every
 * request, `options.fetch` replaces the global `fetch`, and redirects are
 * refused unless `options.followRedirects` is set (a redirect would move
 * the reads to a host the caller did not choose); the other options are
 * those of `rangeSource`. A strong ETag is pinned with `If-Match`; without
 * one, only the object's size is checked between requests.
 */
export function httpSource(
  url: string | URL,
  options?: RangeOptions & { headers?: HeadersInit; fetch?: typeof fetch; followRedirects?: boolean },
): RangeSource;

/** Version, packaged-artifact SHA-256 and current linear memory size. Custom init callers must supply the matching module. */
export function moduleInfo(): { version: string; build: string; memoryBytes: number };

/**
 * The input was rejected or could not be read: a `coreav.error/1` object,
 * as `coreav` prints it, in `document`, with its fields copied for
 * convenience. `documentText` retains the original JSON number tokens.
 * When a source failed, `cause` is the source's error.
 */
export class CoreAVError extends Error {
  constructor(document: ErrorDocument | string, options?: ErrorOptions);
  document: ErrorDocument;

  /** Original JSON text, preserving integer precision for HTTP responses. */
  documentText: string;
  kind: ErrorDocument["kind"];
  limit: string | null;
  offset: number | null;
}
/** The object behind a source changed while it was read. */
export class SourceChangedError extends Error {}
/** The source object does not exist. Adapters do not put a URL or key in the message. */
export class SourceNotFoundError extends Error {
  constructor(message?: string);
}
/** A source exceeded its request cap (`maxRequests`). */
export class RequestLimitError extends Error {
  constructor(limit: number);
  limit: number;
}

/** Map usage, missing-source and CoreAV errors to 400, 404, 422 or 502. Rethrows unrelated errors. */
export function errorResponse(error: unknown): Response;
