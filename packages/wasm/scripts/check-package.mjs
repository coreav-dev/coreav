import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, existsSync, lstatSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gunzipSync } from "node:zlib";
import { licensePolicy, permitted } from "../../../scripts/notices/render.mjs";
import { checkNotices } from "../../../scripts/notices/main.mjs";
import { noticeSections, validateEvidence } from "../../../scripts/notices/evidence.mjs";

export const PACKED = Object.freeze([
  "package/LICENSE-APACHE", "package/LICENSE-MIT", "package/NOTICE", "package/README.md",
  "package/THIRD-PARTY-NOTICES.txt", "package/package.json",
  "package/src/api.js", "package/src/engine.js", "package/src/errors.js", "package/src/index.d.ts",
  "package/src/index.js", "package/src/internal.js", "package/src/node.d.ts", "package/src/node.js",
  "package/src/r2.js", "package/src/sources.js", "package/src/workers.d.ts", "package/src/workers.js",
  "package/wasm/build.js", "package/wasm/coreav.js", "package/wasm/coreav_bg.wasm",
]);
export const SIZE_BAR = 1_048_576;
const packageRoot = realpathSync(fileURLToPath(new URL("..", import.meta.url)));
const repo = resolve(packageRoot, "../..");
const sha = bytes => createHash("sha256").update(bytes).digest("hex");

function within(parent, path) {
  const suffix = relative(parent, path);
  return suffix === "" || (!isAbsolute(suffix) && suffix !== ".." && !suffix.startsWith(`..${sep}`));
}

function octal(bytes, label) {
  const value = bytes.toString("ascii").replace(/\0.*$/, "").trim();
  assert.match(value, /^[0-7]+$/, `invalid tar ${label}`);
  const number = Number.parseInt(value, 8);
  assert.ok(Number.isSafeInteger(number), `oversized tar ${label}`);
  return number;
}

function field(bytes) {
  const nul = bytes.indexOf(0);
  if (nul !== -1) assert.ok(bytes.subarray(nul).every(byte => byte === 0), "nonzero tar string padding");
  return new TextDecoder("utf-8", { fatal: true }).decode(nul === -1 ? bytes : bytes.subarray(0, nul));
}

function safePath(path) {
  assert.ok(path.startsWith("package/") || path === "package", "archive entry must be inside package/");
  assert.ok(!/[\\\x00-\x1f\x7f]/.test(path), "archive entry has a control character or backslash");
  assert.ok(!path.startsWith("/") && !path.split("/").some(part => part === ".." || part === "." || part === ""), "archive entry has a noncanonical or traversing path");
}

/** Decode only the regular ustar files emitted for this package's roster. */
export function archiveEntries(compressed) {
  const tar = gunzipSync(compressed, { maxOutputLength: 64 << 20 });
  const entries = [];
  let at = 0;
  while (at + 512 <= tar.length) {
    const header = tar.subarray(at, at + 512);
    if (header.every(byte => byte === 0)) {
      assert.ok(tar.length - at >= 1024 && tar.subarray(at).every(byte => byte === 0), "tar must end with zero blocks only");
      return entries;
    }
    const checksum = header.reduce((sum, byte, index) => sum + (index >= 148 && index < 156 ? 32 : byte), 0);
    assert.equal(octal(header.subarray(148, 156), "checksum"), checksum, "tar header checksum mismatch");
    assert.equal(header.subarray(257, 263).toString("ascii"), "ustar\0", "archive must use ustar headers");
    assert.equal(header.subarray(263, 265).toString("ascii"), "00", "archive must use ustar version 00");
    const name = field(header.subarray(0, 100));
    const prefix = field(header.subarray(345, 500));
    const path = prefix ? `${prefix}/${name}` : name;
    const type = String.fromCharCode(header[156]);
    assert.equal(type, "0", `only regular files may be packed: ${path} has type ${JSON.stringify(type)}`);
    const mode = octal(header.subarray(100, 108), `${path} mode`);
    const size = octal(header.subarray(124, 136), `${path} size`);
    const end = at + 512 + size;
    const next = at + 512 + Math.ceil(size / 512) * 512;
    assert.ok(next <= tar.length, "truncated tar entry");
    const bytes = tar.subarray(at + 512, end);
    assert.ok(tar.subarray(end, next).every(byte => byte === 0), "nonzero tar entry padding");
    assert.equal(field(header.subarray(157, 257)), "", "archive link targets are forbidden");
    safePath(path);
    entries.push({ path, mode, size, bytes });
    at = next;
  }
  assert.fail("tar is missing its end blocks");
}

