#!/usr/bin/env bash
set -euo pipefail
DEST="${RUNNER_TEMP:?}/coreav-training-extractor"
VERSION=7.0.2
SHA256=abda8d77ce8309141f83ab8edf0596834087c52467f6badf376a6a2a4c87cf67
HERE=$(dirname "${BASH_SOURCE[0]}")
mkdir -p "$DEST"
WORK=$(mktemp -d "$DEST/.download.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
ARCHIVE="$WORK/ffmpeg.tar.xz"
OK=
# The publisher can answer 200 with a short page that is not the archive
# (seen from runner addresses), which curl accepts: the hash decides, a
# mismatch is tried again, and its first bytes are shown when it is small.
for AREA in releases old-releases; do
  for ATTEMPT in 1 2 3; do
    if ! curl --fail --location --retry 5 --retry-connrefused --retry-delay 10 \
      --retry-max-time 900 --connect-timeout 20 --max-time 600 \
      "https://johnvansickle.com/ffmpeg/$AREA/ffmpeg-$VERSION-amd64-static.tar.xz" \
      --output "$ARCHIVE"; then
      break
    fi
    if echo "$SHA256  $ARCHIVE" | sha256sum --check --strict; then
      OK=1
      break 2
    fi
    if [[ $(wc -c < "$ARCHIVE") -lt 65536 ]]; then
      echo "$AREA answered with $(wc -c < "$ARCHIVE") bytes that are not the archive:" >&2
      head -c 300 "$ARCHIVE" | tr -c '[:print:]\n' '?' >&2
      echo >&2
    fi
    sleep 20
  done
done
# The bench bucket keeps the same archive, for a host with R2 credentials
# (no workflow passes them).
if [[ -z "$OK" ]] && python3 -I "$HERE/../../../../scripts/oracle_from_r2.py" "$ARCHIVE" \
  "tools/ffmpeg-$VERSION-amd64-static.tar.xz" "$SHA256"; then
  OK=1
fi
if [[ -z "$OK" ]]; then
  echo 'No source served the historical extractor archive.' >&2
  exit 1
fi
tar -xJf "$ARCHIVE" -C "$WORK" "ffmpeg-$VERSION-amd64-static/ffmpeg"
install -m 755 "$WORK/ffmpeg-$VERSION-amd64-static/ffmpeg" "$DEST/ffmpeg"
ACTUAL=$("$DEST/ffmpeg" -version | awk 'NR == 1 { print $3 }')
if [[ "$ACTUAL" != "$VERSION-static" ]]; then
  echo "Historical extractor version mismatch: $ACTUAL" >&2
  exit 1
fi
echo "Historical training extractor: $ACTUAL"
