const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const version = String.raw`[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?`;
const crateLabel = new RegExp(`^([A-Za-z0-9_][A-Za-z0-9_-]*) (${version})$`);
const standardLabel = new RegExp(`^Rust standard library (${version}) \\(([a-f0-9]{40})\\)$`);
const normalise = name => name.replace(/-/g, "_");

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

/** Parse notice identities independently of the code that selected or rendered them. */
export function noticeSections(text) {
  const sections = new Map();
  const lines = text.split(/\r?\n/);
  for (let at = 0; at < lines.length; at += 1) {
    if (!lines[at].startsWith("===") || /^=+$/.test(lines[at])) continue;
    const header = /^=== (.+) \((crates\.io|Rust toolchain|Rust toolchain, crates\.io)\) ===$/.exec(lines[at]);
    requireValue(header, "malformed third-party notice header or unknown origin");
    const [, label, origin] = header;
    requireValue(!/[\x00-\x1f\x7f]/.test(label), "malformed third-party notice label");
    const license = /^License: ([^\x00-\x1f\x7f]+)$/.exec(lines[at + 1] ?? "");
    requireValue(license && license[1].trim(), "third-party notice section is missing its License line");
    const key = `${origin}\0${label}`;
    requireValue(!sections.has(key), "duplicate third-party notice section within an origin");
    const section = { origin, label, license: license[1] };
    const standard = standardLabel.exec(label);
    requireValue(standard || crateLabel.test(label), "malformed third-party notice crate label");
    if (standard) {
      requireValue(origin === "Rust toolchain", "Rust standard library section has the wrong origin");
      const crates = /^Crates: ([A-Za-z0-9_-]+(?:, [A-Za-z0-9_-]+)*)$/.exec(lines[at + 2] ?? "");
      requireValue(crates, "Rust standard library section is missing its Crates line");
      section.crates = crates[1].split(", ");
      requireValue(new Set(section.crates.map(normalise)).size === section.crates.length, "duplicate Rust standard library crate");
    }
    sections.set(key, section);
  }
  return sections;
}

function u32(bytes, cursor) {
  let value = 0;
  for (let digit = 0; digit < 5; digit += 1) {
    requireValue(cursor.at < cursor.end, "malformed WebAssembly: truncated ULEB32");
    const byte = bytes[cursor.at++];
    requireValue(digit < 4 || byte < 16, "malformed WebAssembly: ULEB32 overflow");
    value += (byte & 127) * 2 ** (digit * 7);
    if (!(byte & 128)) return value;
  }
  throw new Error("malformed WebAssembly: unbounded ULEB32");
}

function wasmString(bytes, cursor) {
  const length = u32(bytes, cursor);
  requireValue(length <= cursor.end - cursor.at, "malformed WebAssembly: string exceeds section bounds");
  let text;
  try { text = utf8.decode(bytes.subarray(cursor.at, cursor.at + length)); }
  catch { throw new Error("malformed WebAssembly: invalid UTF-8 name"); }
  cursor.at += length;
  return text;
}

