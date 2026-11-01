import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const compiler = fileURLToPath(new URL("../node_modules/typescript/bin/tsc", import.meta.url));
const flags = ["--noEmit", "--strict", "--target", "ES2022", "--module", "NodeNext"];
execFileSync(process.execPath, [compiler, ...flags, fileURLToPath(new URL("../test/types.mts", import.meta.url))], { stdio: "inherit" });
const directory = mkdtempSync(join(tmpdir(), "coreav-types-"));
try {
  const root = fileURLToPath(new URL("../src/index.js", import.meta.url));
  const invalid = join(directory, "invalid.mts");
  writeFileSync(invalid, `import { probe, ffprobe, bytesSource, r2Source } from ${JSON.stringify(root)};
const source = bytesSource(new Uint8Array());
probe(source, { filename: "file.mp4" });
probe(source, { format: "native" });
ffprobe(source, { allow: ["h264"] });
ffprobe(source, { scanFragments: true });
`);
  const checked = spawnSync(process.execPath, [compiler, ...flags, invalid], { encoding: "utf8" });
  assert.equal(checked.status, 2, checked.stdout + checked.stderr);
  for (const name of ["r2Source", "filename", "format", "allow", "scanFragments"]) {
    assert.ok(checked.stdout.includes(name), `the declarations accepted ${name}`);
  }
} finally {
  rmSync(directory, { recursive: true, force: true });
}