/** Validate the entire roster before npm is allowed to extract anything. */
export function validateArchive(entries) {
  const seen = new Set();
  const files = new Map();
  for (const entry of entries) {
    const { path, mode, size, bytes } = entry;
    safePath(path);
    assert.ok(!seen.has(path), `duplicate archive entry: ${path}`);
    seen.add(path);
    assert.ok(Number.isSafeInteger(mode) && mode >= 0 && mode <= 0o7777 && !(mode & 0o7133), `unsafe archive mode: ${path}`);
    assert.ok(!/\.env|\.dev\.vars|wrangler|node_modules|\.tgz|(?:^|\/)(?:test|examples|scripts)(?:\/|$)/i.test(path), `forbidden packed file: ${path}`);
    assert.ok(Number.isSafeInteger(size) && size >= 0 && bytes.byteLength === size, `incorrect archive size: ${path}`);
    assert.ok(PACKED.includes(path), `unexpected packed file: ${path}`);
    files.set(path, bytes);
  }
  assert.deepEqual([...files.keys()].sort(), [...PACKED].sort(), "packed file roster differs");
  return files;
}

export function validateManifest(manifest, allowed) {
  const keys = ["name", "version", "description", "license", "repository", "private", "type", "exports", "files", "engines", "scripts", "devDependencies"];
  assert.deepEqual(Object.keys(manifest).sort(), keys.sort(), "unexpected packed manifest fields (runtime dependencies and publishing hooks are forbidden)");
  assert.equal(manifest.name, "@coreav/wasm");
  assert.equal(manifest.private, true, "the package must remain private");
  assert.equal(manifest.type, "module");
  assert.equal(manifest.license, "MIT OR Apache-2.0");
  assert.ok(permitted(manifest.license, allowed), "package licence is not permitted by deny.toml");
  assert.deepEqual(manifest.engines, { node: ">=24" });
  assert.deepEqual(manifest.exports, {
    ".": { types: "./src/index.d.ts", default: "./src/index.js" },
    "./workers": { types: "./src/workers.d.ts", default: "./src/workers.js" },
    "./node": { types: "./src/node.d.ts", default: "./src/node.js" },
  }, "the public exports must match the approved entries");
  assert.deepEqual([...manifest.files].sort(), ["src", "wasm/coreav.js", "wasm/build.js", "wasm/coreav_bg.wasm", "README.md", "LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD-PARTY-NOTICES.txt"].sort());
  assert.deepEqual(Object.keys(manifest.scripts).sort(), ["build", "test", "test:module", "test:types", "check:package", "prepack"].sort(), "unexpected lifecycle or package script");
}

