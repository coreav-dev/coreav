import { appendFileSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { licensePolicy, renderNotices, renderToolchain } from "./render.mjs";
import { toolchain, projectGraph, toolchainCrates, vendoredCrates, inTreeCrates, prepareFlags } from "./select.mjs";
import { moduleEvidence, nativeEvidence, noticeSections, validateEvidence } from "./evidence.mjs";
import { makeRecord, verifyRecord, buildScript, NOTICE_FILE, RECORD_FILE } from "./record.mjs";

const repository = fileURLToPath(new URL("../..", import.meta.url));
const sha = bytes => createHash("sha256").update(bytes).digest("hex");

export function selection(args) {
  const result = { features: [], defaultFeatures: true };
  const keys = { "-p": "package", "--target": "target", "--profile": "profile", "--features": "features" };
  const seen = new Set();
  for (let at = 0; at < args.length; at += 1) {
    const key = args[at];
    if (key === "--no-default-features" && result.defaultFeatures) { result.defaultFeatures = false; continue; }
    if (!Object.hasOwn(keys, key) || seen.has(key) || typeof args[at + 1] !== "string") throw new Error("pass the same package, target, profile and feature selection used by cargo build");
    seen.add(key);
    const value = args[++at];
    if (!/^[A-Za-z0-9_.,/:-]+$/.test(value)) throw new Error("invalid Cargo distribution selection");
    result[keys[key]] = key === "--features" ? [...new Set(value.split(","))].sort() : value;
  }
  if (!result.package || !result.target || !result.profile || !["coreav-cli", "coreav-wasm"].includes(result.package)) throw new Error("name the package, target and profile of the distribution");
  buildScript(result);
  return result;
}

function flags(encoded) {
  if (!encoded) throw new Error("run notices prepare before building and generating notices");
  const result = [], words = encoded.split("\x1f");
  for (let at = 0; at < words.length; at += 1) {
    if (words[at] === "--remap-path-prefix") { at += 1; continue; }
    if (!words[at].startsWith("--remap-path-prefix=")) result.push(words[at]);
  }
  return result;
}

function compute(directory, repo, distribution, artifact, raw) {
  const tc = toolchain(repo, distribution.target), policy = licensePolicy(repo);
  const graph = projectGraph(repo, tc, distribution);
  const closure = toolchainCrates(readFileSync(join(tc.library, "Cargo.lock"), "utf8"), readdirSync(tc.targetLib));
  const repoNotice = readFileSync(join(repo, "NOTICE"), "utf8");
  const header = distribution.package === "coreav-wasm"
    ? "Rust dependency notices for this WebAssembly build.\nThe standard-library target closure is a superset of linker-retained code.\nCoreAV's own licence and notices are in LICENSE-MIT, LICENSE-APACHE and NOTICE.\n\n"
    : "Rust dependency notices for coreav on " + distribution.target + ".\nThe standard-library target closure is a superset of linker-retained code.\nCoreAV's own terms are in LICENSE-MIT, LICENSE-APACHE and NOTICE.\nC library notices are in NOTICE and licenses/ where shipped; C runtime coverage is described in docs/release/notices.md.\n\n";
  const text = header + renderNotices(graph.crates, policy, repoNotice) + "\n" + renderToolchain(tc, inTreeCrates(tc.library, closure.inTree), policy) + "\n" + renderNotices(vendoredCrates(tc.library, closure.vendored, repo), policy, repoNotice);
  const packed = readFileSync(artifact);
  const wasm = distribution.target.startsWith("wasm32-");
  if (wasm && !raw) throw new Error("keep the raw module for independent notice evidence");
  const evidence = wasm ? moduleEvidence(packed, readFileSync(raw)) : nativeEvidence(packed);
  const context = { version: tc.version, commit: tc.commit, members: graph.members, memberPaths: graph.memberPaths, library: new Set(readdirSync(tc.library, { withFileTypes: true }).filter(entry => entry.isDirectory()).map(entry => entry.name)) };
  const sections = noticeSections(text);
  validateEvidence(evidence, sections, context);
  const record = makeRecord(directory, repo, distribution, tc, artifact, raw, text, sections, closure);
  return { text, evidence, context, sections, record, tc };
}

export function checkNotices(directory, repo = repository, options = {}) {
  const record = verifyRecord(directory, repo);
  const raw = options.raw ?? (record.raw && join(repo, record.raw.path));
  if (record.raw && (!raw || sha(readFileSync(raw)) !== record.raw.sha256)) throw new Error("run npm run build: raw module provenance is stale");
  const result = compute(directory, repo, record.distribution, join(directory, record.artifact.path), raw);
  const text = readFileSync(join(directory, NOTICE_FILE), "utf8");
  validateEvidence(result.evidence, noticeSections(text), result.context);
  if (text !== result.text) throw new Error("run npm run build: third-party notices are stale");
  return result;
}

function summary(record) {
  return { package: record.distribution.package, target: record.distribution.target, profile: record.distribution.profile, features: record.distribution.features, defaultFeatures: record.distribution.defaultFeatures, sections: record.sections, toolchainCrates: record.toolchainCrates };
}

function main(args) {
  const [command, ...rest] = args;
  if (command === "verify") {
    if (rest.length !== 1) throw new Error("usage: notices verify DIRECTORY");
    verifyRecord(resolve(rest[0]), repository);
    return;
  }
  if (command === "check") {
    if (!rest[0] || (rest.length !== 1 && (rest.length !== 3 || rest[1] !== "--raw"))) throw new Error("usage: notices check DIRECTORY [--raw FILE]");
    console.log(JSON.stringify(summary(checkNotices(resolve(rest[0]), repository, { raw: rest[2] && resolve(rest[2]) }).record)));
    return;
  }
  const divider = rest.indexOf("--");
  if (!["prepare", "generate"].includes(command) || divider < 0) throw new Error("usage: notices prepare|generate [--out DIR --artifact FILE --raw FILE] -- CARGO_SELECTION");
  const distribution = selection(rest.slice(divider + 1));
  const options = {};
  for (let at = 0; at < divider; at += 2) {
    if (!["--out", "--artifact", "--raw"].includes(rest[at]) || !rest[at + 1] || Object.hasOwn(options, rest[at])) throw new Error("invalid distribution notice option");
    options[rest[at]] = resolve(rest[at + 1]);
  }
  if (command === "prepare") {
    if (divider !== 0) throw new Error("notices prepare accepts only the Cargo selection");
    const tc = toolchain(repository, distribution.target);
    process.stdout.write(prepareFlags(repository, tc, distribution).join("\x1f"));
    return;
  }
  const directory = options["--out"], artifact = options["--artifact"], raw = options["--raw"];
  if (!directory || !artifact) throw new Error("name the notice directory and the shipped artifact");
  for (const name of [NOTICE_FILE, RECORD_FILE]) rmSync(join(directory, name), { force: true });
  const result = compute(directory, repository, { ...distribution, rustflags: flags(process.env.CARGO_ENCODED_RUSTFLAGS) }, artifact, raw);
  writeFileSync(join(directory, NOTICE_FILE), result.text);
  writeFileSync(join(directory, RECORD_FILE), JSON.stringify(result.record, null, 2) + "\n");
  const publicRecord = summary(result.record);
  console.log(JSON.stringify(publicRecord));
  const fence = String.fromCharCode(96).repeat(3);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, "\n### Rust distribution notices\n\n" + fence + "json\n" + JSON.stringify(publicRecord, null, 2) + "\n" + fence + "\n");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main(process.argv.slice(2));
