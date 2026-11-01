import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

function permitted(text, allowed) {
  if (typeof text !== "string") return false;
  const tokens = text.match(/\(|\)|[A-Za-z0-9.+-]+/g) ?? [];
  if (tokens.join("") !== text.replace(/\s/g, "")) return false;
  let at = 0;
  function atom() {
    if (tokens[at] === "(") {
      at += 1;
      const value = either();
      if (tokens[at++] !== ")") throw new Error("unbalanced license expression");
      return value;
    }
    let name = tokens[at++];
    if (!name || ["AND", "OR", ")", "WITH"].includes(name)) throw new Error("invalid license expression");
    if (tokens[at] === "WITH") { at += 1; name += ` WITH ${tokens[at++]}`; }
    return allowed.has(name);
  }
  function both() {
    let value = atom();
    while (tokens[at] === "AND") { at += 1; const right = atom(); value = value && right; }
    return value;
  }
  function either() {
    let value = both();
    while (tokens[at] === "OR") { at += 1; const right = both(); value = value || right; }
    return value;
  }
  try { const value = either(); return at === tokens.length && value; }
  catch { return false; }
}

test("the fresh example tool lock follows the repository license policy", () => {
  const policy = readFileSync(new URL("../../../../../deny.toml", import.meta.url), "utf8");
  const list = /^\[licenses\][\s\S]*?\ballow\s*=\s*\[([\s\S]*?)\]/m.exec(policy);
  assert.ok(list, "read the licenses.allow array from deny.toml");
  const allowed = new Set([...list[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]));
  assert.equal(permitted("MIT OR GPL-3.0", allowed), true);
  for (const expression of ["MIT AND GPL-3.0", "MIT?", "MIT OR", "", "(MIT"]) assert.equal(permitted(expression, allowed), false);
  assert.equal(permitted("(MIT OR Apache-2.0) AND BSD-3-Clause", allowed), true);
  assert.equal(permitted("Apache-2.0 WITH LLVM-exception", allowed), true);
  const lock = JSON.parse(readFileSync(new URL("../../package-lock.json", import.meta.url)));
  assert.equal(lock.lockfileVersion, 3);
  let checked = 0;
  for (const [path, entry] of Object.entries(lock.packages)) {
    if (!path || entry.link) continue;
    assert.doesNotMatch(path, /(?:^|\/)(?:wrangler|miniflare|sharp|@img)(?:\/|$)/);
    assert.ok(permitted(entry.license, allowed), `unapproved or missing license for ${path}`);
    checked += 1;
  }
  assert.ok(checked > 0);
});
