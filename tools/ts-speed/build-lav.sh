#!/usr/bin/env bash
# Build lav_ts (libavformat's side of the packet-listing speed bar,
# docs/design/mpegts.md, "Speed bars, round 2") against the pinned LGPL
# shared build of the oracle FFmpeg (LAV_ARCHIVE in scripts/oracle-ffmpeg.env).
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
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../scripts/oracle-ffmpeg.env
source "$HERE/../../scripts/oracle-ffmpeg.env"
NAME=$LAV_ARCHIVE
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
  echo 'The pinned build is for Linux x86_64 only.' >&2
  exit 1
}
mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
curl --fail --location --retry 5 --retry-delay 10 --connect-timeout 20 --max-time 600 -o "$WORK/a.tar.xz" "${URL%/*}/$NAME.tar.xz"
echo "$LAV_SHA256  $WORK/a.tar.xz" | sha256sum -c -
tar -xJf "$WORK/a.tar.xz" -C "$WORK" "$NAME/include" "$NAME/lib" "$NAME/LICENSE.txt" "$NAME/bin/ffprobe"
rm -rf "$DEST/lib" "$DEST/include" "$DEST/bin"
mkdir "$DEST/bin"
mv "$WORK/$NAME/include" "$WORK/$NAME/lib" "$WORK/$NAME/LICENSE.txt" "$DEST/"
mv "$WORK/$NAME/bin/ffprobe" "$DEST/bin/"
rm -rf "$DEST/lib/pkgconfig"
gcc -O2 -Wall -Werror -DPINNED="\"$BUILD\"" -o "$DEST/lav_ts" "$HERE/lav_ts.c" -I"$DEST/include" -L"$DEST/lib" \
  -lavformat -lavcodec -lavutil -Wl,-rpath-link,"$DEST/lib" -Wl,--disable-new-dtags,-rpath,'$ORIGIN/lib'
rm -rf "$DEST/include"
"$DEST/lav_ts" --identity
