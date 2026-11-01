import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { basename, join } from "node:path";
import { test } from "node:test";
import { inTreeStatement, permitted, renderNotices, renderToolchain } from "../render.mjs";
import { toolchainCrates, vendoredCrates } from "../select.mjs";

const allowed = new Set(["MIT", "Apache-2.0", "BSD-2-Clause", "Unicode-3.0", "Apache-2.0 WITH LLVM-exception"]);
const policy = { allowed, exceptions: new Map() };
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const registry = "registry+https://github.com/rust-lang/crates.io-index";
const record = (name, dependencies = [], source = false) => `[[package]]\nname = "${name}"\nversion = "1.0.0"\n${source ? `source = "${registry}"\nchecksum = "${"a".repeat(64)}"\n` : ""}dependencies = [${dependencies.map((name) => JSON.stringify(name)).join(",")} ]\n`;

test("the standard-library closure excludes unrelated and unavailable target libraries", () => {
  const lock = record("std", ["alloc", "dlmalloc", "without_rlib"]) + record("alloc", ["core"]) + record("core") + record("panic_abort", ["core"]) + record("dlmalloc", [], true) + record("without_rlib") + record("test", ["getopts"]) + record("getopts");
  const rlibs = ["std", "alloc", "core", "panic_abort", "dlmalloc", "getopts"].map((name) => `lib${name}-${"a".repeat(16)}.rlib`);
  const result = toolchainCrates(lock, rlibs);
  assert.deepEqual(new Set(result.inTree.map((pkg) => pkg.name)), new Set(["std", "alloc", "core", "panic_abort"]));
  assert.deepEqual(result.vendored.map((pkg) => pkg.name), ["dlmalloc"]);
  assert.throws(() => toolchainCrates(lock + record("core"), rlibs), /core: ambiguous/);
  assert.throws(() => toolchainCrates(lock, [...rlibs, `libcore-${"b".repeat(16)}.rlib`]), /core: ambiguous/);
});

test("vendored notice files retain their lock and per-file integrity checks", () => {
  const files = { "LICENSE-MIT": Buffer.from("Upstream MIT permission\n"), "LICENSE-APACHE": Buffer.from("Upstream Apache permission\n") };
  const checksum = { package: "a".repeat(64), files: Object.fromEntries(Object.entries(files).map(([name, bytes]) => [name, hash(bytes)])) };
  const pkg = { name: "dlmalloc", version: "1.0.0", source: registry, checksum: checksum.package };
  const read = (path) => {
    const name = basename(path);
    if (name === ".cargo-checksum.json") return Buffer.from(JSON.stringify(checksum));
    if (name === "Cargo.toml") return Buffer.from('name = "dlmalloc"\nversion = "1.0.0"\nlicense = \'MIT/Apache-2.0\'\n');
    return files[name];
  };
  const list = () => Object.keys(files).map((name) => ({ name, isFile: () => true, isSymbolicLink: () => false }));
  const selected = vendoredCrates("/library", [pkg], "/repo", read);
  assert.match(renderNotices(selected, policy, "", list, (dir, name) => read(join(dir, name))), /dlmalloc 1.0.0 \(Rust toolchain, crates.io\)/);
  assert.throws(() => vendoredCrates("/library", [{ ...pkg, checksum: "stale" }], "/repo", read), /dlmalloc 1.0.0: vendored checksum/);
  assert.throws(() => vendoredCrates("/library", [pkg], "/repo", () => { throw new Error("absent"); }), /dlmalloc 1.0.0: vendored sources missing/);
  files["LICENSE-MIT"] = Buffer.from("changed upstream file\n");
  assert.throws(() => renderNotices(selected, policy, "", list, (dir, name) => read(join(dir, name))), /dlmalloc 1.0.0: LICENSE-MIT differs/);
});

test("legacy Cargo licence alternatives preserve the existing SPDX choice semantics", () => {
  assert.equal(permitted("MIT/Apache-2.0", allowed), true);
  assert.equal(permitted("MIT/GPL-3.0", allowed), true);
  assert.equal(permitted("GPL-3.0/GPL-2.0", allowed), false);
  assert.equal(permitted("MIT AND Apache-2.0 WITH LLVM-exception AND (MIT OR Apache-2.0)", allowed), true);
  assert.equal(permitted("MIT AND GPL-3.0", allowed), false);
});