function functionNames(bytes, required) {
  requireValue(bytes instanceof Uint8Array && bytes.length >= 8 && [0, 97, 115, 109, 1, 0, 0, 0].every((byte, at) => bytes[at] === byte), "malformed WebAssembly: invalid module header");
  const cursor = { at: 8, end: bytes.length };
  const names = [];
  const indices = new Set();
  let named = false;
  let functions = false;
  while (cursor.at < cursor.end) {
    const id = bytes[cursor.at++];
    requireValue(id <= 13, "malformed WebAssembly: unknown section ID");
    const size = u32(bytes, cursor);
    requireValue(size <= cursor.end - cursor.at, "malformed WebAssembly: section exceeds module bounds");
    const section = { at: cursor.at, end: cursor.at + size };
    if (id === 0 && wasmString(bytes, section) === "name") {
      requireValue(!named, "malformed WebAssembly: duplicate name section");
      named = true;
      while (section.at < section.end) {
        const kind = bytes[section.at++];
        const length = u32(bytes, section);
        requireValue(length <= section.end - section.at, "malformed WebAssembly: name subsection exceeds bounds");
        const subsection = { at: section.at, end: section.at + length };
        if (kind === 1) {
          requireValue(!functions, "malformed WebAssembly: duplicate function-name subsection");
          functions = true;
          const count = u32(bytes, subsection);
          requireValue(count <= (subsection.end - subsection.at) / 2, "malformed WebAssembly: function-name count exceeds bounds");
          for (let index = 0; index < count; index += 1) {
            const functionIndex = u32(bytes, subsection);
            requireValue(!indices.has(functionIndex), "malformed WebAssembly: duplicate function-name index");
            indices.add(functionIndex);
            const name = wasmString(bytes, subsection);
            if (name) names.push(name);
          }
          requireValue(subsection.at === subsection.end, "malformed WebAssembly: trailing function-name data");
        }
        section.at = subsection.end;
      }
    }
    cursor.at = section.end;
  }
  requireValue(!required || names.length > 0, 'raw module has no function names; keep profile.edge strip = "debuginfo"');
  return names;
}

function identifier(symbol, at, separator = true) {
  const digits = /^[0-9]+/.exec(symbol.slice(at));
  requireValue(digits, "malformed Rust symbol: missing crate identifier length");
  const length = Number(digits[0]);
  at += digits[0].length;
  if (separator && symbol[at] === "_") at += 1;
  requireValue(Number.isSafeInteger(length) && length > 0 && length <= symbol.length - at, "malformed Rust symbol: crate identifier exceeds bounds");
  const root = symbol.slice(at, at + length);
  requireValue(/^[A-Za-z_][A-Za-z0-9_]*$/.test(root), "malformed Rust symbol: invalid crate identifier");
  return root;
}

function base62(symbol, at) {
  const end = symbol.indexOf("_", at);
  requireValue(end >= at && /^[A-Za-z0-9]*$/.test(symbol.slice(at, end)), "malformed Rust symbol: invalid disambiguator or backreference");
  return end + 1;
}

function crateRoot(symbol) {
  if (symbol.startsWith("_ZN")) return identifier(symbol, 3, false);
  if (!symbol.startsWith("_R")) return null;
  let at = 2;
  while (at < symbol.length) {
    const tag = symbol[at++];
    if (tag === "N") {
      requireValue(/[A-Za-z]/.test(symbol[at] ?? ""), "malformed Rust symbol: invalid namespace");
      at += 1;
    } else if (tag === "C") {
      if (symbol[at] === "s") at = base62(symbol, at + 1);
      return identifier(symbol, at);
    } else if (tag === "M" || tag === "X") {
      if (symbol[at] === "s") at = base62(symbol, at + 1);
    } else if (tag === "B" || tag === "L") {
      at = base62(symbol, at);
    }
  }
  throw new Error("malformed Rust symbol: missing owner crate root");
}

function addVersion(map, name, version) {
  if (!map.has(name)) map.set(name, new Set());
  map.get(name).add(version);
}

