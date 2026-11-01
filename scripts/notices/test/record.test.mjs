import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { makeRecord, verifyRecord, NOTICE_FILE, RECORD_FILE } from "../record.mjs";

function fixture() {
  const repo = mkdtempSync(join(tmpdir(), "coreav-record-"));
  const put = (path, text) => { mkdirSync(dirname(join(repo, path)), { recursive: true }); writeFileSync(join(repo, path), text); };
  for (const path of ["Cargo.lock", "Cargo.toml", "deny.toml", "NOTICE", "crates/a/Cargo.toml", "packages/wasm/scripts/build.sh", "scripts/notices/main.mjs", "scripts/notices/record.mjs"]) put(path, path + "\n");
  put("scripts/rust-dist.env", "RUST_DIST_TOOLCHAIN=1.2.3\nRUST_DIST_COMMIT=" + "a".repeat(40) + "\n");
  const distribution = { package: "coreav-wasm", target: "wasm32-unknown-unknown", profile: "edge", features: [], defaultFeatures: true, rustflags: ["-C", "target-feature=+simd128"] };
  put("packages/wasm/wasm/coreav_bg.wasm", "binary bytes\n");
  put("target/raw.wasm", "raw binary bytes\n");
  put("packages/wasm/" + NOTICE_FILE, "upstream notices\n");
  const directory = join(repo, "packages/wasm");
  const record = makeRecord(directory, repo, distribution, { version: "1.2.3", commit: "a".repeat(40) }, join(directory, "wasm/coreav_bg.wasm"), join(repo, "target/raw.wasm"), "upstream notices\n", new Map(), { inTree: [], vendored: [] });
  const save = value => put("packages/wasm/" + RECORD_FILE, JSON.stringify(value));
  save(record);
  return { repo, directory, put, record, save, close: () => rmSync(repo, { recursive: true, force: true }) };
}

test("verified packing reads no raw target and starts no toolchain process", () => {
  const f = fixture();
  const methods = ["spawn", "spawnSync", "exec", "execSync", "execFile", "execFileSync", "fork"];
  const saved = Object.fromEntries(methods.map(name => [name, childProcess[name]]));
  try {
    rmSync(join(f.repo, "target"), { recursive: true });
    for (const name of methods) childProcess[name] = () => { throw new Error("packing tried to start a toolchain process"); };
    syncBuiltinESMExports();
    assert.equal(verifyRecord(f.directory, f.repo).notices, f.record.notices);
  } finally {
    Object.assign(childProcess, saved);
    syncBuiltinESMExports();
    f.close();
  }
});

test("an existing notices file without its generated record is not blessed", () => {
  const f = fixture();
  try {
    rmSync(join(f.directory, RECORD_FILE));
    assert.throws(() => verifyRecord(f.directory, f.repo), /has no build record/);
    assert.equal(readFileSync(join(f.directory, NOTICE_FILE), "utf8"), "upstream notices\n");
  } finally { f.close(); }
});

test("changed notice bytes and artifact bytes are refused without rewriting", () => {
  for (const path of ["packages/wasm/" + NOTICE_FILE, "packages/wasm/wasm/coreav_bg.wasm"]) {
    const f = fixture();
    try {
      f.put(path, "changed\n");
      assert.throws(() => verifyRecord(f.directory, f.repo), /differs from the one|artifact changed/);
      assert.equal(readFileSync(join(f.repo, path), "utf8"), "changed\n");
    } finally { f.close(); }
  }
});

test("input changes and newly discovered member, generator and upstream files make records stale", () => {
  for (const path of ["Cargo.lock", "scripts/rust-dist.env", "crates/new-member/Cargo.toml", "scripts/notices/new-module.mjs", "scripts/notices/upstream/pkg/LICENSE"]) {
    const f = fixture();
    try {
      f.put(path, "changed\n");
      assert.throws(() => verifyRecord(f.directory, f.repo), /inputs changed|is stale/);
    } finally { f.close(); }
  }
});

test("malformed records and artifact path escapes fail before reading outside the distribution", () => {
  const f = fixture();
  try {
    for (const record of [
      { ...f.record, format: 2 },
      { ...f.record, artifact: { path: "../../NOTICE", sha256: f.record.artifact.sha256 } },
      { ...f.record, distribution: { ...f.record.distribution, profile: undefined } },
      { ...f.record, distribution: { ...f.record.distribution, rustflags: ["--remap-path-prefix=/private=/r"] } },
      { ...f.record, raw: undefined },
      { ...f.record, toolchain: { ...f.record.toolchain, commit: "b".repeat(40) } },
    ]) {
      f.save(record);
      assert.throws(() => verifyRecord(f.directory, f.repo));
    }
    f.save(f.record);
    rmSync(join(f.directory, "wasm"), { recursive: true });
    mkdirSync(join(f.repo, "elsewhere"));
    f.put("elsewhere/coreav_bg.wasm", "binary bytes\n");
    symlinkSync(join(f.repo, "elsewhere"), join(f.directory, "wasm"), "dir");
    assert.throws(() => verifyRecord(f.directory, f.repo), /missing or unreadable build inputs/);
  } finally { f.close(); }
});