export function validateProvenance(provenance, wasm, fingerprint, pins) {
  const hash = sha(wasm);
  assert.equal(fingerprint.toString(), `export const BUILD = "${hash}";\n`, "run npm run build: packed module fingerprint is stale");
  assert.equal(provenance.wasmSha256, hash, "run npm run build: provenance does not describe the packed WASM");
  assert.ok(typeof provenance.wasmOpt === "string" && new RegExp(`^wasm-opt version ${pins.binaryen}(?:\\s|$)`).test(provenance.wasmOpt), "build with the pinned wasm-opt (scripts/install-binaryen.sh), then run npm run check:package");
  assert.equal(provenance.wasmBindgen, pins.bindgen, "build with the pinned wasm-bindgen");
  assert.equal(provenance.profile, "edge", "build with the edge profile");
  assert.equal(provenance.target, "wasm32-unknown-unknown", "build for wasm32-unknown-unknown");
  assert.ok(Number.isSafeInteger(provenance.simdInstructions) && provenance.simdInstructions > 0, "build with wasm-dis beside the pinned wasm-opt to verify SIMD instructions");
  assert.match(provenance.rustc, /^rustc \S+ .+$/, "missing rustc build provenance");
  assert.match(pins.notices.raw.sha256, /^[a-f0-9]{64}$/, "missing raw module build provenance");
  assert.ok(Array.isArray(pins.notices.distribution.rustflags) && pins.notices.distribution.rustflags.every((flag) => typeof flag === "string" && !flag.startsWith("--remap-path-prefix")) && pins.notices.distribution.rustflags.some((flag, index) => flag === "-C" && pins.notices.distribution.rustflags[index + 1] === "target-feature=+simd128"), "missing SIMD build flags in provenance");
  assert.ok(provenance.rustc.startsWith(`rustc ${pins.rust} `), "build with the pinned WASM Rust toolchain");
  return hash;
}

export function scanPaths(files, prefixes) {
  const classes = [
    ["home directory", prefixes.home], ["repository directory", prefixes.repo],
    ["Linux runner home", "/home/runner"], ["Unix home", "/home/"],
    ["macOS home", "/Users/"], ["Windows home", "C:\\Users"],
    ["Windows slash home", "C:/Users"],
  ];
  for (const [path, bytes] of files) {
    const texts = path.endsWith(".wasm") ? (bytes.toString("latin1").match(/[\x20-\x7e]{8,}/g) ?? []) : [bytes.toString("utf8")];
    for (const [label, prefix] of classes) {
      assert.ok(!prefix || !texts.some(text => text.includes(prefix)), `${path}: contains a ${label} path prefix`);
    }
  }
}

export function validateCompressedSize(raw, compressed) {
  assert.ok(Number.isSafeInteger(raw) && raw > 0 && Number.isSafeInteger(compressed) && compressed > 0, "invalid WASM size record");
  assert.ok(compressed <= SIZE_BAR, `shipped WASM is ${raw} bytes raw, ${compressed} bytes gzip -9 -n; limit ${SIZE_BAR}`);
}

export function consumerEnv(env = process.env) {
  const clean = Object.fromEntries(Object.entries(env).filter(([key]) => !/^npm_/i.test(key) && !["INIT_CWD", "NODE_PATH", "NODE_OPTIONS"].includes(key)));
  clean.PATH = [dirname(process.execPath), ...(clean.PATH ?? "").split(sep === "\\" ? ";" : ":").filter(path => !/node_modules[\\/]\.bin/.test(path))].join(sep === "\\" ? ";" : ":");
  clean.npm_config_offline = "true";
  clean.npm_config_audit = "false";
  clean.npm_config_fund = "false";
  clean.npm_config_update_notifier = "false";
  return clean;
}

function run(command, args, options = {}) {
  return execFileSync(command, args, { encoding: "utf8", timeout: 120_000, maxBuffer: 16 * 1024 * 1024, ...options });
}

function npmCli() {
  const candidates = [process.env.npm_execpath, ...(process.env.PATH ?? "").split(sep === "\\" ? ";" : ":").map(path => join(path, "npm"))];
  for (const candidate of candidates) {
    if (!candidate || !isAbsolute(candidate) || !existsSync(candidate)) continue;
    const path = realpathSync(candidate);
    if (path.endsWith(".js") && lstatSync(path).isFile()) return path;
  }
  throw new Error("cannot find npm's absolute CLI path; run through npm run check:package");
}

function installedFiles(directory, prefix = "") {
  return readdirSync(join(directory, prefix), { withFileTypes: true }).flatMap(entry => {
    const name = prefix ? `${prefix}/${entry.name}` : entry.name;
    assert.ok(!entry.isSymbolicLink(), `installed symlink: ${name}`);
    if (entry.isDirectory()) return installedFiles(directory, name);
    assert.ok(entry.isFile(), `installed nonregular file: ${name}`);
    return [name];
  });
}

