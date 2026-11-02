import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { licensePolicy, permitted } from "../../../scripts/notices/render.mjs";
import { fileURLToPath } from "node:url";
import {
  archiveEntries, consumerEnv, PACKED, scanPaths, SIZE_BAR, validateArchive,
  validateCompressedSize, validateManifest, validateProvenance,
} from "../scripts/check-package.mjs";

const allowed = new Set(["MIT", "Apache-2.0"]);

test("locked package verification tools satisfy the repository licence policy", () => {
  const policy = licensePolicy(fileURLToPath(new URL("../../..", import.meta.url))).allowed;
  const lock = JSON.parse(readFileSync(new URL("../package-lock.json", import.meta.url)));
  for (const [name, entry] of Object.entries(lock.packages)) assert.ok(permitted(entry.license, policy), `${name || "package"}: unapproved licence`);
});
const manifest = () => ({
  name: "@coreav/wasm", version: "0.0.1", description: "Media operations",
  license: "MIT OR Apache-2.0", repository: { type: "git", url: "https://example.invalid/repo.git" },
  private: true, type: "module",
  exports: {
    ".": { types: "./src/index.d.ts", default: "./src/index.js" },
    "./workers": { types: "./src/workers.d.ts", default: "./src/workers.js" },
    "./node": { types: "./src/node.d.ts", default: "./src/node.js" },
  },
  files: ["src", "wasm/coreav.js", "wasm/build.js", "wasm/coreav_bg.wasm", "README.md", "LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD-PARTY-NOTICES.txt"],
  engines: { node: ">=24" },
  scripts: { build: "build", test: "test", "test:module": "test", "test:types": "test", "check:package": "check", prepack: "check" },
  devDependencies: { typescript: "5.9.3" },
});

function roster() {
  return PACKED.map(path => ({ path, mode: 0o644, size: 1, bytes: Buffer.from("x") }));
}

function tarRecord(path, { type = "0", mode = 0o644, bytes = Buffer.from("x"), link = "", prefix = "" } = {}) {
  const header = Buffer.alloc(512);
  const number = (at, length, value) => header.write(`${value.toString(8).padStart(length - 1, "0")}\0`, at, length, "ascii");
  header.write(path, 0, 100);
  number(100, 8, mode);
  number(108, 8, 0);
  number(116, 8, 0);
  number(124, 12, bytes.length);
  number(136, 12, 0);
  header.fill(32, 148, 156);
  header.write(type, 156, 1);
  header.write(link, 157, 100);
  header.write("ustar\0", 257, 6);
  header.write("00", 263, 2);
  header.write(prefix, 345, 155);
  const checksum = header.reduce((sum, byte) => sum + byte, 0);
  header.write(`${checksum.toString(8).padStart(6, "0")}\0 `, 148, 8);
  return Buffer.concat([header, bytes, Buffer.alloc((512 - bytes.length % 512) % 512)]);
}

function archive(...records) {
  return gzipSync(Buffer.concat([...records, Buffer.alloc(1024)]));
}

test("the exact 21-file archive roster includes upstream notices", () => {
  assert.equal(PACKED.length, 21);
  const files = validateArchive(roster());
  assert.equal(files.size, 21);
  const parsed = archiveEntries(archive(...roster().map(entry => tarRecord(entry.path))));
  assert.deepEqual(validateArchive(parsed), files);
  assert.throws(() => validateArchive(roster().slice(1)), /roster/);
  assert.throws(() => validateArchive([...roster(), roster()[0]]), /duplicate/);
  assert.throws(() => validateArchive([...roster(), { ...roster()[0], path: "package/src/new.js" }]), /unexpected packed file/);
});

