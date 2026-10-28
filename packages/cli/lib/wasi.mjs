// The fallback: the coreav command built for WASI (wasm32-wasip1), run by
// Node's WASI with the arguments, standard streams and exit code passed
// through, on platforms without a native binary (docs/design/npm-cli.md).
// `lib/main.js` loads it; `node lib/wasi.mjs ARGS` runs it directly.

import { readFile } from "node:fs/promises";
import { join, parse, sep } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

// Node prints an ExperimentalWarning when WASI is first loaded. The
// command's stderr is for its own errors, which callers may parse, so that
// one warning is dropped; every other warning is printed as usual.
const emitWarning = process.emitWarning;
process.emitWarning = function (warning, ...rest) {
  const type = typeof rest[0] === "string" ? rest[0] : rest[0]?.type;
  if (type === "ExperimentalWarning" && String(warning?.message ?? warning).includes("WASI")) return;
  return emitWarning.call(this, warning, ...rest);
};
const { WASI } = await import("node:wasi");

/**
 * The guest sees the host's file system root as `/` and starts in the
 * caller's working directory (`COREAV_CWD`, read by the command on WASI).
 * On Windows the root is the current drive's (or share's): `\` in an
 * argument, or in the value of a `--name=value` argument, is read as `/`,
 * and a leading drive letter equal to the current drive is dropped. A path
 * on another drive or share cannot be reached and is a usage error.
 */
function layout() {
  const cwd = process.cwd();
  const args = process.argv.slice(2);
  if (process.platform !== "win32") return { root: "/", cwd, args };
  const root = parse(cwd).root;
  const drive = /^[A-Za-z]:/.test(root) ? root.slice(0, 2).toLowerCase() : null;
  const unreachable = [];
  const guest = (value) => {
    let p = value.replaceAll("\\", "/");
    if (/^[A-Za-z]:/.test(p)) {
      if (p.slice(0, 2).toLowerCase() !== drive) unreachable.push(value);
      p = p.slice(2);
    } else if (p.startsWith("//")) {
      unreachable.push(value);
    }
    return p;
  };
  const mapped = args.map((arg) => {
    const option = /^(--[^=]+=)(.*)$/s.exec(arg);
    return option ? option[1] + guest(option[2]) : guest(arg);
  });
  return {
    root,
    cwd: "/" + cwd.slice(root.length).split(sep).join("/"),
    args: mapped,
    unreachable,
  };
}

// Node opens pipes and sockets on stdio in non-blocking mode, and its WASI
// passes EAGAIN on to the module, which a write to a slow reader then fails
// with. The module writes through the descriptors themselves, so they are
// made blocking again, as they are for the native command.
for (const stream of [process.stdout, process.stderr]) stream._handle?.setBlocking?.(true);

const { root, cwd, args, unreachable = [] } = layout();
if (unreachable.length > 0) {
  process.stderr.write(
    `error: cannot reach ${unreachable[0]}: on Windows, coreav from npm reaches only the working directory's drive; give a path on that drive or a relative one\n`,
  );
  process.exit(2);
}
const wasm = join(fileURLToPath(new URL("..", import.meta.url)), "wasm", "coreav.wasm");
const wasi = new WASI({
  version: "preview1",
  args: ["coreav", ...args],
  env: { COREAV_CWD: cwd },
  preopens: { "/": root },
  returnOnExit: true,
});
let code;
try {
  const module = await WebAssembly.compile(await readFile(wasm));
  const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
  code = wasi.start(instance);
} catch (error) {
  // A trap or a missing module; the command itself never panics.
  process.stderr.write(`error: the coreav WebAssembly module failed: ${error?.message ?? error}\n`);
  code = 70;
}
process.exitCode = code;
