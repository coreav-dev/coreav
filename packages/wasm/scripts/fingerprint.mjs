import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export function moduleHash(directory) {
  return createHash("sha256").update(readFileSync(resolve(directory, "wasm/coreav_bg.wasm"))).digest("hex");
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const directory = process.argv[2] ?? fileURLToPath(new URL("..", import.meta.url));
  writeFileSync(resolve(directory, "wasm/build.js"), `export const BUILD = "${moduleHash(directory)}";\n`);
}
