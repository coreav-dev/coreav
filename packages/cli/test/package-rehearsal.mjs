import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, renameSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const artifacts = resolve(process.argv[2]);
mkdirSync(artifacts, { recursive: true });
const platform = `${process.platform}-${process.arch}`;
assert.equal(platform, process.env.PLATFORM);
const fixture = fileURLToPath(new URL("../../../crates/coreav-audio/fuzz/regressions/decode/edit-rounding.mp4", import.meta.url));
const version = JSON.parse(readFileSync(new URL("../package.json", import.meta.url))).version;
const npmCli = process.platform === "win32"
  ? join(dirname(process.execPath), "node_modules/npm/bin/npm-cli.js")
  : realpathSync(execFileSync("which", ["npm"], { encoding: "utf8" }).trim());
const env = { ...process.env, COREAV_STRICT_SAFE: "1" };
delete env.COREAV_BINARY;
const results = [];

function run(program, args, cwd) {
  console.log(JSON.stringify([program, ...args]));
  return execFileSync(program, args, { cwd, env, encoding: "utf8", timeout: 120_000, maxBuffer: 8 << 20 });
}

function only(directory, matches) {
  const names = readdirSync(directory).filter(matches);
  assert.equal(names.length, 1, `expected one matching artifact in ${directory}, found ${names}`);
  return join(directory, names[0]);
}

function cli(program, prefix, cwd) {
  const printedVersion = run(program, [...prefix, "--version"], cwd).trim();
  assert.equal(printedVersion, `coreav ${version}`);
  copyFileSync(fixture, join(cwd, "fixture.mp4"));
  const probe = name => JSON.parse(run(program, [...prefix, "ffprobe", "-v", "error", "-of", "json", "-show_streams", "-show_format", name], cwd));
  const before = probe("fixture.mp4");
  assert.ok(before.streams.length > 0);
  run(program, [...prefix, "ffmpeg", "-i", "fixture.mp4", "-c", "copy", "out.mp4"], cwd);
  assert.ok(readFileSync(join(cwd, "out.mp4")).length > 0);
  const after = probe("out.mp4");
  assert.deepEqual(after.streams.map(stream => stream.codec_name), before.streams.map(stream => stream.codec_name));
  return printedVersion;
}

function check(packageName, action) {
  const cwd = mkdtempSync(join(tmpdir(), "coreav-rehearsal-"));
  try {
    const detail = action(cwd);
    results.push({ platform, package: packageName, result: "pass", detail });
  } catch (error) {
    console.error(error);
    results.push({ platform, package: packageName, result: "fail", detail: error.message });
  }
}

for (const build of ["default", "full"]) {
  check(`archive-${build}`, cwd => {
    const directory = join(artifacts, "release");
    const archive = only(directory, name => name.startsWith(build === "full" ? "coreav-full-" : "coreav-")
      && (build === "full" || !name.startsWith("coreav-full-"))
      && name.endsWith(`-${process.env.TARGET}${process.platform === "win32" ? ".zip" : ".tar.gz"}`));
    const checksum = readFileSync(join(directory, "SHA256SUMS"), "utf8").split("\n")
      .find(line => line.endsWith(`  ${archive.split(/[\\/]/).pop()}`));
    assert.ok(checksum, "archive is missing from SHA256SUMS");
    assert.equal(createHash("sha256").update(readFileSync(archive)).digest("hex"), checksum.slice(0, 64));
    run("tar", ["-xf", archive, "-C", cwd], cwd);
    const installed = only(cwd, () => true);
    return cli(join(installed, process.platform === "win32" ? "coreav.exe" : "coreav"), [], cwd);
  });
}

for (const mode of ["native", "wasi"]) {
  check(`npm-${mode}`, cwd => {
    writeFileSync(join(cwd, "package.json"), '{"name":"rehearsal","private":true}');
    const main = only(join(artifacts, "npm-main"), name => /^coreav-\d.*\.tgz$/.test(name));
    const packages = [main];
    if (mode === "native") {
      packages.push(only(join(artifacts, platform === "linux-x64" ? "npm-main" : "npm-native"), name => name.startsWith(`coreav-cli-${platform}-`) && name.endsWith(".tgz")));
    }
    run(process.execPath, [npmCli, "install", "--offline", "--no-audit", "--no-fund", ...(mode === "wasi" ? ["--omit=optional"] : []), ...packages], cwd);
    const entry = join(cwd, "node_modules/coreav/bin/coreav");
    if (mode === "native") {
      const native = join(cwd, `node_modules/@coreav/cli-${platform}/bin/${process.platform === "win32" ? "coreav.exe" : "coreav"}`);
      if (process.platform !== "win32") assert.deepEqual(readFileSync(entry), readFileSync(native));
      else assert.ok(readFileSync(native).length > 0);
      renameSync(join(cwd, "node_modules/coreav/wasm/coreav.wasm"), join(cwd, "disabled.wasm"));
    } else {
      assert.equal(existsSync(join(cwd, `node_modules/@coreav/cli-${platform}`)), false);
      assert.equal(readFileSync(entry).subarray(0, 2).toString(), "#!");
    }
    const scripted = mode === "wasi" || process.platform === "win32";
    return cli(scripted ? process.execPath : entry, scripted ? [entry] : [], cwd);
  });
}

check("wasm-node", cwd => {
  writeFileSync(join(cwd, "package.json"), '{"name":"rehearsal","private":true}');
  const tarball = only(join(artifacts, "wasm"), name => /^coreav-wasm-.*\.tgz$/.test(name));
  run(process.execPath, [npmCli, "install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarball], cwd);
  copyFileSync(fixture, join(cwd, "fixture.mp4"));
  writeFileSync(join(cwd, "probe.mjs"), `
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { probe, bytesSource } from "@coreav/wasm/node";
const result = await probe(bytesSource(readFileSync("fixture.mp4")));
assert.equal(result.schema, "coreav.probe/1");
assert.ok(result.streams.length > 0);
console.log(JSON.stringify(result));
`);
  run(process.execPath, ["--experimental-wasm-jspi", "--experimental-wasm-exnref", "probe.mjs"], cwd);
  return "installed Node entry probed the committed fixture";
});

writeFileSync(join(artifacts, "results.json"), JSON.stringify(results, null, 2) + "\n");
const table = ["| Platform | Package | Result |", "|---|---|---|", ...results.map(row => `| ${row.platform} | ${row.package} | ${row.result} |`)].join("\n") + "\n";
console.log(table);
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, table);
if (results.some(row => row.result !== "pass")) process.exitCode = 1;
