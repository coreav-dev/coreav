import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { moduleEvidence, noticeSections, validateEvidence } from "./module-evidence.mjs";

/** Evaluate SPDX alternatives against the repository's explicit licence choices. */
export function permitted(text, allowed) {
  if (typeof text !== "string") return false;
  text = text.replaceAll("/", " OR ");
  const tokens = text.match(/\(|\)|[A-Za-z0-9.+-]+/g) ?? [];
  if (tokens.join("") !== text.replace(/\s/g, "")) return false;
  let at = 0;
  function atom() {
    if (tokens[at] === "(") {
      at += 1;
      const value = either();
      if (tokens[at++] !== ")") throw new Error("unbalanced licence expression");
      return value;
    }
    let name = tokens[at++];
    if (!name || ["AND", "OR", ")", "WITH"].includes(name)) throw new Error("invalid licence expression");
    if (tokens[at] === "WITH") { at += 1; name += ` WITH ${tokens[at++]}`; }
    return allowed.has(name);
  }
  function both() {
    let value = atom();
    while (tokens[at] === "AND") { at += 1; const right = atom(); value = value && right; }
    return value;
  }
  function either() {
    let value = both();
    while (tokens[at] === "OR") { at += 1; const right = both(); value = value || right; }
    return value;
  }
  try { const value = either(); return at === tokens.length && value; }
  catch { return false; }
}

export function allowedLicenses(repo) {
  const policy = readFileSync(join(repo, "deny.toml"), "utf8");
  const list = /^\[licenses\][\s\S]*?\ballow\s*=\s*\[([\s\S]*?)\]/m.exec(policy);
  if (!list) throw new Error("read licenses.allow from deny.toml");
  return new Set([...list[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]));
}

/** Join Cargo's package-selected normal graph with locked registry metadata. */
export function selectCrates(metadata, tree, repo) {
  const members = new Set(metadata.workspace_members);
  const found = new Map();
  for (const line of tree.trim().split("\n")) {
    const match = /^([A-Za-z0-9_-]+) v([^\s]+)(?:\s.*)?$/.exec(line.trim());
    if (!match) throw new Error("unrecognised cargo tree package record");
    const [, name, version] = match;
    const candidates = metadata.packages.filter((pkg) => pkg.name === name && pkg.version === version);
    if (candidates.length !== 1) throw new Error(`ambiguous Cargo metadata for ${name} ${version}`);
    const pkg = candidates[0];
    if (members.has(pkg.id)) {
      const path = relative(repo, dirname(pkg.manifest_path));
      if (pkg.license !== "MIT OR Apache-2.0" || path === ".." || path.startsWith("../") || isAbsolute(path)) throw new Error(`${name} ${version}: workspace licence or location is not covered by the root files`);
      continue;
    }
    if (!pkg.source?.startsWith("registry+")) throw new Error(`${name} ${version}: third-party source is not a registry`);
    if (pkg.source !== "registry+https://github.com/rust-lang/crates.io-index") throw new Error(`${name} ${version}: third-party source is not crates.io`);
    found.set(pkg.id, { name, version, license: pkg.license, dir: dirname(pkg.manifest_path) });
  }
  return [...found.values()].sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : a.version > b.version ? 1 : 0);
}

export function projectGraph(repo, tc, rustflags) {
  const common = ["--locked", "--offline", "--manifest-path", join(repo, "Cargo.toml")];
  const options = { encoding: "utf8", maxBuffer: 32 * 1024 * 1024, env: { ...tc.env, CARGO_ENCODED_RUSTFLAGS: rustflags.join("\x1f") } };
  const metadata = JSON.parse(execFileSync("cargo", ["metadata", ...common, "--format-version=1", "--filter-platform", "wasm32-unknown-unknown"], options));
  const tree = execFileSync("cargo", ["tree", ...common, "-p", "coreav-wasm", "--target", "wasm32-unknown-unknown", "-e", "normal,no-proc-macro", "--prefix", "none", "--format", "{p}", "--no-dedupe"], options);
  const members = metadata.packages.filter((pkg) => metadata.workspace_members.includes(pkg.id));
  return { crates: selectCrates(metadata, tree, repo), members: new Set(members.flatMap((pkg) => pkg.targets.map((target) => target.name.replaceAll("-", "_")))), memberPaths: new Set(members.map((pkg) => relative(repo, dirname(pkg.manifest_path)))) };
}

