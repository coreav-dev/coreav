import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { nativeEvidence, noticeSections, validateEvidence } from "../evidence.mjs";
import { renderRuntimes, runtimeComponents, runtimeManifest } from "../runtime.mjs";

const commit = runtimeManifest.rustCommit;
const context = { commit, version: "1.99.1", library: new Set(), members: new Set(), memberPaths: new Set() };
const evidence = nativeEvidence(Buffer.alloc(0));

test("Microsoft runtime notices distinguish the SDK and Visual Studio terms", () => {
  const text = renderRuntimes("x86_64-pc-windows-msvc", commit);
  assert.ok(text.includes("UCRT is distributed under the Windows SDK terms:"));
  assert.ok(text.includes("https://learn.microsoft.com/en-us/legal/windows-sdk/license"));
  assert.equal(noticeSections(text).get("C runtime\0Microsoft C runtime").license, "Visual Studio redistribution terms AND Windows SDK terms");
});

test("Windows notices identify the static Microsoft CRT and link its terms", () => {
  const target = "x86_64-pc-windows-msvc";
  const text = renderRuntimes(target, commit);
  assert.ok(text.includes("The statically linked Microsoft C runtime component vcruntime is distributed under the Visual Studio redistribution terms:"));
  assert.ok(text.includes("The statically linked Microsoft C runtime component UCRT is distributed under the Windows SDK terms:"));
  assert.ok(text.includes("https://visualstudio.microsoft.com/license-terms/vs2026-ga-pro-enterprise/"));
  const sections = noticeSections(text);
  assert.ok(sections.has("C runtime\0Microsoft C runtime"));
  assert.throws(() => validateEvidence(evidence, new Map(), { ...context, target }), /C runtime Microsoft C runtime is not covered/);
  assert.doesNotThrow(() => validateEvidence(evidence, sections, { ...context, target }));
  for (const origin of ["crates.io", "Rust toolchain", "Rust toolchain, crates.io"]) {
    assert.throws(() => noticeSections(text.replace("(C runtime) ===", `(${origin}) ===`)), /malformed third-party notice crate label/);
  }
  for (const other of Object.keys(runtimeManifest.targets).filter(value => value !== target)) {
    assert.ok(!renderRuntimes(other, commit).includes("Microsoft C runtime"));
  }
});

test("C runtime sections cannot cover Rust roots or build output", () => {
  for (const target of ["x86_64-unknown-linux-musl", "wasm32-wasip1"]) {
    const sections = noticeSections(renderRuntimes(target, commit));
    for (const component of runtimeComponents(target, commit)) {
      const root = component.label.split(" ")[0].replaceAll("-", "_");
      for (const kind of ["roots", "buildOut"]) {
        const observed = nativeEvidence(Buffer.alloc(0));
        observed[kind].add(root);
        assert.throws(() => validateEvidence(observed, sections, { ...context, target }), /module carries (?:crate root|build output for crate)/);
        const withRust = noticeSections(renderRuntimes(target, commit) + `\n=== ${root} 1.0.0 (crates.io) ===\nLicense: MIT\n`);
        assert.doesNotThrow(() => validateEvidence(observed, withRust, { ...context, target }));
      }
    }
  }
});

test("LLVM runtime expressions preserve the legacy licence choice", () => {
  for (const name of ["libunwind", "compiler-rt"]) {
    assert.equal(runtimeManifest.components[name].license, "Apache-2.0 WITH LLVM-exception AND (NCSA OR MIT)");
  }
});

test("the runtime inventory covers every shipped target, independently of Cargo features", () => {
  const expected = {
    "x86_64-unknown-linux-musl": ["musl 1.2.5", "libunwind 23.1.1", "compiler-rt 23.1.1"],
    "aarch64-unknown-linux-musl": ["musl 1.2.5", "libunwind 23.1.1", "compiler-rt 23.1.1"],
    "wasm32-wasip1": ["wasi-libc 33.0.0"],
    "wasm32-unknown-unknown": [],
    "aarch64-apple-darwin": [],
    "x86_64-apple-darwin": [],
    "x86_64-pc-windows-msvc": ["Microsoft C runtime"],
  };
  assert.deepEqual(Object.keys(runtimeManifest.targets).sort(), Object.keys(expected).sort());
  for (const [target, labels] of Object.entries(expected)) {
    assert.deepEqual(runtimeComponents(target, commit).map(component => component.label), labels);
    const sections = noticeSections(renderRuntimes(target, commit));
    validateEvidence(evidence, sections, { ...context, target });
    for (const label of labels) {
      const missing = new Map(sections);
      missing.delete("C runtime\0" + label);
      assert.throws(() => validateEvidence(evidence, missing, { ...context, target }), /C runtime .*not covered/);
      const changed = new Map(sections);
      changed.set("C runtime\0" + label, { ...sections.get("C runtime\0" + label), license: "MIT" });
      if (sections.get("C runtime\0" + label).license !== "MIT") assert.throws(() => validateEvidence(evidence, changed, { ...context, target }), /C runtime .*not covered/);
    }
  }
});

test("runtime licence bytes are pinned, including each subordinate WASI notice", () => {
  for (const [target, names] of Object.entries(runtimeManifest.targets)) {
    for (const name of names) {
      for (const path of runtimeManifest.components[name].files) {
        const read = file => file === path ? Buffer.from("changed permission\n") : readFileSync(new URL("../upstream/runtime/" + file, import.meta.url));
        assert.throws(() => renderRuntimes(target, commit, read), /C runtime upstream licence changed/);
      }
    }
  }
  const wasi = renderRuntimes("wasm32-wasip1", commit);
  for (const phrase of ["Rich Felker", "Nuxi", "Regents of the University of California", "LLVM Exceptions", "dlmalloc/"]) assert.ok(wasi.includes(phrase));
  const linux = renderRuntimes("aarch64-unknown-linux-musl", commit);
  for (const phrase of ["Copyright (c) 2012-2020, Arm Limited.", "Copyright (c) 2018, Arm Limited.", "Developed at SunPro", "Developed at SunSoft", "provided that this notice"]) assert.ok(linux.includes(phrase));
});

test("compiler and target changes require a fresh runtime inventory", () => {
  assert.throws(() => renderRuntimes("wasm32-wasip1", "a".repeat(40)), /C runtime inventory is stale/);
  assert.throws(() => renderRuntimes("x86_64-unknown-linux-gnu", commit), /C runtime inventory has no distribution target/);
});