async function checkConsumer(consumer, tarball, files, manifest, hash, npm) {
  const env = consumerEnv();
  assert.ok(!within(repo, realpathSync(consumer)), "consumer must be outside the repository");
  writeFileSync(join(consumer, "package.json"), JSON.stringify({ private: true, type: "module", dependencies: { "@coreav/wasm": pathToFileURL(tarball).href } }));
  const options = { cwd: consumer, env };
  run(process.execPath, [npm, "install", "--offline", "--no-audit", "--no-fund"], options);
  const modules = join(consumer, "node_modules");
  assert.deepEqual(readdirSync(modules).sort(), [".package-lock.json", "@coreav"].sort(), "consumer has runtime or development dependency leakage");
  assert.deepEqual(readdirSync(join(modules, "@coreav")), ["wasm"]);
  const installed = realpathSync(join(modules, "@coreav/wasm"));
  assert.ok(within(consumer, installed), "installed package resolved outside consumer");
  assert.deepEqual(installedFiles(installed).sort(), PACKED.map(path => path.slice(8)).sort(), "installed package roster differs from tarball");
  for (const [path, bytes] of files) assert.deepEqual(readFileSync(join(installed, path.slice(8))), bytes, `installed file differs: ${path}`);
  const tree = JSON.parse(run(process.execPath, [npm, "ls", "--all", "--json"], options));
  assert.deepEqual(Object.keys(tree.dependencies), ["@coreav/wasm"]);
  const dependency = tree.dependencies["@coreav/wasm"];
  assert.equal(dependency.version, manifest.version);
  assert.deepEqual(dependency.dependencies ?? {}, {}, "installed runtime graph must be empty");

  const common = `import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { realpathSync } from "node:fs";
const installed = ${JSON.stringify(installed + sep)};
for (const name of ["@coreav/wasm", "@coreav/wasm/node", "@coreav/wasm/workers"]) {
  assert.ok(realpathSync(fileURLToPath(import.meta.resolve(name))).startsWith(installed));
}
`;
  writeFileSync(join(consumer, "root.mjs"), `${common}
let fetches = 0;
globalThis.fetch = () => { fetches += 1; throw new Error("root import fetched bytes"); };
const api = await import("@coreav/wasm");
assert.throws(() => api.moduleInfo(), /not loaded/);
await assert.rejects(api.probe(api.bytesSource(new Uint8Array())), /not loaded/);
assert.equal(fetches, 0);
`);
  run(process.execPath, ["root.mjs"], options);
  const { fixtures } = await import(new URL("../examples/test/synth.mjs", import.meta.url));
  writeFileSync(join(consumer, "av.mp4"), fixtures().get("av.mp4"));
  writeFileSync(join(consumer, "node.mjs"), `${common}
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
const api = await import("@coreav/wasm/node");
const wasm = readFileSync(installed + "wasm/coreav_bg.wasm");
assert.equal(api.moduleInfo().build, createHash("sha256").update(wasm).digest("hex"));
assert.equal(api.moduleInfo().build, ${JSON.stringify(hash)});
const source = api.bytesSource(readFileSync("av.mp4"));
assert.equal(JSON.parse(await api.probeText(source)).schema, "coreav.probe/1");
const clipped = await api.clip(source, { start: "0", end: "1" });
const output = new Uint8Array(await new Response(clipped.body).arrayBuffer());
assert.ok(output.byteLength > 0);
await clipped.summary;
`);
  const flags = ["--experimental-wasm-jspi", "--experimental-wasm-exnref"];
  run(process.execPath, [...flags, "node.mjs"], options);
  writeFileSync(join(consumer, "deep.mjs"), `${common}
for (const name of ["@coreav/wasm/src/engine.js", "@coreav/wasm/wasm/coreav_bg.wasm", "@coreav/wasm/package.json"]) {
  await assert.rejects(import(name), { code: "ERR_PACKAGE_PATH_NOT_EXPORTED" });
}
`);
  run(process.execPath, [...flags, "deep.mjs"], options);
  for (const [program, message] of [[[...flags, "--input-type=module", "-e", 'delete WebAssembly.Suspending; await import("@coreav/wasm/node");'], /JSPI/], [["--experimental-wasm-jspi", "--no-experimental-wasm-exnref", "node.mjs"], /exception references/]]) {
    const refused = spawnSync(process.execPath, program, { ...options, encoding: "utf8", timeout: 30_000 });
    if (refused.error) throw refused.error;
    assert.notEqual(refused.status, 0, "installed Node loader accepted missing runtime support");
    assert.match(refused.stderr, message);
  }
  run(process.execPath, [join(packageRoot, "scripts/check-types.mjs"), "--consumer", consumer], options);
  await checkBundles(consumer, installed, hash);
}

