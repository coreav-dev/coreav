export * from "./index.js";

import type { Source, ThumbnailOptions, ThumbnailSummary, Output } from "./index.js";
export function thumbnail(source: Source, options: ThumbnailOptions & { deadline?: number }): Promise<Output<ThumbnailSummary>>;