/** Copy upstream texts; missing, empty or unreadable data is never synthesised. */
export function renderNotices(crates, allowed, list = (dir) => readdirSync(dir, { withFileTypes: true }), read = (dir, name) => readFileSync(join(dir, name))) {
  const sections = [];
  const ordered = [...crates].sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : a.version > b.version ? 1 : 0);
  for (const crate of ordered) {
    const label = `${crate.name} ${crate.version}`;
    if (!permitted(crate.license, allowed)) throw new Error(`${label}: unapproved or missing licence expression`);
    let files;
    try { files = list(crate.dir).filter((entry) => /^(?:LICEN[CS]E|COPYING|NOTICE|UNLICENSE|COPYRIGHT)(?:[-._].*)?$/i.test(entry.name)).sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0); }
    catch { throw new Error(`${label}: cannot read upstream licence directory`); }
    if (!files.some((entry) => /^(?:LICEN[CS]E|COPYING|UNLICENSE)/i.test(entry.name) && entry.isFile())) throw new Error(`${label}: no upstream licence file`);
    const rules = new Map([["MIT", /^LICENSE-MIT(?:[-._].*)?$/i], ["Apache-2.0", /^LICENSE-APACHE(?:[-._].*)?$/i], ["Zlib", /^LICENSE-ZLIB(?:[-._].*)?$/i], ["0BSD", /^LICENSE-0BSD(?:[-._].*)?$/i], ["Unlicense", /^UNLICENSE(?:[-._].*)?$/i], ["Unicode-3.0", /^LICENSE-UNICODE(?:[-._].*)?$/i], ["Apache-2.0 WITH LLVM-exception", /^LICENSE-Apache-2\.0_WITH_LLVM-exception(?:[-._].*)?$/i]]);
    const terms = crate.license.replaceAll("/", " OR ").match(/[A-Za-z0-9.+-]+(?:\s+WITH\s+[A-Za-z0-9.+-]+)?/g).filter((term) => !["AND", "OR"].includes(term)).map((term) => term.replace(/\s+/g, " "));
    const covered = new Set();
    for (const term of terms) {
      const rule = rules.get(term);
      if (!rule) throw new Error(`${label}: add a file rule for ${term}`);
      if (files.some((entry) => rule.test(entry.name) || terms.length === 1 && /^(?:LICEN[CS]E|COPYING)(?:\.txt|\.md)?$/i.test(entry.name))) covered.add(term);
    }
    if (!permitted(crate.license, covered)) throw new Error(`${label}: missing upstream licence file for ${terms.filter((term) => !covered.has(term)).join(", ")}`);
    sections.push(`=== ${label} (${crate.origin ?? "crates.io"}) ===\nLicense: ${crate.license}\n`);
    for (const entry of files) {
      if (!entry.isFile() || entry.isSymbolicLink()) throw new Error(`${label}: licence or notice is not a regular file`);
      let bytes;
      try { bytes = read(crate.dir, entry.name); }
      catch { throw new Error(`${label}: cannot read upstream file ${entry.name}`); }
      if (crate.checksums && crate.checksums[entry.name] !== createHash("sha256").update(bytes).digest("hex")) throw new Error(`${label}: ${entry.name} differs from its vendored checksum`);
      if (!bytes.byteLength || bytes.byteLength > 256 * 1024) throw new Error(`${label}: empty or oversized licence file ${entry.name}`);
      let text;
      try { text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes).replace(/\r\n/g, "\n").replace(/\n*$/, "\n"); }
      catch { throw new Error(`${label}: non-UTF-8 licence file ${entry.name}`); }
      sections.push(`--- ${entry.name} ---\n${text}`);
    }
  }
  return sections.join("\n");
}

