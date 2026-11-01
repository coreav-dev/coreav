#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(cd "$here/../.." && pwd)"
out="$here/wasm"
. "$root/scripts/rust-wasm.env"
export RUSTUP_TOOLCHAIN="$RUST_WASM_TOOLCHAIN"
have_commit="$(rustc -vV 2>/dev/null | sed -n 's/^commit-hash: //p' || true)"
if [ "$have_commit" != "$RUST_WASM_COMMIT" ]; then
  echo "error: install Rust $RUST_WASM_TOOLCHAIN: rustup toolchain install $RUST_WASM_TOOLCHAIN --profile minimal --target wasm32-unknown-unknown --component rust-src" >&2
  exit 1
fi
sysroot="$(rustc --print sysroot)"
if [ ! -f "$sysroot/lib/rustlib/src/rust/library/Cargo.lock" ]; then
  echo "error: rust-src for $RUST_WASM_TOOLCHAIN is missing: rustup component add rust-src --toolchain $RUST_WASM_TOOLCHAIN" >&2
  exit 1
fi
pin="$(sed -n 's/^wasm-bindgen = "=\(.*\)"$/\1/p' "$root/crates/coreav-wasm/Cargo.toml")"
if [ -z "$pin" ]; then
  echo "error: no wasm-bindgen pin in crates/coreav-wasm/Cargo.toml" >&2
  exit 1
fi
want="wasm-bindgen $pin"
have="$(wasm-bindgen --version 2>/dev/null || true)"
if [ "$have" != "$want" ]; then
  echo "error: need $want (cargo install wasm-bindgen-cli --version $pin --locked), found '${have:-none}'" >&2
  exit 1
fi
if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
  echo "error: CARGO_ENCODED_RUSTFLAGS is set and would drop the SIMD flag; unset it" >&2
  exit 1
fi
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
flags=()
IFS=$' \t\n' read -r -d '' -a flags < <(printf '%s\0' "${RUSTFLAGS:-}")
flags+=("-C" "target-feature=+simd128" "--remap-path-prefix=$cargo_home/registry/src=/cargo/registry" "--remap-path-prefix=$cargo_home/git/checkouts=/cargo/git" "--remap-path-prefix=$root=/coreav")
flags+=("--remap-path-prefix=$sysroot/lib/rustlib/src/rust/library=/rustc/$RUST_WASM_COMMIT/library" "--remap-path-prefix=$sysroot/lib/rustlib/src/rust/library/vendor=/rust/deps")
printf -v CARGO_ENCODED_RUSTFLAGS '%s\x1f' "${flags[@]}"
export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS%$'\x1f'}"
unset RUSTFLAGS
if ! cargo metadata --locked --offline --format-version=1 --filter-platform wasm32-unknown-unknown --manifest-path "$root/Cargo.toml" >/dev/null 2>&1; then
  cargo fetch --locked --manifest-path "$root/Cargo.toml"
fi
cargo build --locked --manifest-path "$root/Cargo.toml" -p coreav-wasm \
  --profile edge --target wasm32-unknown-unknown
rm -rf "$out"
wasm-bindgen --target web --remove-name-section --remove-producers-section \
  --out-dir "$out" --out-name coreav \
  "$root/target/wasm32-unknown-unknown/edge/coreav_wasm.wasm"
opt="${WASM_OPT:-$(command -v wasm-opt || true)}"
opt_version=""
simd=""
if [ -n "$opt" ]; then
  "$opt" -O3 --enable-bulk-memory --enable-nontrapping-float-to-int \
    --enable-sign-ext --enable-mutable-globals --enable-reference-types \
    --enable-multivalue --enable-exception-handling --enable-simd \
    "$out/coreav_bg.wasm" -o "$out/coreav_bg.opt.wasm"
  mv "$out/coreav_bg.opt.wasm" "$out/coreav_bg.wasm"
  opt_version="$("$opt" --version)"
  echo "wasm-opt: $opt_version"
else
  echo "wasm-opt: not found; the module is not optimized by binaryen" >&2
fi
dis="$(dirname "${opt:-/nonexistent/wasm-opt}")/wasm-dis"
if [ -n "$opt" ] && [ -x "$dis" ]; then
  simd=$("$dis" "$out/coreav_bg.wasm" | grep -c 'v128\.' || true)
  if [ "$simd" -eq 0 ]; then
    echo "error: the module holds no SIMD instructions" >&2
    exit 1
  fi
  echo "SIMD instructions: $simd"
elif ! grep -qa 'simd128' "$root/target/wasm32-unknown-unknown/edge/coreav_wasm.wasm"; then
  echo "error: the module was not compiled with simd128" >&2
  exit 1
else
  echo "SIMD: simd128 among the compiled module's target features (no wasm-dis to count instructions)"
fi
node "$here/scripts/fingerprint.mjs" "$here"
WASM_OPT_VERSION="$opt_version" WASM_SIMD_COUNT="$simd" BINDGEN_VERSION="$pin" node --input-type=module - "$here" "$root" <<'JS'
import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
const directory = process.argv[2];
const bytes = readFileSync(join(directory, "wasm/coreav_bg.wasm"));
const raw = readFileSync(join(process.argv[3], "target/wasm32-unknown-unknown/edge/coreav_wasm.wasm"));
const rustflags = [];
const encoded = process.env.CARGO_ENCODED_RUSTFLAGS.split("\x1f");
for (let index = 0; index < encoded.length; index += 1) {
  if (encoded[index] === "--remap-path-prefix") { index += 1; continue; }
  if (!encoded[index].startsWith("--remap-path-prefix=")) rustflags.push(encoded[index]);
}
writeFileSync(join(directory, "wasm/provenance.json"), JSON.stringify({
  wasmSha256: createHash("sha256").update(bytes).digest("hex"),
  rawWasmSha256: createHash("sha256").update(raw).digest("hex"),
  rustflags,
  wasmBindgen: process.env.BINDGEN_VERSION,
  wasmOpt: process.env.WASM_OPT_VERSION || null,
  rustc: execFileSync("rustc", ["-V"], { encoding: "utf8" }).trim(),
  profile: "edge", target: "wasm32-unknown-unknown",
  simdInstructions: process.env.WASM_SIMD_COUNT ? Number(process.env.WASM_SIMD_COUNT) : null
}, null, 2) + "\n");
JS
node "$here/scripts/legal.mjs" "$here"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/NOTICE" "$here/"
cd "$out"
shasum -a 256 coreav_bg.wasm coreav.js
echo "coreav_bg.wasm: $(wc -c < coreav_bg.wasm) bytes, gzip -9 $(gzip -9 -n -c coreav_bg.wasm | wc -c) bytes"
