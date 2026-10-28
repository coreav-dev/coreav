#!/usr/bin/env bash
# Builds one native package, @coreav/cli-PLATFORM, into npm/PLATFORM/
# (docs/design/npm-cli.md, "Native binaries"):
#   scripts/platform.sh PLATFORM [RUST_TARGET]
# PLATFORM is `${process.platform}-${process.arch}` (linux-x64,
# linux-arm64, darwin-arm64, darwin-x64, win32-x64); RUST_TARGET defaults
# to that platform's target. The binary is `coreav` built with the `dist`
# profile, stripped; Linux binaries are static (musl), Windows ones carry the C
# runtime (crt-static). Prints the binary's SHA-256.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(cd "$here/../.." && pwd)"
platform="$1"
case "$platform" in
  linux-x64) default=x86_64-unknown-linux-musl ;;
  linux-arm64) default=aarch64-unknown-linux-musl ;;
  darwin-arm64) default=aarch64-apple-darwin ;;
  darwin-x64) default=x86_64-apple-darwin ;;
  win32-x64) default=x86_64-pc-windows-msvc ;;
  *) echo "error: unknown platform $platform" >&2; exit 2 ;;
esac
target="${2:-$default}"
exe=coreav
if [ "${platform%%-*}" = win32 ]; then
  exe=coreav.exe
  export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
fi
# Linux: static musl binaries. The crates that compile C (ring, for HTTPS)
# use zig as a musl C compiler for both architectures (scripts/zig.sh;
# ZIG names the command that runs zig).
case "$target" in
  *-linux-musl)
    triple="${target%%-*}-linux-musl"
    var_lower="$(echo "$target" | tr '-' '_')"
    export ZIG_TARGET="$triple"
    export "CC_${var_lower}=$here/scripts/zig.sh cc"
    export "AR_${var_lower}=$here/scripts/zig.sh ar"
    ;;
esac
# Cross-built Linux arm64 binaries link with the toolchain's own lld,
# against the musl libc and start files that the target ships, so no cross
# C toolchain is needed.
if [ "$target" = aarch64-unknown-linux-musl ] && [ "$(uname -m)" != aarch64 ]; then
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
  # Added to RUSTFLAGS, which, when set, cargo uses instead of the
  # target's own flags.
  export RUSTFLAGS="${RUSTFLAGS:-} -C linker-flavor=ld.lld"
fi
# The `dist` profile, stripped for the package.
CARGO_PROFILE_DIST_STRIP=symbols \
  cargo build --locked --manifest-path "$root/Cargo.toml" -p coreav-cli --profile dist --target "$target"
out="$here/npm/$platform"
rm -rf "$out"
mkdir -p "$out/bin"
cp "$root/target/$target/dist/$exe" "$out/bin/$exe"
chmod 755 "$out/bin/$exe"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$out/"
node "$here/scripts/platform-package.js" "$platform" "$out"
if command -v sha256sum > /dev/null; then sha256sum "$out/bin/$exe"; else shasum -a 256 "$out/bin/$exe"; fi
