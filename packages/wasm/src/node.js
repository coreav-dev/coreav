/** Load the packaged module from disk. Node needs JSPI and exception references. */
import { readFileSync } from "node:fs";

import { init } from "./engine.js";

init(readFileSync(new URL("../wasm/coreav_bg.wasm", import.meta.url)));

export * from "./index.js";
