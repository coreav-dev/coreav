import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Evaluate SPDX alternatives against the repository's explicit licence choices. */
export function permitted(text, allowed) {
  if (typeof text !== "string") return false;
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

export function linkedCrates(repo) {
  const common = ["--locked", "--offline", "--manifest-path", join(repo, "Cargo.toml")];
  const metadata = JSON.parse(execFileSync("cargo", ["metadata", ...common, "--format-version=1", "--filter-platform", "wasm32-unknown-unknown"], { encoding: "utf8", maxBuffer: 32 * 1024 * 1024 }));
  const tree = execFileSync("cargo", ["tree", ...common, "-p", "coreav-wasm", "--target", "wasm32-unknown-unknown", "-e", "normal,no-proc-macro", "--prefix", "none", "--format", "{p}", "--no-dedupe"], { encoding: "utf8" });
  return selectCrates(metadata, tree, repo);
}

/** Copy upstream texts; missing, empty or unreadable data is never synthesised. */
export function renderNotices(crates, allowed, list = (dir) => readdirSync(dir, { withFileTypes: true }), read = (dir, name) => readFileSync(join(dir, name))) {
  const sections = ["Third-party Rust crates linked into the WebAssembly module.\nCoreAV's own licence and notices are in LICENSE-MIT, LICENSE-APACHE and NOTICE.\n"];
  const ordered = [...crates].sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : a.version > b.version ? 1 : 0);
  for (const crate of ordered) {
    const label = `${crate.name} ${crate.version}`;
    if (!permitted(crate.license, allowed)) throw new Error(`${label}: unapproved or missing licence expression`);
    let files;
    try { files = list(crate.dir).filter((entry) => /^(?:LICEN[CS]E|COPYING|NOTICE|UNLICENSE|COPYRIGHT)(?:[-._].*)?$/i.test(entry.name)).sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0); }
    catch { throw new Error(`${label}: cannot read upstream licence directory`); }
    if (!files.some((entry) => /^(?:LICEN[CS]E|COPYING|UNLICENSE)/i.test(entry.name) && entry.isFile())) throw new Error(`${label}: no upstream licence file`);
    const rules = new Map([["MIT", /^LICENSE-MIT(?:[-._].*)?$/i], ["Apache-2.0", /^LICENSE-APACHE(?:[-._].*)?$/i], ["Zlib", /^LICENSE-ZLIB(?:[-._].*)?$/i], ["0BSD", /^LICENSE-0BSD(?:[-._].*)?$/i], ["Unlicense", /^UNLICENSE(?:[-._].*)?$/i], ["Unicode-3.0", /^LICENSE-UNICODE(?:[-._].*)?$/i]]);
    const terms = crate.license.match(/[A-Za-z0-9.+-]+/g).filter((term) => !["AND", "OR"].includes(term));
    const covered = new Set();
    for (const term of terms) {
      const rule = rules.get(term);
      if (!rule) throw new Error(`${label}: add a file rule for ${term}`);
      if (files.some((entry) => rule.test(entry.name) || terms.length === 1 && /^(?:LICEN[CS]E|COPYING)(?:\.txt|\.md)?$/i.test(entry.name))) covered.add(term);
    }
    if (!permitted(crate.license, covered)) throw new Error(`${label}: missing upstream licence file for ${terms.filter((term) => !covered.has(term)).join(", ")}`);
    sections.push(`=== ${label} (crates.io) ===\nLicense: ${crate.license}\n`);
    for (const entry of files) {
      if (!entry.isFile() || entry.isSymbolicLink()) throw new Error(`${label}: licence or notice is not a regular file`);
      let bytes;
      try { bytes = read(crate.dir, entry.name); }
      catch { throw new Error(`${label}: cannot read upstream file ${entry.name}`); }
      if (!bytes.byteLength || bytes.byteLength > 256 * 1024) throw new Error(`${label}: empty or oversized licence file ${entry.name}`);
      let text;
      try { text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes).replace(/\r\n/g, "\n").replace(/\n*$/, "\n"); }
      catch { throw new Error(`${label}: non-UTF-8 licence file ${entry.name}`); }
      sections.push(`--- ${entry.name} ---\n${text}`);
    }
  }
  return sections.join("\n");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const directory = resolve(process.argv[2] ?? fileURLToPath(new URL("..", import.meta.url)));
  const repo = resolve(directory, "../..");
  const notices = renderNotices(linkedCrates(repo), allowedLicenses(repo));
  const path = join(directory, "THIRD-PARTY-NOTICES.txt");
  if (process.argv[3] === "--check") {
    if (readFileSync(path, "utf8") !== notices) throw new Error("stale third-party notices; run npm run build");
  } else if (process.argv[3]) throw new Error("usage: legal.mjs [PACKAGE] [--check]");
  else writeFileSync(path, notices);
}