test("archive metadata refuses escapes, private files, links, special types and unsafe modes", () => {
  for (const path of ["/package/NOTICE", "package/../NOTICE", "package/./NOTICE", "package//NOTICE", "package/NOTICE\0other", "package/NOTICE\n", "package\\NOTICE", "elsewhere/NOTICE"]) {
    assert.throws(() => validateArchive([{ ...roster()[0], path }, ...roster().slice(1)]));
  }
  for (const path of ["package/.env", "package/.dev.vars", "package/wrangler.jsonc", "package/node_modules/tool.js", "package/input.tgz", "package/test/private.mjs", "package/examples/file.js", "package/scripts/build.sh"]) {
    assert.throws(() => validateArchive([...roster(), { ...roster()[0], path }]), /forbidden packed file/);
  }
  for (const mode of [0o755, 0o664, 0o646, 0o4644, 0o2644, 0o1644, -1, 1.5]) {
    assert.throws(() => validateArchive([{ ...roster()[0], mode }, ...roster().slice(1)]), /unsafe archive mode/);
  }
  assert.throws(() => validateArchive([{ ...roster()[0], size: 2 }, ...roster().slice(1)]), /incorrect archive size/);
});

test("ustar decoding checks checksums, padding, termination and link targets", () => {
  const header = tarRecord("package/NOTICE");
  const badChecksum = Buffer.from(header);
  badChecksum[0] ^= 1;
  assert.throws(() => archiveEntries(archive(badChecksum)), /checksum/);
  assert.throws(() => archiveEntries(gzipSync(header)), /end blocks/);
  assert.throws(() => archiveEntries(gzipSync(Buffer.concat([header.subarray(0, 512), Buffer.alloc(1024)]))), /zero blocks|padding|end blocks/);
  const padding = Buffer.from(header);
  padding[513] = 1;
  assert.throws(() => archiveEntries(archive(padding)), /padding/);
  assert.throws(() => archiveEntries(archive(tarRecord("package/NOTICE", { link: "/outside" }))), /link targets/);
  assert.throws(() => archiveEntries(gzipSync(Buffer.concat([header, Buffer.alloc(1024), Buffer.from("hidden")]))), /zero blocks/);
  assert.throws(() => archiveEntries(archive(tarRecord("NOTICE", { prefix: "../package" }))), /inside package|traversing/);
});

test("only regular npm pack records are accepted", () => {
  for (const type of ["x", "g", "L", "K", "5", "1", "2", "3", "4", "6", "\0"]) {
    assert.throws(() => archiveEntries(archive(tarRecord("package/NOTICE", { type }))), /only regular files/);
  }
  assert.throws(() => archiveEntries(gzipSync(Buffer.alloc((64 << 20) + 1))), /larger than/);
});

test("the packed manifest permits only the approved private export surface and no install hooks", () => {
  assert.doesNotThrow(() => validateManifest(manifest(), allowed));
  for (const key of ["dependencies", "peerDependencies", "optionalDependencies", "bundleDependencies", "bundledDependencies", "publishConfig", "bin", "main"]) {
    assert.throws(() => validateManifest({ ...manifest(), [key]: {} }, allowed), /manifest fields/);
  }
  for (const key of ["preinstall", "install", "postinstall", "prepare", "prepublish", "preprepare", "postprepare"]) {
    const value = manifest();
    value.scripts[key] = "node bad.mjs";
    assert.throws(() => validateManifest(value, allowed), /lifecycle/);
  }
  assert.throws(() => validateManifest({ ...manifest(), private: false }, allowed), /private/);
  assert.throws(() => validateManifest({ ...manifest(), type: "commonjs" }, allowed));
  assert.throws(() => validateManifest({ ...manifest(), engines: { node: ">=22" } }, allowed));
  assert.throws(() => validateManifest({ ...manifest(), license: "GPL-3.0" }, allowed));
  assert.throws(() => validateManifest(manifest(), new Set()), /not permitted/);
  assert.throws(() => validateManifest({ ...manifest(), files: manifest().files.filter(file => file !== "THIRD-PARTY-NOTICES.txt") }, allowed));
  const deep = manifest();
  deep.exports["./src/*"] = "./src/*";
  assert.throws(() => validateManifest(deep, allowed), /public exports/);
  const wrong = manifest();
  wrong.exports["./workers"].default = "./src/node.js";
  assert.throws(() => validateManifest(wrong, allowed), /public exports/);
});

