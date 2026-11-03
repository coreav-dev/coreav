import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { verifyRecord } from "../../../scripts/notices/record.mjs";
import { verifyPackage } from "../../wasm/scripts/prepack.mjs";
import { verifyCopies } from "./sync-sources.mjs";

const directory = fileURLToPath(new URL("..", import.meta.url));
const repo = resolve(directory, "../..");
verifyCopies();
verifyPackage(directory, new Map(["LICENSE-MIT", "LICENSE-APACHE", "NOTICE"].map((name) => [name, readFileSync(resolve(repo, name))])));
verifyRecord(directory, repo);
