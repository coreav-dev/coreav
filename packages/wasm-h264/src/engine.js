import * as bindings from "../wasm/coreav.js";
import { BUILD } from "../wasm/build.js";
import { createEngine } from "./engine-core.js";

export const { init, moduleInfo, runToText, runToStream, diagnostics } = createEngine(
  bindings,
  BUILD,
  { exclusive: true },
);
