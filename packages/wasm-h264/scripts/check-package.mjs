import { main } from "../../wasm/scripts/check-package.mjs";
import { profile } from "./profile.mjs";

await main(process.argv.slice(2), profile);
