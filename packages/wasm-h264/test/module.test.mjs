import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

test("module tests require the built H.264 artifact", () => {
  assert.equal(existsSync(fileURLToPath(new URL("../wasm/coreav_bg.wasm", import.meta.url))), true, "run npm run build");
});
