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

  maxMovieBytes?: number;

  maxMovieRatio?: number;
}

interface CommonOptions {

  allow?: string[];

  strict?: boolean;
  limits?: Limits;
}

export interface ProbeOptions extends CommonOptions {
  scanFragments?: boolean;
  scanFrameSizes?: boolean;

}

export interface FfprobeOptions {
  filename?: string;
  limits?: Limits;
}

export interface FfprobeDocument {
  streams: Array<Record<string, unknown>>;
  format: Record<string, unknown>;
}

export interface ExtractAudioOptions extends CommonOptions {

  stream?: number;
}

export interface ClipOptions extends CommonOptions {

  start: string | number;
  end?: string | number;
  video?: number;
  audio?: number | "none";
}

export interface HlsIndexOptions extends CommonOptions {

  segmentDuration?: string | number;

  video?: number | "none";

  audio?: number | "none";

  tag?: string;
}

export interface HlsGetOptions {
  limits?: Limits;
}

export interface CheckOptions {

  policy: Record<string, unknown> | string;

  limits?: Limits;
}

export interface SubtitleOptions {

  stream?: number;
  limits?: Limits;
}

export interface ChapterOptions {
  limits?: Limits;
}

export interface RemuxOptions extends CommonOptions {

  streams?: number[];
  failOnUnsupported?: boolean;

  dropUnsupported?: boolean;
}

export interface SanitizeOptions extends RemuxOptions {
  keep?: Array<"rotation" | "language" | "dates" | "gapless" | "kinds" | "none">;
}

export interface Output<Summary> {
  body: ReadableStream<Uint8Array>;

  summary: Promise<Summary>;
  contentType: string;
}

export type Session =
  | {
      size: number;
      inMemory: true;

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

  sparseGap?: number | "auto";

  bandwidth?: number;

  memoryBytes?: number;
}

export interface ReadStats {
  requests: number;
  pipelines: number;
  bytesRequested: number;
  bytesReceived: number;

  maxHeldBytes: number;
}

export interface RangeSource extends Source {

  lastStats: ReadStats | null;
}

export interface FetchRangeRequest {
  offset: number;
  length: number;

  signal: AbortSignal;

  validator?: string;
}

export interface FetchRangeResponse {
  body: ReadableStream<Uint8Array>;

  offset: number;

  end?: number;

  size: number;
  validator?: string;

  whole?: boolean;
}

export const RANGE_DEFAULTS: Readonly<Required<RangeOptions>>;

export function init(module: WebAssembly.Module | BufferSource): void;
export function ffprobe(source: Source, options?: FfprobeOptions): Promise<FfprobeDocument>;
export function ffprobeText(source: Source, options?: FfprobeOptions): Promise<string>;
export function probe(source: Source, options?: ProbeOptions): Promise<Record<string, unknown>>;

export function probeText(source: Source, options?: ProbeOptions): Promise<string>;
export function extractAudio(source: Source, options?: ExtractAudioOptions): Promise<Output<Record<string, unknown>>>;
export function clip(source: Source, options: ClipOptions): Promise<Output<Record<string, unknown>>>;

export function hlsIndex(
  source: Source,
  options?: HlsIndexOptions,
): Promise<{ index: Uint8Array; summary: Record<string, unknown> }>;

export function hlsGet(
  index: Uint8Array | ArrayBuffer | ArrayBufferView,
  name: string,
  source?: Source | null,
  options?: HlsGetOptions,
): Promise<Output<{ bytes: number }>>;
export function check(source: Source, options: CheckOptions): Promise<Record<string, unknown>>;
export function chapters(source: Source, options?: ChapterOptions): Promise<Record<string, unknown>>;
export function extractSubtitles(source: Source, options?: SubtitleOptions): Promise<Output<Record<string, unknown>>>;

export function remux(source: Source, options?: RemuxOptions): Promise<Output<Record<string, unknown>>>;

export function sanitize(source: Source, options?: SanitizeOptions): Promise<Output<Record<string, unknown>>>;

export function bytesSource(bytes: Uint8Array | ArrayBuffer | ArrayBufferView): Source;
export function rangeSource(
  fetchRange: (request: FetchRangeRequest) => Promise<FetchRangeResponse>,
  options?: RangeOptions,
): RangeSource;
export function httpSource(
  url: string | URL,
  options?: RangeOptions & { headers?: HeadersInit; fetch?: typeof fetch; followRedirects?: boolean },
): RangeSource;

export function moduleInfo(): { version: string; build: string; memoryBytes: number };

export class CoreAVError extends Error {
  constructor(document: ErrorDocument | string, options?: ErrorOptions);
  document: ErrorDocument;

  documentText: string;
  kind: ErrorDocument["kind"];
  limit: string | null;
  offset: number | null;
}
export class SourceChangedError extends Error {}
export class SourceNotFoundError extends Error {
  constructor(message?: string);
}
export class RequestLimitError extends Error {
  constructor(limit: number);
  limit: number;
}

export function errorResponse(error: unknown): Response;
