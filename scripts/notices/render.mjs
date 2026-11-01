import { readdirSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";

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

export function licensePolicy(repo) {
  const text = readFileSync(join(repo, "deny.toml"), "utf8").replace(/#[^\n]*/g, "");
  const list = /^\[licenses\][\s\S]*?\ballow\s*=\s*\[([\s\S]*?)\]/m.exec(text);
  if (!list) throw new Error("read licenses.allow from deny.toml");
  const allowed = new Set([...list[1].matchAll(/"([^"]+)"/g)].map(match => match[1]));
  const exceptions = new Map();
  for (const block of text.split(/^\[\[licenses\.exceptions\]\]\s*$/m).slice(1)) {
    const row = block.split(/^\[/m)[0].replace(/#[^\n]*/g, "");
    const crate = /^crate\s*=\s*"([A-Za-z0-9_-]+)"\s*$/m.exec(row)?.[1];
    const values = /^allow\s*=\s*\[([\s\S]*?)\]/m.exec(row);
    const rest = row.replace(/^crate\s*=\s*"[^"]+"\s*$/m, "").replace(/^allow\s*=\s*\[[\s\S]*?\]/m, "").trim();
    if (!crate || !values || rest || exceptions.has(crate)) throw new Error("read each licenses.exceptions crate and allow list from deny.toml; unsupported policy needs a generator update");
    exceptions.set(crate, new Set([...values[1].matchAll(/"([^"]+)"/g)].map(match => match[1])));
  }
  return { allowed, exceptions };
}

export function licenseTerms(expression) {
  return [...new Set(expression.replaceAll("/", " OR ").match(/[A-Za-z0-9.+-]+(?:\s+WITH\s+[A-Za-z0-9.+-]+)?/g).filter(term => !["AND", "OR"].includes(term)).map(term => term.replace(/\s+/g, " ")))];
}

const FILE_RULES = new Map([
  ["MIT", /^LICENSE-MIT(?:[-._].*)?$/i], ["Apache-2.0", /^LICENSE-APACHE(?:[-._].*)?$/i],
  ["Zlib", /^LICENSE-ZLIB(?:[-._].*)?$/i], ["0BSD", /^LICENSE-0BSD(?:[-._].*)?$/i],
  ["Unlicense", /^UNLICENSE(?:[-._].*)?$/i], ["Unicode-3.0", /^LICENSE-UNICODE(?:[-._].*)?$/i],
  ["ISC", /^LICENSE-ISC(?:[-._].*)?$/i],
  ["Apache-2.0 WITH LLVM-exception", /^LICENSE-Apache-2\.0_WITH_LLVM-exception(?:[-._].*)?$/i],
]);
const CRATE_TERM_FILES = Object.freeze({
  "ring 0.17.14": { "Apache-2.0": ["LICENSE-BoringSSL"], ISC: ["LICENSE-other-bits"] },
});
const REPO_NOTICE_TERMS = Object.freeze({
  "jpeg-encoder": { IJG: "This software is based in part on the work of the Independent JPEG Group" },
});
/** Copy upstream texts; missing, empty or unreadable data is never synthesised. */
export function renderNotices(crates, policy, repoNotice = "", list = dir => readdirSync(dir, { withFileTypes: true }), read = (dir, name) => readFileSync(join(dir, name))) {
  const sections = [];
  const ordered = [...crates].sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : a.version > b.version ? 1 : 0);
  for (const crate of ordered) {
    const label = crate.name + " " + crate.version;
    const exceptions = policy.exceptions.get(crate.name) ?? new Set();
    if (!permitted(crate.license, new Set([...policy.allowed, ...exceptions]))) throw new Error(label + ": unapproved or missing licence expression");
    const visit = (prefix = "") => {
      const found = [];
      for (const entry of list(join(crate.dir, prefix))) {
        const name = prefix ? prefix + "/" + entry.name : entry.name;
        if (/^(?:LICEN[CS]E|COPYING|NOTICE|UNLICENSE|COPYRIGHT)(?:[-._].*)?$/i.test(entry.name)) {
          if (!entry.isFile() || entry.isSymbolicLink()) throw new Error("licence or notice is not a regular file");
          found.push({ name });
        } else if (entry.isDirectory?.() && !entry.isSymbolicLink()) found.push(...visit(name));
      }
      return found;
    };
    let files;
    try { files = visit().sort((a, b) => Number(a.name.includes("/")) - Number(b.name.includes("/")) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0)); }
    catch (error) {
      if (error.message === "licence or notice is not a regular file") throw new Error(label + ": " + error.message);
      throw new Error(label + ": cannot read upstream licence directory");
    }
    const top = files.filter(entry => !entry.name.includes("/"));
    if (!top.some(entry => /^(?:LICEN[CS]E|COPYING|UNLICENSE)/i.test(entry.name))) throw new Error(label + ": no upstream licence file");
    const terms = licenseTerms(crate.license);
    const covered = new Set(), pointers = [];
    const specific = CRATE_TERM_FILES[label];
    if (!specific && Object.keys(CRATE_TERM_FILES).some(key => key.startsWith(crate.name + " "))) throw new Error(label + ": add a CRATE_TERM_FILES row");
    for (const term of terms) {
      const rule = FILE_RULES.get(term), named = specific?.[term];
      if (named ? named.every(name => top.some(entry => entry.name === name)) : top.some(entry => rule?.test(entry.name) || terms.length === 1 && /^(?:LICEN[CS]E|COPYING)(?:\.txt|\.md)?$/i.test(entry.name))) covered.add(term);
      const sentence = REPO_NOTICE_TERMS[crate.name]?.[term];
      if (sentence && exceptions.has(term) && repoNotice.includes(sentence)) {
        covered.add(term);
        pointers.push(term + ": see NOTICE");
      }
    }
    if (!permitted(crate.license, covered)) throw new Error(label + ": missing upstream licence file; add a file rule for " + terms.filter(term => !covered.has(term)).join(", "));
    sections.push("=== " + label + " (" + (crate.origin ?? "crates.io") + ") ===\nLicense: " + crate.license + "\n" + (pointers.length ? pointers.join("\n") + "\n" : ""));
    for (const entry of files) {
      let bytes;
      try { bytes = read(crate.dir, entry.name); }
      catch { throw new Error(label + ": cannot read upstream file " + entry.name); }
      if (crate.checksums && crate.checksums[entry.name] !== createHash("sha256").update(bytes).digest("hex")) throw new Error(label + ": " + entry.name + " differs from its vendored checksum");
      if (!bytes.byteLength || bytes.byteLength > 256 * 1024) throw new Error(label + ": empty or oversized licence file " + entry.name);
      let text;
      try { text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes).replace(/\r\n/g, "\n").replace(/\n*$/, "\n"); }
      catch { throw new Error(label + ": non-UTF-8 licence file " + entry.name); }
      sections.push("--- " + entry.name + " ---\n" + text);
    }
  }
  return sections.join("\n");
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

export function renderToolchain(tc, crates, policy, read = (path) => readFileSync(path)) {
  const readText = (path, label, limit = 256 * 1024) => {
    try {
      const bytes = read(path);
      if (!bytes.byteLength || bytes.byteLength > limit) throw new Error("invalid licence file");
      return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes).replace(/\r\n/g, "\n").replace(/\n*$/, "\n");
    } catch { throw new Error(`${label}: missing or unreadable licence text`); }
  };
  const allowed = policy.allowed;
  const normal = crates.filter((crate) => crate.license === "MIT OR Apache-2.0" || crate.license === null);
  if (!["std", "core", "alloc"].every((name) => normal.some((crate) => crate.name === name))) throw new Error("Rust standard library: std, core or alloc coverage is missing");
  const doc = join(tc.sysroot, "share/doc/rust");
  const statement = inTreeStatement(readText(join(doc, "COPYRIGHT-library.html"), `Rust ${tc.version} copyright`, 4 * 1024 * 1024));
  for (const crate of normal.filter(crate => crate.license === null)) {
    const directory = "library/" + crate.dir;
    if (!/^File\/Directory: \.\nLicense: Apache-2\.0 OR MIT$/m.test(statement) || [...statement.matchAll(/^File\/Directory: (.+)$/gm)].some(match => match[1] !== "." && (directory === match[1] || directory.startsWith(match[1] + "/")))) throw new Error(crate.name + ": in-tree copyright statement does not cover its missing licence metadata");
  }
  const ids = new Set();
  for (const expression of [...statement.matchAll(/^License:\s*(.+)$/gm)].map((match) => match[1])) {
    if (!permitted(expression, allowed)) throw new Error(`Rust ${tc.version}: unapproved in-tree licence expression`);
    for (const id of expression.match(/[A-Za-z0-9.+-]+/g).filter((id) => !["AND", "OR"].includes(id))) ids.add(id);
  }
  if (!ids.size) throw new Error(`Rust ${tc.version}: copyright statement has no licence expressions`);
  const sections = [`=== Rust standard library ${tc.version} (${tc.commit}) (Rust toolchain) ===\nLicense: MIT OR Apache-2.0\nCrates: ${normal.map((crate) => crate.name).sort().join(", ")}\n\n${statement}`];
  for (const id of [...ids].sort()) sections.push(`--- licenses/${id}.txt ---\n${readText(join(doc, "licenses", `${id}.txt`), `licenses/${id}.txt from Rust ${tc.version}`)}`);
  for (const crate of crates.filter((crate) => !normal.includes(crate)).sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0)) {
    const files = IN_TREE_FILES[crate.name];
    if (!files || !permitted(crate.license, allowed)) throw new Error(`in-tree crate ${crate.name} (${crate.license}) needs an IN_TREE_FILES row`);
    sections.push(`=== ${crate.name} ${crate.version} (Rust toolchain) ===\nLicense: ${crate.license}\n`);
    for (const file of files) sections.push(`--- ${file} ---\n${readText(join(tc.library, file), `${crate.name} ${crate.version} ${file}`)}`);
  }
  return sections.join("\n");
}
