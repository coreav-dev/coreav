import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { licensePolicy, licenseTerms, renderNotices } from "../render.mjs";

const policy = { allowed: new Set(["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "ISC", "BSD-3-Clause"]), exceptions: new Map() };

function render(name, license, files, selectedPolicy = policy, notice = "", version = "1.0.0") {
  const list = directory => {
    const prefix = directory.slice("/upstream".length).replace(/^\//, "");
    const relative = Object.keys(files).filter(path => !prefix || path.startsWith(prefix + "/")).map(path => prefix ? path.slice(prefix.length + 1) : path);
    return [...new Set(relative.map(path => path.split("/")[0]))].map(name => ({
      name, isSymbolicLink: () => false,
      isFile: () => Object.hasOwn(files, prefix ? prefix + "/" + name : name),
      isDirectory: () => relative.some(path => path.startsWith(name + "/")),
    }));
  };
  return renderNotices([{ name, version, license, dir: "/upstream" }], selectedPolicy, notice, list, (directory, name) => Buffer.from(files[name]));
}

test("native licence expressions keep WITH terms and accept covered alternatives", () => {
  assert.deepEqual(licenseTerms("Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT"), ["Apache-2.0 WITH LLVM-exception", "Apache-2.0", "MIT"]);
  assert.match(render("rustix", "Apache-2.0 WITH LLVM-exception OR MIT", { "LICENSE-Apache-2.0_WITH_LLVM-exception": "LLVM upstream permission\n" }), /LLVM upstream permission/);
  assert.match(render("tls", "MIT OR ISC", { "LICENSE-ISC": "ISC permission\n" }), /ISC permission/);
  assert.match(render("codec", "BSD-3-Clause", { LICENSE: "BSD permission\n" }), /BSD permission/);
  assert.match(render("choice", "MIT OR Unknown", { "LICENSE-MIT": "MIT permission\n" }), /MIT permission/);
  assert.throws(() => render("both", "MIT AND ISC", { "LICENSE-MIT": "MIT permission\n" }), /both 1.0.0: missing.*ISC/);
});

test("ring text coverage is version keyed and includes nested permissions without borrowing them", () => {
  const files = {
    LICENSE: "See LICENSE-BoringSSL and LICENSE-other-bits\n",
    "LICENSE-BoringSSL": "Apache permission\n",
    "LICENSE-other-bits": "ISC permission\n",
    "third_party/fiat/LICENSE": "Fiat permission\n",
    "src/polyfill/once_cell/LICENSE-MIT": "Copied permission\n",
  };
  const text = render("ring", "Apache-2.0 AND ISC", files, policy, "", "0.17.14");
  assert.match(text, /--- third_party\/fiat\/LICENSE ---\nFiat permission/);
  assert.match(text, /--- src\/polyfill\/once_cell\/LICENSE-MIT ---\nCopied permission/);
  assert.ok(text.indexOf("--- LICENSE-other-bits ---") < text.indexOf("--- third_party/fiat/LICENSE ---"));
  assert.throws(() => render("ring", "Apache-2.0 AND ISC", files, policy, "", "0.17.15"), /ring 0.17.15: add a CRATE_TERM_FILES row/);
  delete files["LICENSE-other-bits"];
  assert.throws(() => render("ring", "Apache-2.0 AND ISC", files, policy, "", "0.17.14"), /missing.*ISC/);
});

test("the IJG term requires both the existing scoped policy and the root notice sentence", () => {
  const files = { "LICENSE-MIT": "MIT permission\n", "LICENSE-APACHE": "Apache permission\n" };
  const expression = "(MIT OR Apache-2.0) AND IJG";
  const notice = "This software is based in part on the work of the Independent JPEG Group";
  const scoped = { ...policy, exceptions: new Map([["jpeg-encoder", new Set(["IJG"])]]) };
  assert.throws(() => render("jpeg-encoder", expression, files, policy, notice), /unapproved/);
  assert.throws(() => render("jpeg-encoder", expression, files, scoped), /missing.*IJG/);
  assert.match(render("jpeg-encoder", expression, files, scoped, notice), /IJG: see NOTICE/);
  assert.throws(() => render("another", expression, files, scoped, notice), /unapproved/);
  assert.equal(policy.allowed.has("IJG"), false);
});

test("policy exceptions cannot broaden another crate or silently ignore a new restriction", () => {
  const directory = mkdtempSync(join(tmpdir(), "coreav-policy-"));
  try {
    const base = '[licenses]\nallow = ["MIT"] # "GPL-3.0" is not an allowance\n[[licenses.exceptions]]\ncrate = "jpeg-encoder"\nallow = ["IJG"]\n';
    writeFileSync(join(directory, "deny.toml"), base);
    const parsed = licensePolicy(directory);
    assert.deepEqual(parsed.allowed, new Set(["MIT"]));
    assert.deepEqual(parsed.exceptions, new Map([["jpeg-encoder", new Set(["IJG"])]]));
    writeFileSync(join(directory, "deny.toml"), base + 'version = "0.6.1"\n');
    assert.throws(() => licensePolicy(directory), /unsupported policy needs a generator update/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
