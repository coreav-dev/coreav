import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { inTreeCrates, vendoredCrates } from "../select.mjs";
import { renderNotices, renderToolchain } from "../render.mjs";

const policy = { allowed: new Set(["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception"]), exceptions: new Map() };

test("in-tree discovery maps hyphenated package identities and refuses ambiguous manifests", () => {
  const library = mkdtempSync(join(tmpdir(), "coreav-library-"));
  const put = (path, text) => { mkdirSync(dirname(join(library, path)), { recursive: true }); writeFileSync(join(library, path), text); };
  try {
    put("windows_link/Cargo.toml", '[package]\nname = "windows-link"\nversion = "0.0.0"\n');
    const result = inTreeCrates(library, [{ name: "windows-link", version: "0.0.0" }]);
    assert.equal(result[0].dir, "windows_link");
    assert.equal(result[0].license, null);
    put("windows-link/Cargo.toml", '[package]\nname = "windows-link"\nversion = "0.0.0"\n');
    assert.throws(() => inTreeCrates(library, [{ name: "windows-link", version: "0.0.0" }]), /windows-link.*ambiguous/);
  } finally { rmSync(library, { recursive: true, force: true }); }
});

test("a licence-less in-tree crate needs the authoritative root statement with no directory exception", () => {
  const tc = { version: "1.2.3", commit: "a".repeat(40), sysroot: "/toolchain", library: "/library" };
  const crates = ["std", "core", "alloc"].map(name => ({ name, version: "0.0.0", license: "MIT OR Apache-2.0", dir: name }));
  crates.push({ name: "windows-link", version: "0.0.0", license: null, dir: "windows_link" });
  let statement = '<h2 id="in-tree-files">\n<p>File/Directory: .</p>\n<p>License: Apache-2.0 OR MIT</p>\n<h2 id="out-of-tree-dependencies">';
  const read = path => Buffer.from(path.endsWith(".html") ? statement : "upstream permission\n");
  assert.match(renderToolchain(tc, crates, policy, read), /windows-link/);
  statement = statement.replace("File/Directory: .", "File/Directory: library/core");
  assert.throws(() => renderToolchain(tc, crates, policy, read), /missing licence metadata/);
  statement = '<h2 id="in-tree-files">\nFile/Directory: .\nLicense: Apache-2.0 OR MIT\nFile/Directory: library/windows_link\nLicense: MIT\n<h2 id="out-of-tree-dependencies">';
  assert.throws(() => renderToolchain(tc, crates, policy, read), /missing licence metadata/);
});
