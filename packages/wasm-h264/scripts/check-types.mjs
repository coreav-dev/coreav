import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const compiler = fileURLToPath(new URL("../../wasm/node_modules/typescript/bin/tsc", import.meta.url));
const fixture = fileURLToPath(new URL("../src/typecheck.mts", import.meta.url));
const invalid = fileURLToPath(new URL("../src/typecheck-invalid.mts", import.meta.url));

function compile(path) {
  return spawnSync(process.execPath, [
    compiler,
    "--strict",
    "--module",
    "nodenext",
    "--moduleResolution",
    "nodenext",
    "--target",
    "es2022",
    "--noEmit",
    "--pretty",
    "false",
    path,
  ], { encoding: "utf8", cwd: root });
}

try {
  writeFileSync(fixture, `import { bytesSource, thumbnail } from "./index.js";
const source = bytesSource(new Uint8Array());
await thumbnail(source, { at: "1.5", signal: new AbortController().signal });
await thumbnail(source, { at: 1, height: 20, format: "jpg", filter: "bicubic", chroma: "native", quality: 91, limits: { maxTracks: 1, maxElements: 1, maxWidth: 1920, maxPixels: 2073600, maxPlanningBytes: 1, maxOutputBytes: 1 } });
`);
  writeFileSync(invalid, `import { thumbnail } from "./index.js";
await thumbnail(null, { width: 1 });
`);
  const accepted = compile(fixture);
  assert.equal(accepted.status, 0, accepted.stdout + accepted.stderr);
  const refused = compile(invalid);
  assert.notEqual(refused.status, 0, "thumbnail accepted a missing source and time");
} finally {
  rmSync(fixture, { force: true });
  rmSync(invalid, { force: true });
}
