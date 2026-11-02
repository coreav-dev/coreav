import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { basename, join } from "node:path";
import { test } from "node:test";
import { inTreeStatement, permitted, renderNotices, renderToolchain } from "../render.mjs";
import { toolchainCrates, vendoredCrates } from "../select.mjs";

const allowed = new Set(["MIT", "Apache-2.0", "BSD-2-Clause", "Unicode-3.0", "Apache-2.0 WITH LLVM-exception"]);
const policy = { allowed, exceptions: new Map() };
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const registry = "registry+https://github.com/rust-lang/crates.io-index";
const llvmException = readFileSync(new URL("./fixtures/LLVM-exception.txt", import.meta.url), "utf8");
const record = (name, dependencies = [], source = false) => `[[package]]\nname = "${name}"\nversion = "1.0.0"\n${source ? `source = "${registry}"\nchecksum = "${"a".repeat(64)}"\n` : ""}dependencies = [${dependencies.map((name) => JSON.stringify(name)).join(",")} ]\n`;

test("registry WITH terms retain their exception text and single-term licence coverage", () => {
  const render = (license, files) => renderNotices([{ name: "exception", version: "1.0.0", license, dir: "/upstream" }], policy, "",
    () => Object.keys(files).map((name) => ({ name, isFile: () => true, isSymbolicLink: () => false })),
    (_dir, name) => Buffer.from(files[name]));
  const expression = "Apache-2.0 WITH LLVM-exception";
  const permission = "Apache permission\nLLVM exception permission\n" + llvmException;
  assert.match(render(expression, { LICENSE: permission }), /LLVM exception permission/);
  const named = { "LICENSE-Apache-2.0_WITH_LLVM-exception": permission };
  assert.match(render(expression, named), /License: Apache-2.0 WITH LLVM-exception/);
  assert.match(render(expression + " OR MIT", named), /LLVM exception permission/);
  assert.match(render(expression + " AND MIT", { ...named, "LICENSE-MIT": "MIT permission\n" }), /MIT permission/);
  assert.match(render("Apache-2.0\tWITH\nLLVM-exception", { "COPYING.txt": permission }), /LLVM exception permission/);
  assert.throws(() => render(expression, { "LICENSE-APACHE": "Apache permission\n" }), /missing upstream licence file/);
  assert.throws(() => render(expression + " AND MIT", { "LICENSE-APACHE": "Apache permission\n", "LICENSE-MIT": "MIT permission\n" }), /missing upstream licence file.*Apache-2.0 WITH LLVM-exception/);
  assert.throws(() => render(expression + " AND MIT", { LICENSE: permission, "LICENSE-MIT": "MIT permission\n" }), /missing upstream licence file.*Apache-2.0 WITH LLVM-exception/);
  assert.throws(() => render(expression + " AND MIT", named), /missing upstream licence file.*MIT/);
});

test("generic compound licence files must contain the exception text", () => {
  const expression = "Apache-2.0 WITH LLVM-exception";
  const base = readFileSync(new URL("../../../LICENSE-APACHE", import.meta.url), "utf8");
  for (const name of ["LICENSE", "COPYING", "LICENSE.txt", "COPYING.md", "LICENSE-Apache-2.0_WITH_LLVM-exception"]) {
    const render = (text) => renderNotices([{ name: "exception", version: "1.0.0", license: expression, dir: "/upstream" }], policy, "",
      () => [{ name, isFile: () => true, isSymbolicLink: () => false }], () => Buffer.from(text));
    assert.throws(() => render(base), /missing.*LLVM-exception/);
    assert.throws(() => render(base + "\nLLVM Exceptions to the Apache 2.0 License\n"), /missing.*LLVM-exception/);
    assert.throws(() => render(base + "\nSPDX-License-Identifier: " + expression), /missing.*LLVM-exception/);
    for (const partial of llvmException.split("\n\n")) assert.throws(() => render(base + partial), /missing.*LLVM-exception/);
    assert.throws(() => render(base + llvmException.replace("may redistribute", "must redistribute")), /missing.*LLVM-exception/);
    assert.ok(render(base + llvmException).includes(llvmException));
    const wrapped = llvmException.replace(/\s+/g, "\r\n\t ");
    assert.ok(render(base + wrapped).replace(/\s+/g, " ").includes(llvmException.replace(/\s+/g, " ").trim()));
  }
  const render = (license, files) => renderNotices([{ name: "exception", version: "1.0.0", license, dir: "/upstream" }], policy, "",
    () => Object.keys(files).map((name) => ({ name, isFile: () => true, isSymbolicLink: () => false })),
    (_dir, name) => Buffer.from(files[name]));
  assert.match(render("Apache-2.0", { LICENSE: base }), /Apache License/);
  const alternatives = { "LICENSE-Apache-2.0_WITH_LLVM-exception": base, "LICENSE-MIT": "MIT permission\n" };
  assert.match(render(expression + " OR MIT", alternatives), /MIT permission/);
  delete alternatives["LICENSE-MIT"];
  assert.throws(() => render(expression + " OR MIT", alternatives), /missing.*LLVM-exception/);
});

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
