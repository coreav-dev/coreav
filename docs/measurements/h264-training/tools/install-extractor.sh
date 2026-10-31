#!/usr/bin/env bash
set -euo pipefail
DEST="${RUNNER_TEMP:?}/coreav-training-extractor"
VERSION=7.0.2
SHA256=abda8d77ce8309141f83ab8edf0596834087c52467f6badf376a6a2a4c87cf67
mkdir -p "$DEST"
WORK=$(mktemp -d "$DEST/.download.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
OK=
for AREA in releases old-releases; do
  if curl --fail --location --retry 5 --retry-connrefused --retry-delay 10 \
    --retry-max-time 900 --connect-timeout 20 --max-time 600 \
    "https://johnvansickle.com/ffmpeg/$AREA/ffmpeg-$VERSION-amd64-static.tar.xz" \
    --output "$WORK/ffmpeg.tar.xz" &&
    echo "$SHA256  $WORK/ffmpeg.tar.xz" | sha256sum --check --strict; then
    OK=1
    break
  fi
done
if [[ -z "$OK" ]]; then
  echo 'No source served the historical extractor archive.' >&2
  exit 1
fi
tar -xJf "$WORK/ffmpeg.tar.xz" -C "$WORK" "ffmpeg-$VERSION-amd64-static/ffmpeg"
install -m 755 "$WORK/ffmpeg-$VERSION-amd64-static/ffmpeg" "$DEST/ffmpeg"
ACTUAL=$("$DEST/ffmpeg" -version | awk 'NR == 1 { print $3 }')
if [[ "$ACTUAL" != "$VERSION-static" ]]; then
  echo "Historical extractor version mismatch: $ACTUAL" >&2
  exit 1
fi
echo "Historical training extractor: $ACTUAL"