export function toolchain(repo, env = process.env) {
  const pin = readFileSync(join(repo, "scripts/rust-wasm.env"), "utf8");
  const version = /^RUST_WASM_TOOLCHAIN=([0-9.]+)$/m.exec(pin)?.[1];
  const commit = /^RUST_WASM_COMMIT=([a-f0-9]{40})$/m.exec(pin)?.[1];
  if (!version || !commit) throw new Error("read the WASM Rust version and commit from scripts/rust-wasm.env");
  const pinned = { ...env, RUSTUP_TOOLCHAIN: version };
  let details, sysroot;
  try {
    details = execFileSync("rustc", ["-vV"], { env: pinned, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    sysroot = execFileSync("rustc", ["--print", "sysroot"], { env: pinned, encoding: "utf8" }).trim();
  } catch { throw new Error(`install Rust ${version}: rustup toolchain install ${version} --profile minimal --target wasm32-unknown-unknown --component rust-src`); }
  if (!details.includes(`commit-hash: ${commit}\n`) || !details.includes(`release: ${version}\n`)) throw new Error(`Rust ${version}: compiler commit differs from scripts/rust-wasm.env`);
  const library = join(sysroot, "lib/rustlib/src/rust/library");
  if (!existsSync(join(library, "Cargo.lock"))) throw new Error(`rust-src for ${version} is missing: rustup component add rust-src --toolchain ${version}`);
  return { version, commit, sysroot, library, env: pinned };
}

function field(text, name, optional = false) {
  const value = new RegExp(`^${name}\\s*=\\s*("[^\\n]*"|'[^'\\n]*')\\s*$`, "m").exec(text);
  if (value) return value[1].startsWith("'") ? value[1].slice(1, -1) : JSON.parse(value[1]);
  if (optional) return null;
  throw new Error(`missing ${name} in Rust library metadata`);
}

export function toolchainCrates(lockText, rlibNames) {
  const packages = lockText.split(/^\[\[package\]\]\s*$/m).slice(1).map((block) => {
    const dependencies = /^dependencies\s*=\s*\[([\s\S]*?)\]/m.exec(block)?.[1] ?? "";
    return { name: field(block, "name"), version: field(block, "version"), source: field(block, "source", true), checksum: field(block, "checksum", true), dependencies: JSON.parse(`[${dependencies.replace(/,\s*$/, "")}]`) };
  });
  const visited = new Set(), selected = [];
  const pending = ["std", "panic_abort"];
  while (pending.length) {
    const name = pending.shift().split(" ")[0];
    if (visited.has(name)) continue;
    visited.add(name);
    const matches = packages.filter((pkg) => pkg.name === name);
    if (matches.length !== 1) throw new Error(`${name}: ambiguous or missing Rust library lock package`);
    const pkg = matches[0];
    pending.push(...pkg.dependencies);
    const rustName = name.replaceAll("-", "_");
    const archives = rlibNames.filter((file) => new RegExp(`^lib${rustName}-[a-f0-9]{16}\\.rlib$`).test(file));
    if (archives.length > 1) throw new Error(`${name}: ambiguous wasm32 Rust library archive`);
    if (archives.length === 1) selected.push(pkg);
  }
  return { inTree: selected.filter((pkg) => !pkg.source), vendored: selected.filter((pkg) => pkg.source) };
}

export function vendoredCrates(library, packages, read = (path) => readFileSync(path)) {
  return packages.map((pkg) => {
    const label = `${pkg.name} ${pkg.version}`, dir = join(library, "vendor", `${pkg.name}-${pkg.version}`);
    let checksum, manifest;
    try {
      checksum = JSON.parse(read(join(dir, ".cargo-checksum.json")));
      manifest = read(join(dir, "Cargo.toml")).toString();
    } catch { throw new Error(`${label}: vendored sources missing from rust-src`); }
    if (pkg.source !== "registry+https://github.com/rust-lang/crates.io-index" || !pkg.checksum || checksum.package !== pkg.checksum || !checksum.files || typeof checksum.files !== "object" || Array.isArray(checksum.files)) throw new Error(`${label}: vendored checksum differs from library/Cargo.lock`);
    let license;
    try {
      if (field(manifest, "name") !== pkg.name || field(manifest, "version") !== pkg.version) throw new Error("manifest identity differs");
      license = field(manifest, "license");
    } catch { throw new Error(`${label}: vendored manifest identity or licence differs`); }
    return { name: pkg.name, version: pkg.version, dir, license, checksums: checksum.files, origin: "Rust toolchain, crates.io" };
  });
}

export function inTreeStatement(html) {
  const start = html.indexOf('<h2 id="in-tree-files">'), end = html.indexOf('<h2 id="out-of-tree-dependencies">');
  if (start < 0 || end <= start) throw new Error("Rust standard library: missing copyright statement boundaries");
  const entities = { "&#34;": '"', "&#39;": "'", "&#60;": "<", "&#62;": ">" };
  const text = html.slice(start, end).replace(/<[^>]*>/g, "").replace(/&[^;\s]+;/g, (entity) => {
    if (!(entity in entities)) throw new Error("Rust standard library: unknown copyright HTML entity");
    return entities[entity];
  });
  return text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).join("\n") + "\n";
}

export const IN_TREE_FILES = Object.freeze({ compiler_builtins: ["compiler-builtins/LICENSE.txt", "compiler-builtins/libm/LICENSE.txt"] });
const IN_TREE_MANIFESTS = Object.freeze({ compiler_builtins: "compiler-builtins/compiler-builtins" });

export function renderToolchain(tc, crates, allowed, read = (path) => readFileSync(path)) {
  const readText = (path, label, limit = 256 * 1024) => {
    try {
      const bytes = read(path);
      if (!bytes.byteLength || bytes.byteLength > limit) throw new Error("invalid licence file");
      return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes).replace(/\r\n/g, "\n").replace(/\n*$/, "\n");
    } catch { throw new Error(`${label}: missing or unreadable licence text`); }
  };
  const normal = crates.filter((crate) => crate.license === "MIT OR Apache-2.0");
  if (!["std", "core", "alloc"].every((name) => normal.some((crate) => crate.name === name))) throw new Error("Rust standard library: std, core or alloc coverage is missing");
  const doc = join(tc.sysroot, "share/doc/rust");
  const statement = inTreeStatement(readText(join(doc, "COPYRIGHT-library.html"), `Rust ${tc.version} copyright`, 4 * 1024 * 1024));
  const ids = new Set();
  for (const expression of [...statement.matchAll(/^License:\s*(.+)$/gm)].map((match) => match[1])) {
    if (!permitted(expression, allowed)) throw new Error(`Rust ${tc.version}: unapproved in-tree licence expression`);
    for (const id of expression.match(/[A-Za-z0-9.+-]+/g).filter((id) => !["AND", "OR"].includes(id))) ids.add(id);
  }
  if (!ids.size) throw new Error(`Rust ${tc.version}: copyright statement has no licence expressions`);
  const sections = [`=== Rust standard library ${tc.version} (${tc.commit}) (Rust toolchain) ===\nLicense: MIT OR Apache-2.0\nCrates: ${normal.map((crate) => crate.name).sort().join(", ")}\n\n${statement}`];
  for (const id of [...ids].sort()) sections.push(`--- licenses/${id}.txt ---\n${readText(join(doc, "licenses", `${id}.txt`), `licenses/${id}.txt from Rust ${tc.version}`)}`);
  for (const crate of crates.filter((crate) => crate.license !== "MIT OR Apache-2.0").sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0)) {
    const files = IN_TREE_FILES[crate.name];
    if (!files || !permitted(crate.license, allowed)) throw new Error(`in-tree crate ${crate.name} (${crate.license}) needs an IN_TREE_FILES row`);
    sections.push(`=== ${crate.name} ${crate.version} (Rust toolchain) ===\nLicense: ${crate.license}\n`);
    for (const file of files) sections.push(`--- ${file} ---\n${readText(join(tc.library, file), `${crate.name} ${crate.version} ${file}`)}`);
  }
  return sections.join("\n");
}

