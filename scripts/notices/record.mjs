import { createHash } from "node:crypto";
import { existsSync, lstatSync, readFileSync, readdirSync } from "node:fs";
import { isAbsolute, join, relative, resolve } from "node:path";

export const NOTICE_FILE = "THIRD-PARTY-NOTICES.txt";
export const RECORD_FILE = "THIRD-PARTY-NOTICES.record.json";
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
const normal = path => path.replaceAll("\\", "/");

function localPath(path) {
  return typeof path === "string" && path.length > 0 && !isAbsolute(path) && !path.includes("\\") && !/[\x00-\x1f\x7f:]/.test(path) && path.split("/").every(part => part && part !== "." && part !== "..");
}

function inputFiles(repo, directory) {
  const found = [];
  for (const entry of readdirSync(join(repo, directory), { withFileTypes: true })) {
    const path = directory + "/" + entry.name;
    if (entry.isSymbolicLink()) throw new Error("notice input is not a regular file or directory: " + path);
    if (entry.isDirectory()) found.push(...inputFiles(repo, path));
    else if (entry.isFile()) found.push(path);
  }
  return found;
}

export function buildScript(distribution) {
  if (!distribution || typeof distribution.target !== "string" || typeof distribution.profile !== "string" || !/^[a-z0-9_-]+$/.test(distribution.target) || !/^[a-z0-9_-]+$/.test(distribution.profile) || typeof distribution.defaultFeatures !== "boolean" || !Array.isArray(distribution.features) || !distribution.features.every(value => typeof value === "string" && /^[A-Za-z0-9_./:-]+$/.test(value))) throw new Error("choose a supported Rust distribution");
  if (distribution?.package === "coreav-wasm" && distribution.target === "wasm32-unknown-unknown") return "packages/wasm/scripts/build.sh";
  if (distribution?.package === "coreav-cli" && distribution.target === "wasm32-wasip1") return "packages/cli/scripts/build.sh";
  if (distribution?.package === "coreav-cli" && /^[a-z0-9_-]+$/.test(distribution.target)) return "packages/cli/scripts/platform.sh";
  throw new Error("choose a supported Rust distribution");
}

export function requiredInputs(repo, distribution) {
  const paths = new Set(["Cargo.lock", "Cargo.toml", "deny.toml", "NOTICE", "scripts/rust-dist.env", buildScript(distribution)]);
  for (const directory of ["crates", "tools"]) {
    if (!existsSync(join(repo, directory))) continue;
    for (const entry of readdirSync(join(repo, directory), { withFileTypes: true })) {
      if (entry.isDirectory() && existsSync(join(repo, directory, entry.name, "Cargo.toml"))) paths.add(directory + "/" + entry.name + "/Cargo.toml");
    }
  }
  for (const entry of readdirSync(join(repo, "scripts/notices"), { withFileTypes: true })) {
    if (entry.isFile() && entry.name.endsWith(".mjs")) paths.add("scripts/notices/" + entry.name);
  }
  if (existsSync(join(repo, "scripts/notices/upstream"))) for (const path of inputFiles(repo, "scripts/notices/upstream")) paths.add(path);
  return [...paths].sort();
}

function readRegular(directory, path) {
  if (!localPath(path)) throw new Error("notice record has an unsafe relative path");
  let file = directory;
  const parts = path.split("/");
  for (let index = 0; index < parts.length; index += 1) {
    file = join(file, parts[index]);
    const stat = lstatSync(file);
    if (stat.isSymbolicLink() || (index === parts.length - 1 ? !stat.isFile() : !stat.isDirectory())) throw new Error("notice input is not a regular file: " + path);
  }
  return readFileSync(file);
}

function artifactPath(distribution, path) {
  const expected = distribution.package === "coreav-wasm" ? ["wasm/coreav_bg.wasm"] : distribution.target === "wasm32-wasip1" ? ["wasm/coreav.wasm"] : ["bin/coreav", "bin/coreav.exe"];
  if (!expected.includes(path)) throw new Error("notice record names an unexpected distribution artifact");
  return path;
}

export function makeRecord(directory, repo, distribution, toolchain, artifact, raw, text, sections, closure) {
  const path = normal(relative(directory, resolve(artifact)));
  artifactPath(distribution, path);
  const inputs = Object.fromEntries(requiredInputs(repo, distribution).map(name => [name, sha(readRegular(repo, name))]));
  const record = {
    format: 1, distribution,
    toolchain: { version: toolchain.version, commit: toolchain.commit },
    inputs, notices: sha(Buffer.from(text)),
    artifact: { path, sha256: sha(readRegular(directory, path)) },
    sections: Object.fromEntries(["crates.io", "Rust toolchain", "Rust toolchain, crates.io"].map(origin => [origin, [...sections.values()].filter(section => section.origin === origin).length])),
    toolchainCrates: { inTree: closure.inTree.map(crate => crate.name).sort(), vendored: closure.vendored.map(crate => crate.name + " " + crate.version).sort() },
  };
  if (raw) {
    const path = normal(relative(repo, resolve(raw)));
    record.raw = { path, sha256: sha(readRegular(repo, path)) };
  }
  return record;
}

export function verifyRecord(directory, repo) {
  const fail = message => { throw new Error("run npm run build: " + NOTICE_FILE + " " + message); };
  let record;
  try { record = JSON.parse(readRegular(directory, RECORD_FILE)); }
  catch { fail("has no build record"); }
  if (record?.format !== 1) fail("has no supported build record");
  if (!Array.isArray(record.distribution?.rustflags) || !record.distribution.rustflags.every(flag => typeof flag === "string" && !flag.startsWith("--remap-path-prefix"))) fail("has invalid build flags");
  if (record.distribution?.target?.startsWith("wasm32-") && (!localPath(record.raw?.path) || !/^[a-f0-9]{64}$/.test(record.raw?.sha256))) fail("has no raw module provenance");
  let paths;
  try {
    paths = requiredInputs(repo, record.distribution);
    artifactPath(record.distribution, record.artifact?.path);
  } catch { fail("has an invalid distribution record"); }
  try {
    if (sha(readRegular(directory, NOTICE_FILE)) !== record.notices) fail("differs from the one the build generated");
    if (sha(readRegular(directory, record.artifact.path)) !== record.artifact.sha256) fail("artifact changed after generation");
    if (!record.inputs || JSON.stringify(Object.keys(record.inputs).sort()) !== JSON.stringify(paths)) fail("inputs changed");
    for (const path of paths) if (sha(readRegular(repo, path)) !== record.inputs[path]) fail("is stale: " + path + " changed");
    const pin = readRegular(repo, "scripts/rust-dist.env").toString("utf8");
    if (record.toolchain?.version !== /^RUST_DIST_TOOLCHAIN=([0-9.]+)$/m.exec(pin)?.[1] || record.toolchain?.commit !== /^RUST_DIST_COMMIT=([a-f0-9]{40})$/m.exec(pin)?.[1]) fail("Rust pin changed");
  } catch (error) {
    if (error.message.startsWith("run npm run build:")) throw error;
    fail("has missing or unreadable build inputs");
  }
  return record;
}