function sourcePaths(bytes, evidence) {
  const rust = /^\/rustc\/([a-f0-9]{40})\/library\/([A-Za-z0-9_-]+)(?:\/[^\s]+)?$/;
  const vendor = new RegExp(`^/rustc/([a-f0-9]{40})/library/vendor/([A-Za-z0-9_-]+)-(${version})(?:/[^\\s]+)?$`);
  const deps = new RegExp(`^/rust/deps/([A-Za-z0-9_-]+)-(${version})(?:/[^\\s]+)?$`);
  const registry = new RegExp(`^/cargo/registry/[^/]+/([A-Za-z0-9_-]+)-(${version})(?:/[^\\s]+)?$`);
  const member = /^((?:crates|tools)\/[A-Za-z0-9_-]+)(?:\/[^\s]+)?$/;
  const identityPath = (path) => path.includes(".rs") || /^(?:\/rustc\/|\/rust\/deps\/|\/cargo\/registry\/|crates\/|tools\/)/.test(path);
  let start = 0;
  for (let at = 0; at <= bytes.length; at += 1) {
    if (at < bytes.length && bytes[at] >= 32 && bytes[at] <= 126) continue;
    if (at - start >= 4) {
      const run = utf8.decode(bytes.subarray(start, at));
      if (run.includes("/")) {
        const paths = run.split(/[\s"'()[\]{},;]+/).filter(path => path.includes("/") && identityPath(path));
        for (const path of paths) {
          requireValue(!path.split("/").some(part => part === "." || part === ".."), "module carries an unclassified source path");
          const toolchain = rust.exec(path);
          const toolchainVendor = vendor.exec(path);
          const dependency = deps.exec(path);
          const upstream = registry.exec(path);
          const own = member.exec(path);
          if (toolchainVendor) {
            evidence.commits.add(toolchainVendor[1]);
            addVersion(evidence.deps, toolchainVendor[2], toolchainVendor[3]);
          } else if (toolchain) {
            requireValue(toolchain[2] !== "vendor", "module carries an unclassified source path");
            evidence.commits.add(toolchain[1]);
            evidence.library.add(toolchain[2]);
          } else if (dependency) addVersion(evidence.deps, dependency[1], dependency[2]);
          else if (upstream) addVersion(evidence.registry, upstream[1], upstream[2]);
          else if (own) evidence.members.add(own[1]);
          else throw new Error("module carries an unclassified source path");
        }
      }
    }
    start = at + 1;
  }
}

/** Observe source paths in both artifacts and owner crate roots in the unstripped one. */
export function moduleEvidence(packed, raw) {
  functionNames(packed, false);
  const names = functionNames(raw, true);
  const evidence = { commits: new Set(), library: new Set(), deps: new Map(), registry: new Map(), members: new Set(), roots: new Set() };
  sourcePaths(packed, evidence);
  sourcePaths(raw, evidence);
  for (const name of names) {
    const root = crateRoot(name);
    if (root) evidence.roots.add(root);
  }
  return evidence;
}

/** Require artifact evidence to be covered, without treating absent evidence as absent code. */
export function validateEvidence(evidence, sections, context) {
  const missing = new Set();
  const standard = sections.get(`Rust toolchain\0Rust standard library ${context.version} (${context.commit})`);
  const covered = new Set(standard?.crates?.map(normalise) ?? []);
  let builtins = false;
  for (const section of sections.values()) {
    const crate = crateLabel.exec(section.label);
    if (!crate) continue;
    covered.add(normalise(crate[1]));
    if (section.origin === "Rust toolchain" && normalise(crate[1]) === "compiler_builtins") builtins = true;
  }
  for (const commit of evidence.commits) {
    if (commit !== context.commit) missing.add("module carries a Rust toolchain commit that differs from the pinned commit");
  }
  for (const directory of evidence.library) {
    if (!context.library.has(directory)) missing.add("module carries an unknown Rust library source path");
    else if (directory === "compiler-builtins" ? !builtins : !standard) missing.add(`module carries Rust library ${directory} that THIRD-PARTY-NOTICES.txt does not cover`);
  }
  for (const path of evidence.members) {
    if (!context.memberPaths.has(path)) missing.add("module carries an unknown workspace source path");
  }
  for (const [map, origin, kind] of [[evidence.deps, "Rust toolchain, crates.io", "vendored crate"], [evidence.registry, "crates.io", "registry crate"]]) {
    for (const [name, versions] of map) {
      for (const version of versions) {
        if (!sections.has(`${origin}\0${name} ${version}`)) missing.add(`module carries ${kind} ${name} ${version} that THIRD-PARTY-NOTICES.txt does not cover`);
      }
    }
  }
  for (const root of evidence.roots) {
    if (root === "__rustc") {
      if (!standard) missing.add("module carries crate root __rustc that THIRD-PARTY-NOTICES.txt does not cover");
    } else if (!context.members.has(root) && !covered.has(root)) missing.add(`module carries crate root ${root} that THIRD-PARTY-NOTICES.txt does not cover`);
  }
  requireValue(missing.size === 0, [...missing].join("\n"));
}
