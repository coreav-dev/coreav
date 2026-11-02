import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { verifyRecord } from "../../../scripts/notices/record.mjs";

const directory = fileURLToPath(new URL("..", import.meta.url));
const repo = resolve(directory, "../..");
for (const name of ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE"]) {
  let actual;
  try { actual = readFileSync(resolve(directory, name)); }
  catch { throw new Error("run scripts/build.sh first: " + name + " is missing"); }
  if (!actual.equals(readFileSync(resolve(repo, name)))) throw new Error("run scripts/build.sh first: " + name + " is stale");
}
if (readFileSync(resolve(directory, "bin/coreav")).subarray(0, 2).toString() !== "#!") throw new Error("bin/coreav is not the launcher; restore it before packing");
verifyRecord(directory, repo);