async function checkBundles(consumer, installed, hash) {
  const require = createRequire(new URL("../examples/package.json", import.meta.url));
  const { build, stop } = await import(pathToFileURL(require.resolve("esbuild")));
  function inspect(result) {
    for (const [path, record] of Object.entries(result.metafile.inputs)) {
      assert.ok(within(consumer, realpathSync(resolve(consumer, path))), "bundle input escaped the installed consumer");
      for (const imported of record.imports) assert.ok(!imported.path.startsWith("node:"), "bundle contains a Node runtime import");
    }
    for (const output of Object.values(result.metafile.outputs)) {
      for (const imported of output.imports) assert.equal(imported.path, "./coreav_bg.wasm", "unexpected external bundle import");
    }
  }
  try {
    const root = await build({ absWorkingDir: consumer, entryPoints: [join(installed, "src/index.js")], bundle: true, write: false, outfile: join(consumer, "root-bundle.js"), format: "esm", platform: "neutral", metafile: true, logLevel: "silent" });
    inspect(root);
    assert.ok(!Object.keys(root.metafile.inputs).some(path => path.endsWith(".wasm")), "root graph must not import WASM");
    writeFileSync(join(consumer, "worker.mjs"), 'export * from "@coreav/wasm/workers";\n');
    const modules = [];
    const worker = await build({
      absWorkingDir: consumer, entryPoints: [join(consumer, "worker.mjs")], bundle: true, write: false,
      outfile: join(consumer, "worker-bundle.js"), format: "esm", platform: "neutral", metafile: true, logLevel: "silent",
      plugins: [{ name: "compiled-wasm", setup(builder) {
        builder.onResolve({ filter: /\.wasm$/ }, args => {
          const path = realpathSync(resolve(args.resolveDir, args.path));
          assert.ok(within(installed, path), "Workers WASM escaped the installed package");
          assert.equal(sha(readFileSync(path)), hash, "Workers bundle loaded different WASM bytes");
          modules.push(path);
          return { path: "./coreav_bg.wasm", external: true };
        });
      } }],
    });
    inspect(worker);
    assert.equal(modules.length, 1, "Workers bundle must import exactly one compiled WASM module");
    assert.ok(!Object.keys(worker.metafile.inputs).some(path => path.endsWith(".wasm")), "Workers bundle inlined the WASM");
    assert.deepEqual(Object.values(worker.metafile.outputs).flatMap(output => output.imports).map(imported => ({ path: imported.path, external: imported.external })), [{ path: "./coreav_bg.wasm", external: true }]);
  } finally {
    stop();
  }
}

