import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const directory = new URL("./upstream/runtime/", import.meta.url);
export const runtimeManifest = JSON.parse(readFileSync(new URL("manifest.json", directory), "utf8"));

export function runtimeComponents(target, commit) {
  if (commit !== runtimeManifest.rustCommit) throw new Error("C runtime inventory is stale; review scripts/notices/upstream/runtime/manifest.json for the distribution compiler");
  if (!Object.hasOwn(runtimeManifest.targets, target)) throw new Error("C runtime inventory has no distribution target " + target);
  return runtimeManifest.targets[target].map(name => runtimeManifest.components[name]);
}

export function renderRuntimes(target, commit, read = path => readFileSync(new URL(path, directory))) {
  const sections = [];
  for (const component of runtimeComponents(target, commit)) {
    sections.push(`=== ${component.label} (C runtime) ===\nLicense: ${component.license}\n`);
    for (const path of component.files) {
      const file = runtimeManifest.files[path], bytes = read(path);
      if (createHash("sha256").update(bytes).digest("hex") !== file.sha256) throw new Error("C runtime upstream licence changed: " + path);
      const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes).replace(/\r\n/g, "\n").replace(/\n*$/, "\n");
      sections.push(`--- ${path} ---\nSource: ${file.source}\n${text}`);
    }
  }
  return sections.join("\n");
}
