import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { SIZE_BAR, packageIdentity, validateFeatureClosure, validateManifest } from "../../wasm/scripts/check-package.mjs";
import { rewritePackage } from "../../wasm/scripts/check-types.mjs";
import { EXTRA, profile } from "../scripts/profile.mjs";
import { verifyCopies } from "../scripts/sync-sources.mjs";

test("shared sources match the single owner and the H.264 engine is exclusive", () => {
  verifyCopies();
  const source = readFileSync(fileURLToPath(new URL("../../wasm/src/engine.js", import.meta.url)), "utf8");
  const generated = readFileSync(fileURLToPath(new URL("../src/engine.js", import.meta.url)), "utf8");
  assert.equal(source.includes("exclusive"), false);
  assert.match(generated, /exclusive: true/);
  assert.match(generated, /diagnostics/);
  assert.equal(readFileSync(fileURLToPath(new URL("../src/index.js", import.meta.url)), "utf8").includes("diagnostics"), false);
  assert.equal(rewritePackage('import { probe } from "@coreav/wasm";\nimport { r2Source } from "@coreav/wasm/workers";', "@coreav/wasm-h264"), 'import { probe } from "@coreav/wasm-h264";\nimport { r2Source } from "@coreav/wasm-h264/workers";');
  const root = readFileSync(fileURLToPath(new URL("../../wasm/src/index.js", import.meta.url)), "utf8");
  assert.equal(root.includes("thumbnail"), false);
  assert.equal(root.includes("@coreav/wasm-h264"), false);
});

test("the packed manifest stays private and uses the approved script names", () => {
  const manifest = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
  validateManifest(manifest, new Set(["MIT", "Apache-2.0"]), "@coreav/wasm-h264");
  assert.equal(manifest.dependencies, undefined);
  assert.equal(JSON.stringify(manifest).includes("@coreav/wasm\""), false);
});

test("the H.264 package keeps the same gzip bar and a separate feature closure", () => {
  assert.equal(profile.sizeBar, SIZE_BAR);
  assert.equal(SIZE_BAR, 1_048_576);
  assert.throws(() => packageIdentity({ sizeBar: SIZE_BAR + 1 }), /cannot be relaxed/);
  assert.doesNotThrow(() => packageIdentity({ sizeBar: SIZE_BAR }));
  const identity = packageIdentity(profile);
  assert.equal(identity.name, "@coreav/wasm-h264");
  assert.equal(identity.media, false);
  assert.equal(identity.types, true);
  assert.throws(() => packageIdentity({ types: false }), /type validation cannot be skipped/);
  for (const file of EXTRA) assert.ok(identity.packed.includes(file), file);
  assert.equal(identity.packed.includes("package/scripts/build.sh"), false);
  validateFeatureClosure({
    distribution: { package: "coreav-wasm", features: ["native-h264"], defaultFeatures: true, profile: "edge", target: "wasm32-unknown-unknown" },
    raw: { path: "target/wasm-h264/wasm32-unknown-unknown/edge/coreav_wasm.wasm" },
  }, ["native-h264"], "target/wasm-h264/");
  assert.throws(() => validateFeatureClosure({
    distribution: { package: "coreav-wasm", features: [], defaultFeatures: true, profile: "edge", target: "wasm32-unknown-unknown" },
    raw: { path: "target/wasm32-unknown-unknown/edge/coreav_wasm.wasm" },
  }, ["native-h264"], "target/wasm-h264/"));
});

test("the shared build accepts only the closed default or h264 selection", () => {
  const script = fileURLToPath(new URL("../../wasm/scripts/build.sh", import.meta.url));
  const text = readFileSync(script, "utf8");
  assert.match(text, /--features native-h264/);
  assert.match(text, /target\/wasm-h264/);
  assert.doesNotMatch(text, /coreav-cli/);
  const refused = spawnSync("bash", [script, "vp9"], { encoding: "utf8" });
  assert.notEqual(refused.status, 0);
  assert.match(refused.stderr, /default or h264/);
  const extra = spawnSync("bash", [script, "h264", "extra"], { encoding: "utf8" });
  assert.notEqual(extra.status, 0);
  assert.match(extra.stderr, /usage/);
});
