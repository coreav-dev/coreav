#!/usr/bin/env bash
# Build the pinned x264 command-line program, a test-only black box for
# `coreav transcode`'s encoder (docs/design/transcode.md, "The encoder
# program"): commit 4613ac3 (r3191), the libx264 of the pinned FFmpeg 7.0.2.
# The source is fetched into a temporary directory outside the repository,
# built without reading it (x264 is a program here), and deleted;
# only the `x264` executable is kept. Nothing of it is ever committed.
# Needs git, a C compiler, make and nasm.
set -euo pipefail
DEST="${1:?usage: install-x264.sh /absolute/bin/dir}"
COMMIT=4613ac3c15fd75cebc4b9f65b7fb95e70a3acce1
REPO=https://code.videolan.org/videolan/x264.git
mkdir -p "$DEST"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/coreav-x264.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
# The commit history without file contents, then the files of the pinned
# commit only: x264's version string counts the commits (0.164.3191).
git clone --quiet --filter=blob:none --no-checkout --no-tags "$REPO" "$WORK/x264"
git -C "$WORK/x264" -c advice.detachedHead=false checkout --quiet "$COMMIT"
# The flags of the CI image's build: every bit depth and chroma format, no
# input or output libraries (raw frames in, Annex B out), no swscale.
(
  cd "$WORK/x264"
  ./configure --enable-static --disable-lavf --disable-ffms --disable-gpac \
    --disable-lsmash --disable-swscale --bit-depth=all --chroma-format=all \
    >/dev/null
  make -j"$(getconf _NPROCESSORS_ONLN)" x264 >/dev/null 2>&1
)
install -m 755 "$WORK/x264/x264" "$DEST/x264"
"$DEST/x264" --version | head -1
