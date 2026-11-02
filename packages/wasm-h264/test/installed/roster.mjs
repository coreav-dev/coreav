import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

export const rosterBytes = readFileSync(new URL("./roster.json", import.meta.url));
export const roster = JSON.parse(rosterBytes);
export const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
export const rosterSha256 = sha(rosterBytes);
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}
export const optionsSha256 = (options) => sha(canonical(options));
export const fixtureId = (geometry, entropy, container) => `${geometry}-${entropy}-${container}`;

export function fixtureRoster() {
  const result = [];
  for (const geometry of [...roster.geometries.map(g => g.id), ...roster.variants, "4k", "4k-small-container"])
    for (const entropy of roster.entropy) for (const container of roster.containers)
      result.push({ id: fixtureId(geometry, entropy, container), geometry, entropy, container });
  return result;
}

export function gateRoster() {
  const groups = new Map([["thumbnail-startup", [{ id: "startup", operation: "startup", expectation: "startup", options: {} }]]]);
  const request = (id, fixture, options, expectation = "success") => ({ id, operation: "thumbnail", fixture, options, expectation });
  for (const container of roster.containers) for (const format of roster.formats) {
    const gate = `fresh-thumbnail-${format}-${container}`;
    groups.set(gate, roster.entropy.map(entropy => request(entropy, fixtureId("coded-crop", entropy, container), { at: "1", width: 1280, height: 720, format })));
  }
  for (const container of roster.containers) {
    const steps = [];
    for (const geometry of ["4k", "4k-small-container"]) for (const entropy of roster.entropy)
      steps.push(request(`${geometry}-${entropy}`, fixtureId(geometry, entropy, container), { at: "1", width: 320, format: "jpeg" }, "limit"));
    steps.push(request("benign-reuse", fixtureId("360p", "cavlc", container), { at: "0", format: "jpeg" }));
    groups.set(`refused-thumbnail-4k-${container}`, steps);
  }
  const warm = [];
  for (const at of ["0", "1"]) for (const geometry of roster.geometries) for (const container of roster.containers)
    for (const entropy of roster.entropy) for (const format of roster.formats) for (const size of ["default", "maximum"])
      warm.push(request(`${at}-${geometry.id}-${container}-${entropy}-${format}-${size}`, fixtureId(geometry.id, entropy, container), { at, format, ...(size === "maximum" ? { width: 1280, height: 720 } : {}) }));
  for (const variant of roster.variants) for (const container of roster.containers) for (const entropy of roster.entropy) for (const format of roster.formats)
    warm.push(request(`${variant}-${container}-${entropy}-${format}`, fixtureId(variant, entropy, container), { at: "1", format }));
  groups.set("thumbnail-warm", warm);
  groups.set("thumbnail-admission", ["source-open", "output-backpressure", "probe-first"].map(id => ({ id, operation: "admission", fixture: fixtureId("360p", "cavlc", "mp4"), options: { at: "0", format: "jpeg" }, expectation: "admission" })));
  return groups;
}

export function parityRoster() {
  const cases = [];
  for (const container of roster.containers) for (const entropy of roster.entropy) {
    const fixture = fixtureId("360p", entropy, container);
    for (const quality of [1, 91, 100]) for (const filter of roster.filters) for (const chroma of roster.chroma)
      cases.push({ id: `${container}-${entropy}-q${quality}-${filter}-${chroma}`, operation: "thumbnail", fixture, options: { at: "1", format: "jpeg", quality, filter, chroma }, expectation: "success" });
    for (const dimensions of [{ height: 180 }, { width: 400 }, { width: 400, height: 200 }])
      cases.push({ id: `${container}-${entropy}-${Object.keys(dimensions).join("-")}`, operation: "thumbnail", fixture, options: { at: "1", format: "png", ...dimensions }, expectation: "success" });
    cases.push({ id: `${container}-${entropy}-stream1`, operation: "thumbnail", fixture: fixtureId("stream-ties", entropy, container), options: { at: "1", stream: 1, format: "webp" }, expectation: "success" });
  }
  return cases;
}
