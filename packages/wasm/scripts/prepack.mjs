import { accessSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { moduleHash } from "./fingerprint.mjs";
import { packageLegalFiles } from "./legal.mjs";

export function verifyPackage(directory, legalFiles) {
  for (const file of [ "wasm/coreav.js", "wasm/coreav_bg.wasm", "wasm/build.js" ]) {
    try { accessSync(resolve(directory, file)); }
    catch { throw new Error(`run npm run build first: ${file} is missing`); }
  }
  const expected = `export const BUILD = "${moduleHash(directory)}";\n`;
  if (readFileSync(resolve(directory, "wasm/build.js"), "utf8") !== expected) throw new Error("run npm run build first: the module fingerprint is stale");
  for (const [name, bytes] of legalFiles) {
    let actual;
    try { actual = readFileSync(resolve(directory, name)); }
    catch { throw new Error(`run npm run build: ${name} is missing or stale`); }
    if (!actual.equals(bytes)) throw new Error(`run npm run build: ${name} is missing or stale`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const directory = process.argv[2] ?? fileURLToPath(new URL("..", import.meta.url));
  const repo = resolve(fileURLToPath(new URL("..", import.meta.url)), "../..");
  verifyPackage(directory, packageLegalFiles(directory, repo));
}
