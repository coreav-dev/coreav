#!/usr/bin/env bash
# Builds the C decoders of the release's default build, dav1d (AV1) and
# libvpx (VP9, VP8), as static libraries for one target
# (docs/design/release.md, "Two builds"):
#   scripts/codecs.sh RUST_TARGET PREFIX
# Each library comes from its release tarball, checked against the SHA-256
# below, and is built with the target's C compiler: zig for the musl
# targets (scripts/zig.sh, as for ring), the system's clang on macOS. Only
# the static libraries, their headers and pkg-config files are installed
# into PREFIX, so the crates (coreav-backend-dav1d, coreav-backend-libvpx)
# find them through PKG_CONFIG_PATH=PREFIX/lib/pkgconfig and link them into
# the binary. Their licence files go to PREFIX/licenses, which every archive
# and npm package of a build that links them ships. Needs curl, meson,
# ninja, make, and nasm on x86-64.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
target="$1"
prefix="$2"
DAV1D_VERSION=1.5.3
DAV1D_SHA256=732010aa5ef461fa93355ed2c6c5fedb48ddc4b74e697eaabe8907eaeb943011
# libvpx 1.14: its decoder ABI is the one coreav-backend-libvpx declares
# (docs/design/decode.md, "libvpx").
LIBVPX_VERSION=1.14.1
LIBVPX_SHA256=901747254d80a7937c933d03bd7c5d41e8e6c883e0665fadcb172542167c7977
case "$target" in
  x86_64-unknown-linux-musl) zig_target=x86_64-linux-musl vpx_target=x86_64-linux-gcc ;;
  aarch64-unknown-linux-musl) zig_target=aarch64-linux-musl vpx_target=arm64-linux-gcc ;;
  # The deployment targets Rust builds for: macOS 11 on arm64, 10.12 on x86-64.
  aarch64-apple-darwin) vpx_target=arm64-darwin20-gcc; export MACOSX_DEPLOYMENT_TARGET=11.0 ;;
  x86_64-apple-darwin) vpx_target=x86_64-darwin16-gcc; export MACOSX_DEPLOYMENT_TARGET=10.12 ;;
  *) echo "error: no C decoders for $target" >&2; exit 2 ;;
esac
if [ -n "${zig_target:-}" ]; then
  export ZIG_TARGET="$zig_target"
  export CC="$here/scripts/zig.sh cc" AR="$here/scripts/zig.sh ar"
  export LD="$CC"
fi
work="$(mktemp -d "${TMPDIR:-/tmp}/coreav-codecs.XXXXXX")"
trap 'rm -rf "$work"' EXIT
fetch() {
  curl -sSfL --retry 5 --retry-connrefused -o "$work/$3" "$1"
  echo "$2  $work/$3" | shasum -a 256 -c - > /dev/null
  mkdir -p "$work/src/$3"
  tar -xf "$work/$3" -C "$work/src/$3" --strip-components 1
}
fetch "https://downloads.videolan.org/pub/videolan/dav1d/$DAV1D_VERSION/dav1d-$DAV1D_VERSION.tar.xz" \
  "$DAV1D_SHA256" dav1d.tar.xz
fetch "https://github.com/webmproject/libvpx/archive/refs/tags/v$LIBVPX_VERSION.tar.gz" \
  "$LIBVPX_SHA256" libvpx.tar.gz
rm -rf "$prefix"
mkdir -p "$prefix/licenses"
(
  cd "$work/src/dav1d.tar.xz"
  meson setup build --buildtype release --default-library static --prefix "$prefix" --libdir lib \
    -Denable_tools=false -Denable_tests=false -Denable_examples=false -Denable_docs=false > /dev/null
  ninja -C build install > /dev/null
  cp COPYING "$prefix/licenses/dav1d-COPYING"
  # The Alliance for Open Media patent licence, which asks to be reproduced
  # with binary distributions.
  cp doc/PATENTS "$prefix/licenses/dav1d-PATENTS"
  # The ISC notice of dav1d's x86 assembly layer, the comment block that
  # opens the file; the file has no separate licence file.
  awk '!/^;\*/ { exit } { print }' src/ext/x86/x86inc.asm > "$prefix/licenses/dav1d-x86inc-LICENSE"
)
(
  cd "$work/src/libvpx.tar.gz"
  as=--as=auto
  case "$vpx_target" in x86_64-*) as=--as=nasm ;; esac
  ./configure --target="$vpx_target" --prefix="$prefix" --libdir="$prefix/lib" "$as" \
    --enable-static --disable-shared --enable-pic --enable-vp9-highbitdepth \
    --disable-vp8-encoder --disable-vp9-encoder --disable-libyuv --disable-webm-io \
    --disable-examples --disable-tools --disable-docs --disable-unit-tests > /dev/null
  make -j"$(getconf _NPROCESSORS_ONLN)" install > /dev/null
  cp LICENSE "$prefix/licenses/libvpx-LICENSE"
  cp PATENTS "$prefix/licenses/libvpx-PATENTS"
  cp AUTHORS "$prefix/licenses/libvpx-AUTHORS"
  cp third_party/x86inc/LICENSE "$prefix/licenses/libvpx-x86inc-LICENSE"
)
find "$prefix/lib" \( -name '*.so*' -o -name '*.dylib' \) -delete
# musl's libc.a holds libm and libpthread, and Rust links it; a -lm or
# -lpthread would find the build machine's glibc copies instead.
if [ -n "${zig_target:-}" ]; then
  sed -i.orig -e 's/ -lm\b//g' -e 's/ -lpthread\b//g' "$prefix"/lib/pkgconfig/*.pc
  rm "$prefix"/lib/pkgconfig/*.orig
fi
ls "$prefix/lib" "$prefix/licenses"
