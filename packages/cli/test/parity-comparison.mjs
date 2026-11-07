import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(packageDir, "../..");
const order = [1, 4, 4, 1];
const scratch = mkdtempSync(join(tmpdir(), "coreav-parity-comparison-"));

function identities() {
  const files = [];
  function add(path, label) {
    if (statSync(path).isDirectory()) {
      for (const name of readdirSync(path).sort()) add(join(path, name), `${label}/${name}`);
    } else {
      files.push({ name: label, bytes: statSync(path).size, sha256: createHash("sha256").update(readFileSync(path)).digest("hex") });
    }
  }
  const dirs = (process.env.COREAV_FIXTURE_DIRS ?? "").split(",").filter(Boolean);
  assert.ok(process.env.COREAV_BIN && dirs.length, "comparison requires native binary and complete fixtures");
  dirs.forEach((dir, index) => add(dir, `fixtures-${index}`));
  for (const path of ["test", "lib", "bin", "install.js", "package.json", "wasm/coreav.wasm"]) add(join(packageDir, path), `package/${path}`);
  add(join(repo, "docs/schema/policy-examples"), "policies");
  add(join(repo, "Cargo.toml"), "Cargo.toml");
  add(process.env.COREAV_BIN, "native");
  add(process.execPath, "node");
  return files;
}

try {
  const before = identities();
  const digest = createHash("sha256").update(JSON.stringify(before)).digest("hex");
  const destination = process.env.COREAV_PARITY_COMPARISON_REPORT ?? join(packageDir, "parity-comparison.json");
  const report = { kind: "CI harness comparison, not FFmpeg speed", order, inputDigest: digest, manifest: before, runs: [] };
  const deadline = performance.now() + 70 * 60 * 1000;
  writeFileSync(destination, JSON.stringify(report, null, 2));
  for (const [index, workers] of order.entries()) {
    assert.deepEqual(identities(), before, "shared input identities changed before test");
    const summary = join(scratch, `${index}.json`);
    const start = performance.now();
    const remaining = Math.floor(deadline - start);
    assert.ok(remaining > 0, "comparison exceeded its 70 minute execution budget");
    await new Promise((resolveRun, reject) => {
      const child = spawn("npm", ["test"], {
        cwd: packageDir,
        env: { ...process.env, COREAV_PARITY_WORKERS: String(workers), COREAV_PARITY_SUMMARY: summary },
        stdio: "inherit",
        detached: true,
      });
      const timer = setTimeout(() => process.kill(-child.pid, "SIGKILL"), remaining);
      child.on("error", (error) => { clearTimeout(timer); reject(error); });
      child.on("close", (code, signal) => {
        clearTimeout(timer);
        if (code === 0 && !signal) resolveRun();
        else reject(new Error(`npm test failed: code=${code}, signal=${signal}`));
      });
    });
    const elapsedSeconds = (performance.now() - start) / 1000;
    const counts = JSON.parse(readFileSync(summary, "utf8"));
    assert.ok(counts.compared > 0 && counts.outputs > 50 && counts.succeeded.length > 0);
    if (report.runs.length) assert.deepEqual(counts, report.runs[0].counts, "serial and parallel complete coverage differs");
    assert.deepEqual(identities(), before, "shared input identities changed after test");
    report.runs.push({ workers, elapsedSeconds, counts, inputDigestBefore: digest, inputDigestAfter: digest });
    writeFileSync(destination, JSON.stringify(report, null, 2));
  }
  const serial = report.runs.filter((run) => run.workers === 1).reduce((sum, run) => sum + run.elapsedSeconds, 0);
  const parallel = report.runs.filter((run) => run.workers === 4).reduce((sum, run) => sum + run.elapsedSeconds, 0);
  report.serialOverParallelCiHarnessRatio = serial / parallel;
  writeFileSync(destination, JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report));
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `\nCI harness comparison (not FFmpeg speed). Shared input SHA-256: \`${digest}\`. Fixed order: 1, 4, 4, 1 workers.\n\n| Workers | npm test seconds | Comparisons | Outputs |\n|---|---|---|---|\n${report.runs.map((run) => `| ${run.workers} | ${run.elapsedSeconds.toFixed(3)} | ${run.counts.compared} | ${run.counts.outputs} |`).join("\n")}\n\nSerial/parallel CI harness elapsed ratio: ${report.serialOverParallelCiHarnessRatio.toFixed(3)}.\n`);
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
