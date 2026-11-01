export * from "./index.js";
import type { RangeOptions, RangeSource } from "./index.js";

export interface R2BucketLike {
  get(key: string, options: { range: { offset: number; length: number }; onlyIf?: { etagMatches: string } }): Promise<R2ObjectLike | null>;
}

export interface R2ObjectLike {
  size: number;
  etag: string;
  range?: { offset?: number; length?: number } | { suffix: number };
  body?: ReadableStream<Uint8Array> | null;
}

export const R2_DEFAULTS: Readonly<RangeOptions>;
export function r2Source(bucket: R2BucketLike, key: string, options?: RangeOptions): RangeSource;
