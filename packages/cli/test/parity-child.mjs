import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const [arm, input, output] = process.argv.slice(2);
const spec = JSON.parse(readFileSync(input));
if (spec.delay?.[arm]) await new Promise((resolve) => setTimeout(resolve, spec.delay[arm]));
if (spec.log) appendFileSync(spec.log, `${JSON.stringify({ id: spec.id, arm, output, cwd: process.cwd() })}\n`);
if (output) {
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output.replace("%03d", "000"), String(spec.id));
  if (spec.part) writeFileSync(join(process.cwd(), "left.part"), "partial");
}
process.stdout.write(`${spec.id}:${spec.mismatch && arm === "wasi" ? "different" : "same"}`);
process.stderr.write(`stderr:${spec.id}`);
process.exitCode = spec.status?.[arm] ?? spec.status ?? 0;