const statement = '<h2 id="in-tree-files">In-tree files</h2>\n<p>License: Apache-2.0 OR MIT</p>\n<p>Copyright: Rust Project Developers</p>\n<p>License: Unicode-3.0</p>\n<p>&#34;quoted&#34; &#39;notice&#39; &#60;value&#62;</p>\n<h2 id="out-of-tree-dependencies">Other crates</h2>';

test("toolchain WITH expressions retain exception text without reading an operator file", () => {
  const tc = { version: "1.2.3", commit: "a".repeat(40), sysroot: "/toolchain", library: "/library" };
  const crates = ["std", "core", "alloc"].map((name) => ({ name, version: "0.0.0", license: "MIT OR Apache-2.0" }));
  const html = statement.replace("License: Unicode-3.0", "License: Apache-2.0 WITH LLVM-exception");
  const files = new Map([["/toolchain/share/doc/rust/COPYRIGHT-library.html", Buffer.from(html)]]);
  for (const name of ["Apache-2.0", "MIT", "LLVM-exception"]) files.set(`/toolchain/share/doc/rust/licenses/${name}.txt`, Buffer.from(`${name} upstream licence\n`));
  const requested = [];
  const read = (path) => {
    path = path.replaceAll("\\", "/");
    requested.push(path);
    if (!files.has(path)) throw new Error("absent");
    return files.get(path);
  };
  const text = renderToolchain(tc, crates, policy, read);
  assert.match(text, /--- licenses\/LLVM-exception.txt ---\nLLVM-exception upstream licence/);
  assert.equal(requested.some((path) => path.endsWith("/WITH.txt")), false);
  files.delete("/toolchain/share/doc/rust/licenses/LLVM-exception.txt");
  assert.throws(() => renderToolchain(tc, crates, policy, read), /licenses\/LLVM-exception.txt/);
});

test("Rust copyright extraction retains statement text and refuses unknown markup entities", () => {
  assert.equal(inTreeStatement(statement), 'In-tree files\nLicense: Apache-2.0 OR MIT\nCopyright: Rust Project Developers\nLicense: Unicode-3.0\n"quoted" \'notice\' <value>\n');
  assert.throws(() => inTreeStatement(statement.replace('id="in-tree-files"', 'id="missing"')), /boundaries/);
  assert.throws(() => inTreeStatement(statement.replace("&#34;quoted&#34;", "&amp;")), /unknown copyright HTML entity/);
});

test("standard-library notices include copyright and every named licence text", () => {
  const tc = { version: "1.2.3", commit: "a".repeat(40), sysroot: "/toolchain", library: "/library" };
  const crates = ["std", "core", "alloc"].map((name) => ({ name, version: "0.0.0", license: "MIT OR Apache-2.0" }));
  const files = new Map([["/toolchain/share/doc/rust/COPYRIGHT-library.html", Buffer.from(statement)]]);
  for (const name of ["Apache-2.0", "MIT", "Unicode-3.0"]) files.set(`/toolchain/share/doc/rust/licenses/${name}.txt`, Buffer.from(`${name} upstream licence\n`));
  const read = (path) => { path = path.replaceAll("\\", "/"); if (!files.has(path)) throw new Error("absent"); return files.get(path); };
  assert.match(renderToolchain(tc, crates, policy, read), /Crates: alloc, core, std/);
  files.delete("/toolchain/share/doc/rust/licenses/Unicode-3.0.txt");
  assert.throws(() => renderToolchain(tc, crates, policy, read), /licenses\/Unicode-3.0.txt from Rust 1.2.3/);
  files.set("/toolchain/share/doc/rust/licenses/Unicode-3.0.txt", Buffer.from("Unicode permission\n"));
  assert.throws(() => renderToolchain(tc, [...crates, { name: "unlisted", version: "1.0.0", license: "MIT" }], policy, read), /in-tree crate unlisted.*IN_TREE_FILES/);
  const builtins = { name: "compiler_builtins", version: "0.1.160", license: "MIT AND Apache-2.0 WITH LLVM-exception AND (MIT OR Apache-2.0)" };
  assert.throws(() => renderToolchain(tc, [...crates, builtins], policy, read), /compiler_builtins 0.1.160 compiler-builtins\/LICENSE.txt/);
  files.set("/library/compiler-builtins/LICENSE.txt", Buffer.from("Compiler builtins permission\n"));
  assert.throws(() => renderToolchain(tc, [...crates, builtins], policy, read), /compiler_builtins.*libm\/LICENSE.txt/);
  files.set("/library/compiler-builtins/libm/LICENSE.txt", Buffer.from("Bundled math permission\n"));
  assert.match(renderToolchain(tc, [...crates, builtins], policy, read), /Bundled math permission/);
});