export function packageNotices(directory, repo = resolve(directory, "../..")) {
  const tc = toolchain(repo), allowed = allowedLicenses(repo);
  const provenance = JSON.parse(readFileSync(join(directory, "wasm/provenance.json"), "utf8"));
  const rustflags = provenance.rustflags;
  if (!Array.isArray(rustflags) || !rustflags.every((flag) => typeof flag === "string" && !flag.startsWith("--remap-path-prefix")) || !rustflags.some((flag, index) => flag === "-C" && rustflags[index + 1] === "target-feature=+simd128")) throw new Error("run npm run build: missing SIMD build flags in provenance");
  const raw = readFileSync(join(repo, "target/wasm32-unknown-unknown/edge/coreav_wasm.wasm"));
  if (createHash("sha256").update(raw).digest("hex") !== provenance.rawWasmSha256) throw new Error("run npm run build: raw module provenance is stale");
  const packed = readFileSync(join(directory, "wasm/coreav_bg.wasm"));
  if (createHash("sha256").update(packed).digest("hex") !== provenance.wasmSha256) throw new Error("run npm run build: packed module provenance is stale");
  const graph = projectGraph(repo, tc, rustflags);
  const selected = toolchainCrates(readFileSync(join(tc.library, "Cargo.lock"), "utf8"), readdirSync(join(tc.sysroot, "lib/rustlib/wasm32-unknown-unknown/lib")));
  const inTree = selected.inTree.map((crate) => {
    try {
      const text = readFileSync(join(tc.library, IN_TREE_MANIFESTS[crate.name] ?? crate.name, "Cargo.toml"), "utf8");
      const pkg = /(?:^|\n)\[package\]\s*\n([\s\S]*?)(?=\n\[|$)/.exec(text)?.[1];
      if (!pkg || field(pkg, "name") !== crate.name || field(pkg, "version") !== crate.version) throw new Error("in-tree manifest identity differs");
      return { ...crate, license: field(pkg, "license") };
    } catch { throw new Error(`${crate.name} ${crate.version}: in-tree identity or licence metadata missing`); }
  });
  const text = "Rust dependency notices for this WebAssembly build.\nThe standard-library target closure is a superset of linker-retained code.\nCoreAV's own licence and notices are in LICENSE-MIT, LICENSE-APACHE and NOTICE.\n\n" + renderNotices(graph.crates, allowed) + "\n" + renderToolchain(tc, inTree, allowed) + "\n" + renderNotices(vendoredCrates(tc.library, selected.vendored), allowed);
  const context = { version: tc.version, commit: tc.commit, members: graph.members, memberPaths: graph.memberPaths, library: new Set(readdirSync(tc.library, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name)) };
  const evidence = moduleEvidence(packed, raw);
  const sections = noticeSections(text);
  validateEvidence(evidence, sections, context);
  return { text, evidence, sections, context, tc };
}

export function packageLegalFiles(directory, repo) {
  return new Map([...['LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE'].map((name) => [name, readFileSync(join(repo, name))]), ["THIRD-PARTY-NOTICES.txt", Buffer.from(packageNotices(directory, repo).text)]]);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const directory = resolve(process.argv[2] ?? fileURLToPath(new URL("..", import.meta.url)));
  const repo = resolve(directory, "../..");
  const notices = packageNotices(directory, repo).text;
  const path = join(directory, "THIRD-PARTY-NOTICES.txt");
  if (process.argv[3] === "--check") {
    if (readFileSync(path, "utf8") !== notices) throw new Error("stale third-party notices; run npm run build");
  } else if (process.argv[3]) throw new Error("usage: legal.mjs [PACKAGE] [--check]");
  else writeFileSync(path, notices);
}
