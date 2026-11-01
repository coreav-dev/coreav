import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { moduleHash } from "../scripts/fingerprint.mjs";
import { EXNREF_PROBE } from "../src/internal.js";
import { renderNotices, selectCrates } from "../scripts/legal.mjs";
import { verifyPackage } from "../scripts/prepack.mjs";

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
    for (const name of ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD-PARTY-NOTICES.txt", "wasm/coreav.js"]) {
      writeFileSync(join(directory, name), "test");
    }
    writeFileSync(join(directory, "wasm/coreav_bg.wasm"), new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]));
    const fingerprint = fileURLToPath(new URL("../scripts/fingerprint.mjs", import.meta.url));
    const legalFiles = new Map(["LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD-PARTY-NOTICES.txt"].map((name) => [name, Buffer.from("test")]));
    execFileSync(process.execPath, [fingerprint, directory]);
    const first = moduleHash(directory);
    assert.match(first, /^[a-f0-9]{64}$/);
    assert.equal(readFileSync(join(directory, "wasm/build.js"), "utf8"), `export const BUILD = "${first}";\n`);
    verifyPackage(directory, legalFiles);
    writeFileSync(join(directory, "wasm/coreav_bg.wasm"), new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 0, 1, 0]));
    assert.notEqual(moduleHash(directory), first);
    assert.throws(() => verifyPackage(directory, legalFiles), /stale/);
    execFileSync(process.execPath, [fingerprint, directory]);
    verifyPackage(directory, legalFiles);
    assert.equal(JSON.parse(readFileSync(join(directory, "package.json"))).version, "0.0.1");
    for (const [name, bytes] of legalFiles) {
      writeFileSync(join(directory, name), "stale legal text");
      assert.throws(() => verifyPackage(directory, legalFiles), new RegExp(`${name} is missing or stale`));
      assert.equal(readFileSync(join(directory, name), "utf8"), "stale legal text");
      rmSync(join(directory, name));
      assert.throws(() => verifyPackage(directory, legalFiles), new RegExp(`${name} is missing or stale`));
      assert.equal(existsSync(join(directory, name)), false);
      writeFileSync(join(directory, name), bytes);
    }
    verifyPackage(directory, legalFiles);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("dependency notices are deterministic upstream texts and fail closed on gaps", () => {
  const files = { "LICENSE-MIT": new TextEncoder().encode("Upstream permission\n"), NOTICE: new TextEncoder().encode("Required notice\n") };
  const list = () => Object.keys(files).map((name) => ({ name, isFile: () => true, isSymbolicLink: () => false }));
  const read = (dir, name) => files[name];
  const crates = [{ name: "z", version: "1.0.0", license: "MIT", dir: "/private/home" }, { name: "a", version: "1.0.0", license: "MIT", dir: "/private/other" }];
  const allowed = new Set(["MIT"]);
  const text = renderNotices(crates, allowed, list, read);
  assert.equal(text, renderNotices([...crates].reverse(), allowed, list, read));
  assert.ok(text.indexOf("=== a ") < text.indexOf("=== z "));
  assert.match(text, /Upstream permission\n/);
  assert.match(text, /Required notice\n/);
  assert.doesNotMatch(text, /private\/home|private\/other/);
  assert.throws(() => renderNotices(crates, allowed, () => [], read), /a 1.0.0: no upstream licence file/);
  assert.throws(() => renderNotices([{ ...crates[0], license: "GPL-3.0" }], allowed, list, read), /unapproved/);
  assert.throws(() => renderNotices(crates, allowed, list, () => new Uint8Array()), /empty/);
  assert.throws(() => renderNotices(crates, allowed, list, () => new Uint8Array([255])), /non-UTF-8/);
  assert.throws(() => renderNotices(crates, allowed, () => { throw new Error("host detail"); }, read), /a 1.0.0: cannot read/);
  assert.throws(() => renderNotices(crates, allowed, list, () => { throw new Error("host detail"); }), /a 1.0.0: cannot read/);
  assert.throws(() => renderNotices(crates, allowed, list, () => new Uint8Array(256 * 1024 + 1)), /oversized/);
  assert.throws(() => renderNotices(crates, allowed, () => [{ name: "LICENSE-MIT", isFile: () => true, isSymbolicLink: () => true }], read), /not a regular file/);
  assert.match(renderNotices(crates, allowed, list, () => new TextEncoder().encode("permission\r\n\r\n")), /permission\n/);
  assert.doesNotMatch(renderNotices(crates, allowed, list, () => new TextEncoder().encode("permission\r\n")), /\r/);
  const alternative = [{ ...crates[0], license: "MIT OR Apache-2.0" }];
  assert.match(renderNotices(alternative, new Set(["MIT", "Apache-2.0"]), list, read), /Upstream permission/);
  assert.throws(() => renderNotices([{ ...crates[0], license: "MIT AND Apache-2.0" }], new Set(["MIT", "Apache-2.0"]), list, read), /z 1.0.0: missing.*Apache-2.0/);
  assert.throws(() => renderNotices([{ ...crates[0], license: "Unknown" }], new Set(["Unknown"]), list, read), /add a file rule for Unknown/);
});

test("dependency selection excludes only workspace members and refuses metadata gaps", () => {
  const metadata = { workspace_members: ["own"], packages: [{ id: "own", name: "coreav-wasm", version: "0.0.1", source: null, license: "MIT OR Apache-2.0", manifest_path: "/repo/crates/coreav-wasm/Cargo.toml" }, { id: "other", name: "upstream", version: "1.0.0", source: "registry+https://github.com/rust-lang/crates.io-index", manifest_path: "/registry/upstream/Cargo.toml", license: "MIT" }] };
  const selected = selectCrates(metadata, "coreav-wasm v0.0.1 (/repo)\nupstream v1.0.0\nupstream v1.0.0\n", "/repo");
  assert.deepEqual(selected, [{ name: "upstream", version: "1.0.0", dir: "/registry/upstream", license: "MIT" }]);
  assert.throws(() => selectCrates(metadata, "missing v1.0.0"), /missing/);
  assert.throws(() => selectCrates({ ...metadata, workspace_members: [] }, "coreav-wasm v0.0.1", "/repo"), /not a registry/);
  assert.throws(() => selectCrates(metadata, "coreav-wasm v0.0.1", "/elsewhere"), /workspace licence or location/);
});
