import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = realpathSync(fileURLToPath(new URL("..", import.meta.url)));
const compiler = fileURLToPath(new URL("../node_modules/typescript/bin/tsc", import.meta.url));
const require = createRequire(import.meta.url);
const workersPackage = require.resolve("@cloudflare/workers-types/package.json");
const workersMetadata = JSON.parse(readFileSync(workersPackage, "utf8"));
const workersEntry = resolve(dirname(workersPackage), workersMetadata.types ?? "index.d.ts");
const args = process.argv.slice(2);
let consumer = null;
let packageName = "@coreav/wasm";
for (let index = 0; index < args.length; index += 1) {
  if (args[index] === "--consumer" && args[index + 1]) consumer = realpathSync(resolve(args[++index]));
  else if (args[index] === "--package" && args[index + 1]) packageName = args[++index];
  else throw new Error("usage: check-types.mjs [--package NAME] [--consumer DIR]");
}
assert.ok(packageName.startsWith("@") && packageName.includes("/"), "package name must be a scoped package");
export function rewritePackage(text, name = packageName) {
  if (name === "@coreav/wasm") return text;
  return text.replaceAll("@coreav/wasm", name);
}
const cases = [
  { name: "root-node", fixture: "types.mts", workers: false },
  { name: "workers", fixture: "types-workers.mts", workers: true },
  {
    name: "invalid-root-node",
    workers: false,
    setup: [
      'import { bytesSource, probe, ffprobe } from "@coreav/wasm";',
      'const source = bytesSource(new Uint8Array());',
      ...(packageName.endsWith("wasm-h264") ? ['import { thumbnail } from "@coreav/wasm-h264";'] : []),
    ].map(line => rewritePackage(line)),
    invalid: [
      'import { r2Source as rootR2Source } from "@coreav/wasm";',
      'import { r2Source as nodeR2Source } from "@coreav/wasm/node";',
      'import "@coreav/wasm/src/api.js";',
      'probe(source, { filename: "file.mp4" });',
      'probe(source, { format: "native" });',
      'ffprobe(source, { allow: ["h264"] });',
      'ffprobe(source, { scanFragments: true });',
      ...(packageName.endsWith("wasm-h264") ? [
        'import { diagnostics } from "@coreav/wasm-h264";',
        'thumbnail(source, { width: 1 });',
      ] : []),
    ].map(line => rewritePackage(line)),
  },
  {
    name: "invalid-workers",
    workers: true,
    setup: [
      'import { r2Source } from "@coreav/wasm/workers";'.replaceAll("@coreav/wasm", packageName),
      'declare const kv: KVNamespace;',
      'declare const bucket: R2Bucket;',
    ],
    invalid: ['r2Source(kv, "key");', 'r2Source(bucket, 1);'].concat(packageName.endsWith("wasm-h264") ? ['import { diagnostics } from "@coreav/wasm-h264";'] : []),
  },
];

function within(directory, path) {
  const suffix = relative(directory, path);
  return suffix === "" || (!isAbsolute(suffix) && suffix !== ".." && !suffix.startsWith(`..${sep}`));
}

function checkResolution(output, workers) {
  if (!consumer) return;
  const installedPackage = join(consumer, "node_modules", ...packageName.split("/"));
  const source = realpathSync(join(installedPackage, "src"));
  assert.ok(within(consumer, source), "the installed declarations must not link outside the consumer");
  const declarations = output.split(/\r?\n/).filter(line => isAbsolute(line) && line.endsWith(".d.ts")).map(path => realpathSync(path));
  const packageDeclarations = declarations.filter(path => within(join(packageRoot, "src"), path) || path.includes(`${sep}${join(installedPackage)}${sep}`) || path.startsWith(installedPackage + sep));
  assert.ok(packageDeclarations.length > 0, "--listFiles did not report installed package declarations");
  for (const path of packageDeclarations) {
    assert.ok(within(source, path), `declaration escaped the installed package: ${path}`);
    assert.ok(!within(join(packageRoot, "src"), path), `resolved a repository declaration: ${path}`);
  }
  for (const name of workers ? ["index", "workers"] : ["index", "node"]) {
    assert.ok(declarations.includes(join(source, `${name}.d.ts`)), `--listFiles did not resolve installed ${name}.d.ts`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
const directory = mkdtempSync(join(consumer ?? join(packageRoot, "test"), ".coreav-types-"));
try {
  for (const test of cases) {
    const file = join(directory, `${test.name}.mts`);
    if (test.fixture) {
      let text = rewritePackage(readFileSync(join(packageRoot, "test", test.fixture), "utf8"));
      if (packageName.endsWith("wasm-h264") && test.name === "root-node") {
        text += `\nimport { thumbnail } from ${JSON.stringify(packageName)};\nimport { thumbnail as nodeThumbnail } from ${JSON.stringify(`${packageName}/node`)};\nawait thumbnail(source, { at: "1", format: "jpeg", filter: "bicubic", chroma: "full" });\nawait nodeThumbnail(source, { at: 1, height: 8, format: "jpg" });\n`;
      }
      if (packageName.endsWith("wasm-h264") && test.name === "workers") {
        text += `\nimport { bytesSource as workerBytes, thumbnail as workerThumbnail } from ${JSON.stringify(`${packageName}/workers`)};\nawait workerThumbnail(workerBytes(new Uint8Array()), { at: "0", filter: "area", chroma: "native" });\n`;
      }
      writeFileSync(file, text);
    } else {
      assert.ok(test.invalid.every(line => !line.includes("\n")), "each invalid case must occupy one line");
      writeFileSync(file, [...test.setup, ...test.invalid, ""].join("\n"));
    }
    const config = join(directory, `${test.name}.json`);
    writeFileSync(config, JSON.stringify({
      compilerOptions: {
        noEmit: true,
        strict: true,
        target: "ES2022",
        module: "NodeNext",
        types: [],
        listFiles: true,
        noUncheckedSideEffectImports: true,
        ...(test.workers ? { lib: ["ES2022"] } : {}),
      },
      files: test.workers ? [file, workersEntry] : [file],
    }));
    const checked = spawnSync(process.execPath, [compiler, "--project", config, "--pretty", "false"], {
      cwd: consumer ?? packageRoot,
      encoding: "utf8",
    });
    if (checked.error) throw checked.error;
    const output = checked.stdout + checked.stderr;
    assert.equal(checked.status, test.invalid ? 2 : 0, output);
    if (test.invalid) {
      const diagnostics = [...output.matchAll(/^(.*)\((\d+),(\d+)\): error TS\d+:/gm)];
      assert.equal(diagnostics.length, [...output.matchAll(/^.*error TS\d+:/gm)].length, output);
      const actual = new Set(diagnostics.map(match => `${resolve(consumer ?? packageRoot, match[1])}:${match[2]}`));
      const expected = new Set(test.invalid.map((_, index) => `${file}:${test.setup.length + index + 1}`));
      assert.deepEqual(actual, expected, `each invalid case must fail on its own line:\n${output}`);
    }
    checkResolution(output, test.workers);
    console.log(`Type contract passed: ${test.name}${consumer ? " (installed)" : " (self-reference)"}`);
  }
} finally {
  rmSync(directory, { recursive: true, force: true });
}
}
