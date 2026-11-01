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
assert.ok(args.length === 0 || (args.length === 2 && args[0] === "--consumer"), "usage: check-types.mjs [--consumer DIR]");
const consumer = args.length === 2 ? realpathSync(resolve(args[1])) : null;
const directory = mkdtempSync(join(consumer ?? join(packageRoot, "test"), ".coreav-types-"));
const cases = [
  { name: "root-node", fixture: "types.mts", workers: false },
  { name: "workers", fixture: "types-workers.mts", workers: true },
  {
    name: "invalid-root-node",
    workers: false,
    setup: [
      'import { bytesSource, probe, ffprobe } from "@coreav/wasm";',
      'const source = bytesSource(new Uint8Array());',
    ],
    invalid: [
      'import { r2Source as rootR2Source } from "@coreav/wasm";',
      'import { r2Source as nodeR2Source } from "@coreav/wasm/node";',
      'import "@coreav/wasm/src/api.js";',
      'probe(source, { filename: "file.mp4" });',
      'probe(source, { format: "native" });',
      'ffprobe(source, { allow: ["h264"] });',
      'ffprobe(source, { scanFragments: true });',
    ],
  },
  {
    name: "invalid-workers",
    workers: true,
    setup: [
      'import { r2Source } from "@coreav/wasm/workers";',
      'declare const kv: KVNamespace;',
      'declare const bucket: R2Bucket;',
    ],
    invalid: ['r2Source(kv, "key");', 'r2Source(bucket, 1);'],
  },
];

function within(directory, path) {
  const suffix = relative(directory, path);
  return suffix === "" || (!isAbsolute(suffix) && suffix !== ".." && !suffix.startsWith(`..${sep}`));
}

function checkResolution(output, workers) {
  if (!consumer) return;
  const source = realpathSync(join(consumer, "node_modules/@coreav/wasm/src"));
  assert.ok(within(consumer, source), "the installed declarations must not link outside the consumer");
  const declarations = output.split(/\r?\n/).filter(line => isAbsolute(line) && line.endsWith(".d.ts")).map(path => realpathSync(path));
  const packageDeclarations = declarations.filter(path => within(join(packageRoot, "src"), path) || path.includes(`${sep}${join("node_modules", "@coreav", "wasm")}${sep}`));
  assert.ok(packageDeclarations.length > 0, "--listFiles did not report installed package declarations");
  for (const path of packageDeclarations) {
    assert.ok(within(source, path), `declaration escaped the installed package: ${path}`);
    assert.ok(!within(join(packageRoot, "src"), path), `resolved a repository declaration: ${path}`);
  }
  for (const name of workers ? ["index", "workers"] : ["index", "node"]) {
    assert.ok(declarations.includes(join(source, `${name}.d.ts`)), `--listFiles did not resolve installed ${name}.d.ts`);
  }
}

try {
  for (const test of cases) {
    const file = join(directory, `${test.name}.mts`);
    if (test.fixture) {
      copyFileSync(join(packageRoot, "test", test.fixture), file);
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
