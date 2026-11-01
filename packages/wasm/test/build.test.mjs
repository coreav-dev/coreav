import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { moduleHash } from "../scripts/fingerprint.mjs";
import { EXNREF_PROBE } from "../src/internal.js";

test("the exception-reference probe is a valid type-only module", () => {
  const module = new WebAssembly.Module(EXNREF_PROBE);
  assert.deepEqual(WebAssembly.Module.imports(module), []);
  assert.deepEqual(WebAssembly.Module.exports(module), []);
  const invalidType = EXNREF_PROBE.slice();
  invalidType[13] = 0x50;
  assert.equal(WebAssembly.validate(invalidType), false);
});

test("module fingerprints follow the shipped bytes rather than the package version", () => {
  const directory = mkdtempSync(join(tmpdir(), "coreav-build-"));
  try {
    mkdirSync(join(directory, "wasm"));
    writeFileSync(join(directory, "package.json"), '{"version":"0.0.1"}');
    for (const name of ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "wasm/coreav.js"]) {
      writeFileSync(join(directory, name), "test");
    }
    writeFileSync(join(directory, "wasm/coreav_bg.wasm"), new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]));
    const fingerprint = fileURLToPath(new URL("../scripts/fingerprint.mjs", import.meta.url));
    const prepack = fileURLToPath(new URL("../scripts/prepack.mjs", import.meta.url));
    execFileSync(process.execPath, [fingerprint, directory]);
    const first = moduleHash(directory);
    assert.match(first, /^[a-f0-9]{64}$/);
    assert.equal(readFileSync(join(directory, "wasm/build.js"), "utf8"), `export const BUILD = "${first}";\n`);
    execFileSync(process.execPath, [prepack, directory]);
    writeFileSync(join(directory, "wasm/coreav_bg.wasm"), new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 0, 1, 0]));
    assert.notEqual(moduleHash(directory), first);
    assert.throws(() => execFileSync(process.execPath, [prepack, directory], { stdio: "pipe" }), /stale/);
    execFileSync(process.execPath, [fingerprint, directory]);
    execFileSync(process.execPath, [prepack, directory]);
    assert.equal(JSON.parse(readFileSync(join(directory, "package.json"))).version, "0.0.1");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
