import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative } from "node:path";

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
      const path = relative(repo, dirname(pkg.manifest_path)).replaceAll("\\", "/");
      if (pkg.license !== "MIT OR Apache-2.0" || path === ".." || path.startsWith("../") || isAbsolute(path)) throw new Error(`${name} ${version}: workspace licence or location is not covered by the root files`);
      continue;
    }
    if (!pkg.source?.startsWith("registry+")) throw new Error(`${name} ${version}: third-party source is not a registry`);
    if (pkg.source !== "registry+https://github.com/rust-lang/crates.io-index") throw new Error(`${name} ${version}: third-party source is not crates.io`);
    found.set(pkg.id, { name, version, license: pkg.license, dir: dirname(pkg.manifest_path) });
  }
  return [...found.values()].sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : a.version > b.version ? 1 : 0);
}

export function prepareFlags(repo, tc, distribution, env = process.env) {
  if (env.CARGO_ENCODED_RUSTFLAGS) throw new Error("use RUSTFLAGS rather than CARGO_ENCODED_RUSTFLAGS before notices prepare");
  const flags = (env.RUSTFLAGS ?? "").split(/\s+/).filter(Boolean);
  const options = { env: { ...tc.env, CARGO_ENCODED_RUSTFLAGS: flags.join("\x1f") }, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 };
  const args = ["metadata", "--locked", "--offline", "--all-features", "--format-version=1", "--filter-platform", tc.target, "--manifest-path", join(repo, "Cargo.toml")];
  let metadata;
  try { metadata = JSON.parse(execFileSync("cargo", args, { ...options, stdio: ["ignore", "pipe", "pipe"] })); }
  catch {
    execFileSync("cargo", ["fetch", "--locked", "--manifest-path", join(repo, "Cargo.toml")], { ...options, stdio: "inherit" });
    metadata = JSON.parse(execFileSync("cargo", args, options));
  }
  const prefixes = new Map();
  for (const pkg of metadata.packages.filter(pkg => pkg.source === "registry+https://github.com/rust-lang/crates.io-index")) {
    const registry = dirname(dirname(dirname(pkg.manifest_path)));
    if (!registry.replaceAll("\\", "/").endsWith("/registry/src")) throw new Error("distribution builds require the locked Cargo registry cache");
    prefixes.set(registry, "/cargo/registry");
    prefixes.set(join(dirname(dirname(registry)), "git/checkouts"), "/cargo/git");
  }
  prefixes.set(metadata.workspace_root, "/coreav");
  prefixes.set(tc.library, "/rustc/" + tc.commit + "/library");
  prefixes.set(join(tc.library, "vendor"), "/rust/deps");
  prefixes.set(join(metadata.target_directory, tc.target, distribution.profile, "build"), "/build");
  for (const [from, to] of prefixes) flags.push("--remap-path-prefix=" + from + "=" + to);
  return flags;
}

export function projectGraph(repo, tc, distribution) {
  const common = ["--locked", "--offline", "--manifest-path", join(repo, "Cargo.toml")];
  const options = { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env: { ...tc.env, CARGO_ENCODED_RUSTFLAGS: distribution.rustflags.join("\x1f") } };
  const metadata = JSON.parse(execFileSync("cargo", ["metadata", ...common, "--all-features", "--format-version=1", "--filter-platform", tc.target], options));
  const args = ["tree", ...common, "-p", distribution.package, "--target", tc.target, "-e", "normal,no-proc-macro", "--prefix", "none", "--format", "{p}", "--no-dedupe"];
  if (distribution.features.length) args.push("--features", distribution.features.join(","));
  if (!distribution.defaultFeatures) args.push("--no-default-features");
  const tree = execFileSync("cargo", args, options);
  const members = metadata.packages.filter(pkg => metadata.workspace_members.includes(pkg.id));
  return { crates: selectCrates(metadata, tree, repo), members: new Set(members.flatMap(pkg => pkg.targets.map(target => target.name.replaceAll("-", "_")))), memberPaths: new Set(members.map(pkg => relative(repo, dirname(pkg.manifest_path)).replaceAll("\\", "/"))) };
}

export function toolchain(repo, target, env = process.env) {
  if (!/^[a-z0-9_-]+$/.test(target)) throw new Error("choose a Rust distribution target");
  const pin = readFileSync(join(repo, "scripts/rust-dist.env"), "utf8");
  const version = /^RUST_DIST_TOOLCHAIN=([0-9.]+)$/m.exec(pin)?.[1];
  const commit = /^RUST_DIST_COMMIT=([a-f0-9]{40})$/m.exec(pin)?.[1];
  if (!version || !commit) throw new Error("read the distribution Rust version and commit from scripts/rust-dist.env");
  const pinned = { ...env, RUSTUP_TOOLCHAIN: version };
  const install = "rustup toolchain install " + version + " --profile minimal --target " + target + " --component rust-src";
  let details, sysroot;
  try {
    details = execFileSync("rustc", ["-vV"], { env: pinned, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    sysroot = execFileSync("rustc", ["--print", "sysroot"], { env: pinned, encoding: "utf8" }).trim();
  } catch { throw new Error("install Rust " + version + ": " + install); }
  if (!details.includes("commit-hash: " + commit + "\n") || !details.includes("release: " + version + "\n")) throw new Error("Rust " + version + ": compiler commit differs from scripts/rust-dist.env");
  const library = join(sysroot, "lib/rustlib/src/rust/library"), targetLib = join(sysroot, "lib/rustlib", target, "lib");
  if (!existsSync(join(library, "Cargo.lock"))) throw new Error("rust-src for " + version + " is missing: " + install);
  if (!existsSync(targetLib)) throw new Error("Rust " + version + " target " + target + " is missing: " + install);
  return { version, commit, sysroot, library, target, targetLib, env: pinned };
}

export function inTreeCrates(library, packages) {
  const directories = readdirSync(library, { withFileTypes: true }).filter(entry => entry.isDirectory() && entry.name !== "vendor");
  return packages.map(crate => {
    const roots = directories.filter(entry => entry.name.replaceAll("-", "_") === crate.name.replaceAll("-", "_"));
    const found = [];
    for (const entry of roots) {
      const directory = join(library, entry.name);
      const candidates = [directory, ...readdirSync(directory, { withFileTypes: true }).filter(child => child.isDirectory() && child.name.replaceAll("-", "_") === crate.name.replaceAll("-", "_")).map(child => join(directory, child.name))];
      for (const candidate of candidates) {
        if (!existsSync(join(candidate, "Cargo.toml"))) continue;
        const text = readFileSync(join(candidate, "Cargo.toml"), "utf8");
        const pkg = /(?:^|\n)\[package\]\s*\n([\s\S]*?)(?=\n\[|$)/.exec(text)?.[1];
        if (pkg && field(pkg, "name") === crate.name && field(pkg, "version") === crate.version) found.push({ ...crate, dir: relative(library, candidate).replaceAll("\\", "/"), license: field(pkg, "license", true) });
      }
    }
    if (found.length !== 1) throw new Error(crate.name + " " + crate.version + ": in-tree identity or licence metadata missing or ambiguous");
    return found[0];
  });
}

export function field(text, name, optional = false) {
  const value = new RegExp(`^${name}\\s*=\\s*("[^\\n]*"|'[^'\\n]*')\\s*$`, "m").exec(text);
  if (value) return value[1].startsWith("'") ? value[1].slice(1, -1) : JSON.parse(value[1]);
  if (optional && !new RegExp("^" + name + "\\s*=", "m").test(text)) return null;
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

export function vendoredCrates(library, packages, repo, read = (path) => readFileSync(path)) {
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
