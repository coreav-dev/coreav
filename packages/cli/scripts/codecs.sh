#!/usr/bin/env bash
# Builds the C libraries of the release's default build, the decoders dav1d
# (AV1) and libvpx (VP9, VP8) and the Opus encoder libopus, as static
# libraries for one target (docs/design/release.md, "Two builds"):
#   scripts/codecs.sh RUST_TARGET PREFIX [LIBRARY...]
# LIBRARY is dav1d, libvpx or opus; without one, all three are built. The
# opus-encode workflow builds only opus, for the Linux GNU target its
# coreav is built for, so that the Opus bars judge the libopus the release
# links (docs/design/transcode.md, "Opus bars"); its check reads
# OPUS_VERSION from here, the one pin of both.
# Each library comes from its release tarball, checked against the SHA-256
# below, and is built natively with the target's C compiler: musl-gcc
# (Ubuntu's musl-tools) on Linux, the system's clang on macOS. Only
# the static libraries, their headers and pkg-config files are installed
# into PREFIX, so the crates (coreav-backend-dav1d, coreav-backend-libvpx)
# find them through PKG_CONFIG_PATH=PREFIX/lib/pkgconfig and link them into
# the binary; opusic-sys (coreav-backend-libopus) takes PREFIX/lib from
# OPUS_LIB_DIR, which platform.sh sets. Their licence files go to
# PREFIX/licenses, which every archive and npm package of a build that
# links them ships. Needs curl, meson, ninja, make, musl-gcc for the musl
# targets, and nasm on x86-64.
set -euo pipefail
target="$1"
prefix="$2"
shift 2
libraries="${*:-dav1d libvpx opus}"
for library in $libraries; do
  case "$library" in
    dav1d | libvpx | opus) ;;
    *) echo "error: unknown library $library" >&2; exit 2 ;;
  esac
done
want() { [[ " $libraries " == *" $1 "* ]]; }
DAV1D_VERSION=1.5.3
DAV1D_SHA256=732010aa5ef461fa93355ed2c6c5fedb48ddc4b74e697eaabe8907eaeb943011
# libvpx 1.14: its decoder ABI is the one coreav-backend-libvpx declares
# (docs/design/decode.md, "libvpx").
LIBVPX_VERSION=1.14.1
LIBVPX_SHA256=901747254d80a7937c933d03bd7c5d41e8e6c883e0665fadcb172542167c7977
OPUS_VERSION=1.6.1
OPUS_SHA256=6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1
case "$target" in
  x86_64-unknown-linux-musl) musl=x86_64 vpx_target=x86_64-linux-gcc ;;
  aarch64-unknown-linux-musl) musl=aarch64 vpx_target=arm64-linux-gcc ;;
  # The build machine's own C compiler, for a coreav built for it.
  x86_64-unknown-linux-gnu) vpx_target=x86_64-linux-gcc ;;
  aarch64-unknown-linux-gnu) vpx_target=arm64-linux-gcc ;;
  # The deployment targets Rust builds for: macOS 11 on arm64, 10.12 on x86-64.
  aarch64-apple-darwin) vpx_target=arm64-darwin20-gcc; export MACOSX_DEPLOYMENT_TARGET=11.0 ;;
  x86_64-apple-darwin) vpx_target=x86_64-darwin16-gcc; export MACOSX_DEPLOYMENT_TARGET=10.12 ;;
  *) echo "error: no C decoders for $target" >&2; exit 2 ;;
esac
# musl-gcc compiles for the machine it runs on: no cross-compiling. (zig,
# the C compiler of ring, is not used here: meson cannot identify its
# linker.)
if [ -n "${musl:-}" ]; then
  if [ "$(uname -m)" != "$musl" ]; then
    echo "error: build $target on a $musl Linux machine" >&2
    exit 2
  fi
  export CC=musl-gcc LD=musl-gcc
