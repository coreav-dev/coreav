#!/usr/bin/env bash
# Build lav_ts (libavformat's side of the packet-listing speed bar,
# docs/design/mpegts.md, "Speed bars, round 2") against the pinned LGPL
# shared build of FFmpeg 9.0.2.
#
#   tools/ts-speed/build-lav.sh DEST
#
# Downloads the archive, checks its SHA-256, keeps only its libraries, its
# license and its ffprobe (DEST/bin/ffprobe, for the command arms of
# scripts/ts_speed.py), and writes DEST/lav_ts, which finds its libraries in
# DEST/lib (an rpath relative to the binary, as the build's ffprobe has, so
# DEST can be copied to another host as it is). The headers are used for
# the build and removed. Linux x86_64 only.
set -euo pipefail
DEST="${1:?usage: build-lav.sh DEST}"
NAME=ffmpeg-n9.0.2-17-g2a571b6068-linux64-lgpl-shared-9.0
URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-30-13-08/$NAME.tar.xz"
SHA256=7a1ba2af36e5beeb98151e766a903b1d115f2206a86571f17f873cd7292c424f
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
  echo 'The pinned build is for Linux x86_64 only.' >&2
  exit 1
}
mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
curl --fail --location --retry 5 --retry-delay 10 --connect-timeout 20 --max-time 600 -o "$WORK/a.tar.xz" "$URL"
echo "$SHA256  $WORK/a.tar.xz" | sha256sum -c -
tar -xJf "$WORK/a.tar.xz" -C "$WORK" "$NAME/include" "$NAME/lib" "$NAME/LICENSE.txt" "$NAME/bin/ffprobe"
rm -rf "$DEST/lib" "$DEST/include" "$DEST/bin"
mkdir "$DEST/bin"
mv "$WORK/$NAME/include" "$WORK/$NAME/lib" "$WORK/$NAME/LICENSE.txt" "$DEST/"
mv "$WORK/$NAME/bin/ffprobe" "$DEST/bin/"
rm -rf "$DEST/lib/pkgconfig"
gcc -O2 -Wall -Werror -o "$DEST/lav_ts" "$HERE/lav_ts.c" -I"$DEST/include" -L"$DEST/lib" \
  -lavformat -lavcodec -lavutil -Wl,-rpath-link,"$DEST/lib" -Wl,--disable-new-dtags,-rpath,'$ORIGIN/lib'
rm -rf "$DEST/include"
"$DEST/lav_ts" --identity
