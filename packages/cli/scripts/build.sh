#!/usr/bin/env bash
# Builds the coreav command for WASI into wasm/coreav.wasm and copies the
# license files and NOTICE next to it (docs/design/npm-cli.md, "Build"):
#   1. cargo, profile `edge`, target wasm32-wasip1;
#   2. wasm-opt -O3, when WASM_OPT names a binaryen wasm-opt or one is on
#      PATH.
# Prints the SHA-256 and sizes of the module.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(cd "$here/../.." && pwd)"
out="$here/wasm"
. "$root/scripts/rust-dist.env"
export RUSTUP_TOOLCHAIN="$RUST_DIST_TOOLCHAIN"
# Without the `net` feature: WASI has no sockets, so URLs are refused.
selection=(-p coreav-cli --no-default-features --features cli --profile edge --target wasm32-wasip1)
CARGO_ENCODED_RUSTFLAGS="$(node "$root/scripts/notices/main.mjs" prepare -- "${selection[@]}")"
export CARGO_ENCODED_RUSTFLAGS
unset RUSTFLAGS
cargo build --locked --manifest-path "$root/Cargo.toml" "${selection[@]}"
rm -rf "$out"
mkdir -p "$out"
cp "$root/target/wasm32-wasip1/edge/coreav.wasm" "$out/coreav.wasm"
opt="${WASM_OPT:-$(command -v wasm-opt || true)}"
if [ -n "$opt" ]; then
  # The features rustc's wasm32 targets enable.
  "$opt" -O3 --strip-debug --strip-producers --enable-bulk-memory \
    --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals --enable-reference-types --enable-multivalue \
    "$out/coreav.wasm" -o "$out/coreav.opt.wasm"
  mv "$out/coreav.opt.wasm" "$out/coreav.wasm"
  echo "wasm-opt: $("$opt" --version)"
else
  echo "wasm-opt: not found; the module is not optimized by binaryen" >&2
fi
# NOTICE with every build of the command (docs/release/notices.md).
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/NOTICE" "$here/"
node "$root/scripts/notices/main.mjs" generate --out "$here" --artifact "$out/coreav.wasm" \
  --raw "$root/target/wasm32-wasip1/edge/coreav.wasm" -- "${selection[@]}"
cd "$out"
if command -v sha256sum > /dev/null; then sha256sum coreav.wasm; else shasum -a 256 coreav.wasm; fi
echo "coreav.wasm: $(wc -c < coreav.wasm) bytes, gzip -9 $(gzip -9 -n -c coreav.wasm | wc -c) bytes"
