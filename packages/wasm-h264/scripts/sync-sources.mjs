import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const destination = resolve(here, "../src");
const source = resolve(here, "../../wasm/src");
export const COPIED = Object.freeze([
  "api.js",
  "bounded-source.js",
  "engine-core.js",
  "errors.js",
  "internal.js",
  "r2.js",
  "sources.js",
  "workers.d.ts",
  "workers.js",
]);
const ENGINE_FROM = "export const { init, moduleInfo, runToText, runToStream } = createEngine(\n  bindings,\n  BUILD,\n);";
const ENGINE_TO = "export const { init, moduleInfo, runToText, runToStream, diagnostics } = createEngine(\n  bindings,\n  BUILD,\n  { exclusive: true },\n);";
const INDEX_FROM = 'from "./api.js";\n';
const INDEX_TO = 'from "./api.js";\n\nexport { thumbnail } from "./thumbnail.js";\n';
const TYPES_SUFFIX = `
export interface ThumbnailLimits extends Limits {
  maxElements?: number;
  maxElementBytes?: number;
  maxIndexBytes?: number;
  maxBlockWork?: number;
  maxWidth?: number;
  maxHeight?: number;
  maxPixels?: number;
  maxFrameBytes?: number;
  maxDecodedBytes?: number;
  maxPendingBytes?: number;
  maxMemoryBytes?: number;
  maxPlanningBytes?: number;
  maxOutputBytes?: number;
}

export interface ThumbnailOptions {
  at: string | number;
  stream?: number;
  width?: number;
  height?: number;
  format?: "jpeg" | "jpg" | "png" | "webp";
  quality?: number;
  filter?: "bilinear" | "bicubic" | "area";
  chroma?: "native" | "full";
  limits?: ThumbnailLimits;
  signal?: AbortSignal;
}

export interface ThumbnailSummary {
  schema: "coreav.thumbnail/1";
  output: { bytes: number; format: "jpeg" | "png" | "webp"; width: number; height: number };
  time: {
    requested: { num: number; den: number } | null;
    frame: { num: number; den: number } | null;
    frame_seconds: number;
    decode_start: { num: number; den: number } | null;
  };
  stream_index: number;
  codec: string;
  keyframe: boolean;
  packet: { offset: number; size: number };
  packets_decoded: number;
  decoder: { backend: string; name: string; isolation: string; fell_back: boolean };
  warnings: string[];
  skipped: Array<{ backend: string; reason: string }>;
}

export function thumbnail(source: Source, options: ThumbnailOptions): Promise<Output<ThumbnailSummary>>;
`;

export function exclusiveEngine(text) {
  if (!text.includes(ENGINE_FROM) || text.includes("exclusive")) {
    throw new Error("packages/wasm/src/engine.js no longer has the closed exclusive insertion point");
  }
  return text.replace(ENGINE_FROM, ENGINE_TO);
}

export function thumbnailIndex(text) {
  if (!text.includes(INDEX_FROM) || text.includes("thumbnail")) {
    throw new Error("packages/wasm/src/index.js no longer has the closed thumbnail insertion point");
  }
  return text.replace(INDEX_FROM, INDEX_TO);
}

export function thumbnailTypes(text) {
  if (text.includes("thumbnail")) {
    throw new Error("packages/wasm/src/index.d.ts already names thumbnail");
  }
  return text.endsWith("\n") ? text + TYPES_SUFFIX : `${text}\n${TYPES_SUFFIX}`;
}

export function expectedSources() {
  const files = new Map(COPIED.map((name) => [name, readFileSync(resolve(source, name))]));
  files.set("engine.js", Buffer.from(exclusiveEngine(readFileSync(resolve(source, "engine.js"), "utf8"))));
  files.set("index.js", Buffer.from(thumbnailIndex(readFileSync(resolve(source, "index.js"), "utf8"))));
  files.set("index.d.ts", Buffer.from(thumbnailTypes(readFileSync(resolve(source, "index.d.ts"), "utf8"))));
  files.set("node.d.ts", Buffer.from(readFileSync(resolve(source, "node.d.ts"), "utf8") + `
import type { Source, ThumbnailOptions, ThumbnailSummary, Output } from "./index.js";
export function thumbnail(source: Source, options: ThumbnailOptions & { deadline?: number }): Promise<Output<ThumbnailSummary>>;
`));
  return files;
}

export function verifyCopies() {
  for (const [name, bytes] of expectedSources()) {
    let actual;
    try {
      actual = readFileSync(resolve(destination, name));
    } catch {
      throw new Error(`run node scripts/sync-sources.mjs: ${name} is missing`);
    }
    if (!actual.equals(bytes)) throw new Error(`run node scripts/sync-sources.mjs: ${name} is stale`);
  }
}

export function writeCopies() {
  for (const [name, bytes] of expectedSources()) writeFileSync(resolve(destination, name), bytes);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  writeCopies();
  verifyCopies();
}
