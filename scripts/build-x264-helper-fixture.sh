#!/usr/bin/env bash
set -euo pipefail
DEST="${1:?usage: build-x264-helper-fixture.sh /absolute/test-directory}"
case "$DEST" in /*) ;; *) echo 'The helper fixture destination must be absolute' >&2; exit 1 ;; esac
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/scripts/oracle-ffmpeg.env"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/coreav-helper-fixture.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$DEST"
git clone --quiet --filter=blob:none --no-checkout --no-tags https://code.videolan.org/videolan/x264.git "$WORK/x264"
git -C "$WORK/x264" -c advice.detachedHead=false checkout --quiet "$X264_COMMIT"
(
  cd "$WORK/x264"
  ./configure --prefix="$WORK/prefix" --enable-static --disable-shared --disable-lavf --disable-ffms --disable-gpac \
    --disable-lsmash --disable-swscale --bit-depth=all --chroma-format=all >/dev/null
  make -j"$(getconf _NPROCESSORS_ONLN)" >/dev/null 2>&1
  make install >/dev/null 2>&1
)
PKG_CONFIG_PATH="$WORK/prefix/lib/pkgconfig" make -C "$ROOT/tools/x264-shm-helper" OUTPUT="$DEST/coreav-x264-shm"
python3 "$ROOT/tools/x264-shm-helper/test_helper.py" --helper "$DEST/coreav-x264-shm" --x264 "$WORK/prefix/bin/x264"
