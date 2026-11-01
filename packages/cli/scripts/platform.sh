#!/usr/bin/env bash
# Builds one native package, @coreav/cli-PLATFORM, into npm/PLATFORM/
# (docs/design/npm-cli.md, "Native binaries"), or the binary of the
# release's full build into full/PLATFORM/:
#   scripts/platform.sh PLATFORM [RUST_TARGET [BUILD]]
# PLATFORM is `${process.platform}-${process.arch}` (linux-x64,
# linux-arm64, darwin-arm64, darwin-x64, win32-x64); RUST_TARGET defaults
# to that platform's target. BUILD is `default` (the npm package and the
# archives coreav-VERSION-TARGET) or `full` (the archives
# coreav-full-VERSION-TARGET, never an npm package; docs/design/release.md,
# "Two builds"). The binary is `coreav` built with the `dist` profile,
# stripped; Linux binaries are static (musl), Windows ones carry the C
# runtime (crt-static). dav1d, libvpx and libopus are linked statically,
# built by scripts/codecs.sh into COREAV_CODECS (default
# target/codecs/RUST_TARGET) unless it holds them already, and the binary
# is checked to load none of them, or anything else outside the system, at
# run time. Prints the binary's SHA-256. The release archives take the same
# binary (.github/workflows/release.yml).
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
build="${3:-default}"
. "$root/scripts/rust-dist.env"
export RUSTUP_TOOLCHAIN="$RUST_DIST_TOOLCHAIN"
case "$build-$platform" in
  # Windows has no sandbox for the decoder worker or the encoder program
  # (docs/design/release.md, "Two builds"): the default features, and AAC
  # in the full build.
  default-win32-*) features=cli,net ;;
  full-win32-*) features=cli,net,aac ;;
  default-*) features=release-default ;;
  full-*) features=release-full ;;
  *) echo "error: unknown build $build" >&2; exit 2 ;;
esac
codecs=
if [ "${platform%%-*}" != win32 ]; then
  codecs="${COREAV_CODECS:-$root/target/codecs/$target}"
  if [ ! -f "$codecs/lib/pkgconfig/vpx.pc" ] || [ ! -f "$codecs/lib/pkgconfig/dav1d.pc" ] ||
    [ ! -f "$codecs/lib/pkgconfig/opus.pc" ]; then
    "$here/scripts/codecs.sh" "$target" "$codecs"
  fi
  # The musl targets differ from the build machine's, which pkg-config
  # counts as cross-compiling. opusic-sys reads no pkg-config file: it
  # links the libopus.a of OPUS_LIB_DIR, statically.
  export PKG_CONFIG_PATH="$codecs/lib/pkgconfig" PKG_CONFIG_ALLOW_CROSS=1
  export OPUS_LIB_DIR="$codecs/lib" OPUS_LIB_STATIC=true
fi
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
selection=(-p coreav-cli --profile dist --target "$target" --features "$features")
CARGO_ENCODED_RUSTFLAGS="$(node "$root/scripts/notices/main.mjs" prepare -- "${selection[@]}")"
export CARGO_ENCODED_RUSTFLAGS
unset RUSTFLAGS
CARGO_PROFILE_DIST_STRIP=symbols cargo build --locked --manifest-path "$root/Cargo.toml" "${selection[@]}"
out="$here/npm/$platform"
[ "$build" = full ] && out="$here/full/$platform"
rm -rf "$out"
mkdir -p "$out/bin"
cp "${CARGO_TARGET_DIR:-$root/target}/$target/dist/$exe" "$out/bin/$exe"
chmod 755 "$out/bin/$exe"
# The C libraries are in the binary, not loaded from the machine: a Linux
# binary needs no shared library at all, and a macOS one only the system's.
loaded=
case "$target" in
  *-linux-musl)
    dynamic="$(readelf -d "$out/bin/$exe")"
    loaded="$(grep '(NEEDED)' <<< "$dynamic" || true)"
    ;;
  *-apple-darwin)
    # A path with a . or .. component is not taken as the system's.
    dynamic="$(otool -L "$out/bin/$exe" | tail -n +2)"
    loaded="$( { grep -v -E '^[[:space:]]+(/usr/lib/|/System/Library/)' <<< "$dynamic"
      grep -E '/\.\.?/' <<< "$dynamic"; } || true)"
    ;;
esac
if [ -n "$loaded" ]; then
  echo "$loaded" >&2
  echo "error: $out/bin/$exe loads the libraries above at run time; link them statically" >&2
  exit 1
fi
# NOTICE with every binary (docs/release/notices.md), and the licence
# files of the C libraries it links.
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/NOTICE" "$out/"
[ -n "$codecs" ] && cp -R "$codecs/licenses" "$out/licenses"
node "$root/scripts/notices/main.mjs" generate --out "$out" --artifact "$out/bin/$exe" -- "${selection[@]}"
[ "$build" = default ] && node "$here/scripts/platform-package.js" "$platform" "$out"
if command -v sha256sum > /dev/null; then sha256sum "$out/bin/$exe"; else shasum -a 256 "$out/bin/$exe"; fi
