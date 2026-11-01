/** Load the packaged WASM through the runtime's compiled-module import. */
import module from "../wasm/coreav_bg.wasm";

import { init } from "./engine.js";

init(module);

export * from "./index.js";

export { R2_DEFAULTS, r2Source } from "./r2.js";