test("provenance and fingerprint describe the exact optimized SIMD module", () => {
  const wasm = Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]);
  const hash = createHash("sha256").update(wasm).digest("hex");
  const fingerprint = Buffer.from(`export const BUILD = "${hash}";\n`);
  const pins = { binaryen: "133", bindgen: "0.2.129", rust: "1.99.1" };
  const provenance = { wasmSha256: hash, rawWasmSha256: hash, rustflags: ["-C", "target-feature=+simd128"], wasmOpt: "wasm-opt version 133 (version_133)", wasmBindgen: pins.bindgen, profile: "edge", target: "wasm32-unknown-unknown", rustc: "rustc 1.99.1 (build date)", simdInstructions: 1 };
  const verify = (data, module, identity, pins) => {
    const { rawWasmSha256, rustflags, ...provenance } = data;
    return validateProvenance(provenance, module, identity, { ...pins, notices: { raw: { sha256: rawWasmSha256 }, distribution: { rustflags } } });
  };
  assert.equal(verify(provenance, wasm, fingerprint, pins), hash);
  assert.throws(() => verify(provenance, Buffer.concat([wasm, Buffer.from([0])]), fingerprint, pins), /fingerprint/);
  assert.throws(() => verify({ ...provenance, wasmSha256: "stale" }, wasm, fingerprint, pins), /provenance/);
  for (const wasmOpt of [null, "wasm-opt version 132", "wasm-opt version 1330", "prefix wasm-opt version 133"]) {
    assert.throws(() => verify({ ...provenance, wasmOpt }, wasm, fingerprint, pins), /pinned wasm-opt/);
  }
  for (const [key, value] of [["wasmBindgen", "different"], ["profile", "release"], ["target", "wasm32-wasip1"], ["rustc", ""], ["rustc", "rustc 1.99.0 (build date)"], ["rawWasmSha256", "stale"], ["rustflags", []], ["rustflags", ["-C", "target-feature=+simd128", "--remap-path-prefix=/private=/r"]], ["simdInstructions", null], ["simdInstructions", 0], ["simdInstructions", 1.5]]) {
    assert.throws(() => verify({ ...provenance, [key]: value }, wasm, fingerprint, pins));
  }
});

test("the gzip gate has the exact approved byte boundary and no raw-size gate", () => {
  assert.doesNotThrow(() => validateCompressedSize(100 * SIZE_BAR, SIZE_BAR));
  assert.throws(() => validateCompressedSize(100 * SIZE_BAR, SIZE_BAR + 1), /gzip -9 -n/);
  assert.throws(() => validateCompressedSize(0, 0), /invalid WASM size/);
});

test("local path scans cover all text and printable WASM strings without disclosing the matched path", () => {
  const prefixes = { home: "/fixture/home/person", repo: "/fixture/checkout" };
  for (const marker of [prefixes.home, prefixes.repo, "/home/runner/work", "/home/someone/src", "/Users/person/src", "C:\\Users\\person", "C:/Users/person"]) {
    for (const path of ["package/NOTICE", "package/wasm/coreav_bg.wasm"]) {
      assert.throws(() => scanPaths(new Map([[path, Buffer.concat([Buffer.from([0]), Buffer.from(marker), Buffer.from([0])])]]), prefixes), error => {
        assert.match(error.message, /path prefix/);
        assert.ok(!error.message.includes(marker));
        return true;
      });
    }
  }
  assert.doesNotThrow(() => scanPaths(new Map([["package/NOTICE", Buffer.from("/cargo/registry and /coreav are remapped build roots")]]), prefixes));
});

test("consumer environments cannot inherit npm, Node resolution or injected runtime options", () => {
  const env = consumerEnv({ npm_config_registry: "https://invalid.invalid", NPM_CONFIG_PREFIX: "/bad", npm_execpath: "/bad/npm.js", INIT_CWD: "/repo", NODE_PATH: "/repo/node_modules", NODE_OPTIONS: "--import=/bad.mjs", PATH: "/repo/node_modules/.bin:/usr/bin", SAFE: "value" });
  for (const key of ["NPM_CONFIG_PREFIX", "npm_execpath", "INIT_CWD", "NODE_PATH", "NODE_OPTIONS", "npm_config_registry"]) assert.ok(!Object.hasOwn(env, key));
  assert.doesNotMatch(env.PATH, /node_modules[\\/]\.bin/);
  assert.equal(env.npm_config_offline, "true");
  assert.equal(env.SAFE, "value");
});
