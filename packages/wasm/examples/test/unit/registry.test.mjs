import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { registeredBars } from "../registry.mjs";

const note = readFileSync(new URL("../../../../../docs/design/wasm-memory-bars.md", import.meta.url), "utf8");
const wrap = (raw) => `<!-- wasm-memory-bars:begin -->\n\`\`\`json\n${raw}\n\`\`\`\n<!-- wasm-memory-bars:end -->`;
const registry = (gates = '"one":65536', extra = "") => wrap(`{"signal":"wasm_linear_bytes","predicate":"value <= bar","gates":{${gates}}${extra}}`);

test("the committed registry supplies all sixteen page-aligned bars", () => {
  const bars = registeredBars(note);
  assert.equal(bars.size, 16);
  for (const bar of bars.values()) assert.equal(bar % 65536, 0);
});

test("raw and escaped duplicate names fail before whole-document parsing", () => {
  for (const raw of [
    registry('"one":65536,"one":65536'), registry('"one":65536,"\\u006fne":65536'),
    registry('"one":65536', ',"gates":{"two":65536}'),
    registry('"one":65536', ',"extra":{"key":1,"k\\u0065y":2}')
  ]) assert.throws(() => registeredBars(raw), /duplicate property/);
});

test("key-looking content inside values does not create duplicate names", () => {
  assert.equal(registeredBars(registry('"one":65536')).get("one"), 65536);
  for (const text of ["a:b", '{"key":1,"key":2}', "a\\\"b"]) {
    assert.throws(() => registeredBars(registry('"one":65536', `,"extra":${JSON.stringify(text)}`)), /schema/);
  }
});

test("markers fences syntax and registry schema fail closed", () => {
  for (const raw of [note.replace("<!-- wasm-memory-bars:end -->", ""), note + "<!-- wasm-memory-bars:begin -->",
    note.replace("```json", "```javascript"), note.replace('"predicate": "value <= bar"', '"predicate": "value < bar"'),
    note.replace('"wasm_linear_bytes"', '"heap"'), registry('"one":65536', ',"extra":true'),
    registry('"one":65536,'), registry('"one":01'), registry('"one":65536') + "<!-- wasm-memory-bars:end -->"
  ]) assert.throws(() => registeredBars(raw));
  for (const value of ["0", "-1", "1.5", "1e400", "65537", "null", "true", '"65536"', "[]", "{}"])
    assert.throws(() => registeredBars(registry(`"one":${value}`)), /invalid/);
});