fi
work="$(mktemp -d "${TMPDIR:-/tmp}/coreav-codecs.XXXXXX")"
trap 'rm -rf "$work"' EXIT
fetch() {
  curl -sSfL --retry 5 --retry-connrefused -o "$work/$3" "$1"
  if command -v sha256sum > /dev/null; then sum=(sha256sum); else sum=(shasum -a 256); fi
  echo "$2  $work/$3" | "${sum[@]}" -c - > /dev/null
  mkdir -p "$work/src/$3"
  tar -xf "$work/$3" -C "$work/src/$3" --strip-components 1
}
rm -rf "$prefix"
mkdir -p "$prefix/licenses"
if want dav1d; then (
  fetch "https://downloads.videolan.org/pub/videolan/dav1d/$DAV1D_VERSION/dav1d-$DAV1D_VERSION.tar.xz" \
    "$DAV1D_SHA256" dav1d.tar.xz
  cd "$work/src/dav1d.tar.xz"
  meson setup build --buildtype release --default-library static --prefix "$prefix" --libdir lib \
    -Denable_tools=false -Denable_tests=false -Denable_examples=false -Denable_docs=false
  ninja -C build install
  cp COPYING "$prefix/licenses/dav1d-COPYING"
  # The Alliance for Open Media patent licence, which asks to be reproduced
  # with binary distributions.
  cp doc/PATENTS "$prefix/licenses/dav1d-PATENTS"
  # The ISC notice of dav1d's x86 assembly layer, the comment block that
  # opens the file; the file has no separate licence file.
  awk '!/^;\*/ { exit } { print }' src/ext/x86/x86inc.asm > "$prefix/licenses/dav1d-x86inc-LICENSE"
) fi
if want libvpx; then (
  fetch "https://github.com/webmproject/libvpx/archive/refs/tags/v$LIBVPX_VERSION.tar.gz" \
    "$LIBVPX_SHA256" libvpx.tar.gz
  cd "$work/src/libvpx.tar.gz"
  as=--as=auto
  case "$vpx_target" in x86_64-*) as=--as=nasm ;; esac
  ./configure --target="$vpx_target" --prefix="$prefix" --libdir="$prefix/lib" "$as" \
    --enable-static --disable-shared --enable-pic --enable-vp9-highbitdepth \
    --disable-vp8-encoder --disable-vp9-encoder --disable-libyuv --disable-webm-io \
    --disable-examples --disable-tools --disable-docs --disable-unit-tests
  make -j"$(getconf _NPROCESSORS_ONLN)" install
  cp LICENSE "$prefix/licenses/libvpx-LICENSE"
  cp PATENTS "$prefix/licenses/libvpx-PATENTS"
  cp AUTHORS "$prefix/licenses/libvpx-AUTHORS"
  cp third_party/x86inc/LICENSE "$prefix/licenses/libvpx-x86inc-LICENSE"
) fi
if want opus; then (
  fetch "https://downloads.xiph.org/releases/opus/opus-$OPUS_VERSION.tar.gz" "$OPUS_SHA256" opus.tar.gz
  cd "$work/src/opus.tar.gz"
  meson setup build --buildtype release --default-library static --prefix "$prefix" --libdir lib \
    -Dextra-programs=disabled -Dtests=disabled -Ddocs=disabled
  ninja -C build install
  # The BSD-3 licence, with the royalty-free patent licences it names.
  cp COPYING "$prefix/licenses/opus-COPYING"
) fi
find "$prefix/lib" \( -name '*.so*' -o -name '*.dylib' \) -delete
# musl's libc.a holds libm, libpthread and libdl, and Rust links it; a -lm,
# -lpthread or -ldl would find the build machine's glibc copies instead.
if [ -n "${musl:-}" ]; then
  sed -i.orig -e 's/ -lm\b//g' -e 's/ -lpthread\b//g' -e 's/ -ldl\b//g' "$prefix"/lib/pkgconfig/*.pc
  rm "$prefix"/lib/pkgconfig/*.orig
fi
ls "$prefix/lib" "$prefix/licenses"