export async function main(args = process.argv.slice(2)) {
  assert.ok(Number(process.versions.node.split(".")[0]) >= 24, "check:package needs Node.js 24 or later");
  assert.ok(args.length === 0 || (args.length === 2 && args[0] === "--tarball"), "usage: check-package.mjs [--tarball PATH]");
  const scratch = realpathSync(mkdtempSync(join(tmpdir(), "coreav-package-")));
  const consumer = realpathSync(mkdtempSync(join(tmpdir(), "coreav-consumer-")));
  try {
    const env = consumerEnv();
    const npm = npmCli();
    let tarball;
    if (args.length) tarball = realpathSync(resolve(args[1]));
    else {
      const packed = JSON.parse(run(process.execPath, [npm, "pack", "--offline", "--json", "--pack-destination", scratch], { cwd: packageRoot, env }));
      assert.equal(packed.length, 1, "npm pack must produce one tarball");
      assert.equal(basename(packed[0].filename), packed[0].filename, "npm pack returned a nonlocal tarball name");
      tarball = join(scratch, packed[0].filename);
    }
    const compressed = readFileSync(tarball);
    const files = validateArchive(archiveEntries(compressed));
    const checkedTarball = join(scratch, "checked.tgz");
    writeFileSync(checkedTarball, compressed);
    const allowed = licensePolicy(repo).allowed;
    const manifest = JSON.parse(files.get("package/package.json"));
    validateManifest(manifest, allowed);
    scanPaths(files, { home: homedir(), repo });
    const wasm = files.get("package/wasm/coreav_bg.wasm");
    const glue = files.get("package/wasm/coreav.js");
    const binaryen = /^BINARYEN_VERSION=([0-9]+)$/m.exec(readFileSync(join(repo, "scripts/binaryen.env"), "utf8"));
    const bindgen = /^wasm-bindgen = "=([^"]+)"$/m.exec(readFileSync(join(repo, "crates/coreav-wasm/Cargo.toml"), "utf8"));
    assert.ok(binaryen && bindgen, "missing tool pins");
    const provenance = JSON.parse(readFileSync(join(packageRoot, "wasm/provenance.json"), "utf8"));
    const expected = checkNotices(packageRoot, repo);
    const hash = validateProvenance(provenance, wasm, files.get("package/wasm/build.js"), { binaryen: binaryen[1], bindgen: bindgen[1], rust: expected.tc.version, notices: expected.record });
    for (const name of ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE"]) assert.deepEqual(files.get(`package/${name}`), readFileSync(join(repo, name)), `run npm run build: ${name} differs from the repository`);
    const notices = files.get("package/THIRD-PARTY-NOTICES.txt").toString("utf8");
    const sections = noticeSections(notices);
    validateEvidence(expected.evidence, sections, expected.context);
    assert.equal(notices, expected.text, "run npm run build: third-party notices are stale");
    const gzip = bytes => execFileSync("gzip", ["-9", "-n", "-c"], { input: bytes, maxBuffer: 64 * 1024 * 1024, timeout: 30_000 }).length;
    const wasmGzip = gzip(wasm);
    validateCompressedSize(wasm.length, wasmGzip);
    await checkConsumer(consumer, checkedTarball, files, manifest, hash, npm);
    const evidence = Object.fromEntries(Object.entries(expected.evidence).map(([name, value]) => [name, value instanceof Map ? [...value].map(([crate, versions]) => ({ name: crate, versions: [...versions].sort() })) : [...value].sort()]));
    const record = {
      wasmBytes: wasm.length, wasmGzipBytes: wasmGzip, glueBytes: glue.length, glueGzipBytes: gzip(glue),
      sourceBytes: [...files].filter(([path]) => path.startsWith("package/src/")).reduce((sum, [, bytes]) => sum + bytes.length, 0),
      tarballBytes: compressed.length, tarballSha256: sha(compressed), unpackedBytes: [...files.values()].reduce((sum, bytes) => sum + bytes.length, 0),
      sha256: Object.fromEntries([...files].map(([path, bytes]) => [path.slice(8), sha(bytes)])),
      noticeBytes: Buffer.byteLength(notices), noticeGzipBytes: gzip(Buffer.from(notices)),
      noticeSections: Object.fromEntries(["crates.io", "Rust toolchain", "Rust toolchain, crates.io"].map((origin) => [origin, [...sections.values()].filter((section) => section.origin === origin).length])), evidence,
      node: process.versions.node, npm: run(process.execPath, [npm, "--version"], { env }).trim(), provenance,
    };
    console.log(JSON.stringify(record));
    if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `\n### Installed WASM package\n\n\`\`\`json\n${JSON.stringify(record, null, 2)}\n\`\`\`\n`);
    return record;
  } finally {
    rmSync(consumer, { recursive: true, force: true });
    rmSync(scratch, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
