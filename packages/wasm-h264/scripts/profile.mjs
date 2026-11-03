import { PACKED, SIZE_BAR } from "../../wasm/scripts/check-package.mjs";
import { fileURLToPath } from "node:url";

export const EXTRA = Object.freeze([
  "package/src/node-broker.js",
  "package/src/node-session.js",
  "package/src/node-worker.js",
  "package/src/thumbnail-options.js",
  "package/src/thumbnail.js",
]);

export const profile = Object.freeze({
  name: "@coreav/wasm-h264",
  packed: Object.freeze([...PACKED, ...EXTRA]),
  root: fileURLToPath(new URL("..", import.meta.url)),
  media: false,
  features: Object.freeze(["native-h264"]),
  rawPrefix: "target/wasm-h264/",
  sizeBar: SIZE_BAR,
});
