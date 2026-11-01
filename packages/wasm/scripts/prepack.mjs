import { accessSync, copyFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { moduleHash } from "./fingerprint.mjs";
import { allowedLicenses, linkedCrates, renderNotices } from "./legal.mjs";

const directory = process.argv[2] ?? fileURLToPath(new URL("..", import.meta.url));
const repo = resolve(fileURLToPath(new URL("..", import.meta.url)), "../..");

for (const file of [ "wasm/coreav.js", "wasm/coreav_bg.wasm", "wasm/build.js" ]) {
  try {
    accessSync(resolve(directory, file));
  } catch {
    throw new Error(`run npm run build first: ${file} is missing`);
  }
}

const expected = `export const BUILD = "${moduleHash(directory)}";\n`;
if (readFileSync(resolve(directory, "wasm/build.js"), "utf8") !== expected) {
  throw new Error("run npm run build first: the module fingerprint is stale");
}

for (const name of ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE"]) {
  if (!existsSync(resolve(directory, name))) copyFileSync(resolve(repo, name), resolve(directory, name));
}
if (!existsSync(resolve(directory, "THIRD-PARTY-NOTICES.txt"))) {
  writeFileSync(resolve(directory, "THIRD-PARTY-NOTICES.txt"), renderNotices(linkedCrates(repo), allowedLicenses(repo)));
}
