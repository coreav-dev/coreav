#!/usr/bin/env bash
# Install test-only black-box binaries; no libraries or implementation source.
set -euo pipefail
DEST="${1:?usage: install-oracle-ffmpeg.sh /absolute/bin/dir}"
VERSION=7.0.2
SHA256=abda8d77ce8309141f83ab8edf0596834087c52467f6badf376a6a2a4c87cf67
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
  echo 'The pinned fixture binary supports Linux x86_64 only.' >&2
  exit 1
}
mkdir -p "$DEST"
WORK=$(mktemp -d "$DEST/.download.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
# A host without a route to any publisher (some sandboxes reach only a few
# hosts) takes the same two executables from the bench bucket, each checked
# against its own SHA-256; the tarball below stays the origin of record.
# Set COREAV_ORACLE_FROM_R2=1 to take that path without trying them first.
from_r2() {
  python3 -I "$(dirname "${BASH_SOURCE[0]}")/oracle_from_r2.py" "$DEST"
}
if [[ "${COREAV_ORACLE_FROM_R2:-0}" == 1 ]]; then
  from_r2
  "$DEST/ffmpeg" -version
  "$DEST/ffprobe" -version
  exit 0
fi
# The publisher's own host only: the current release sits under releases/,
# and a release a newer one replaces moves to old-releases/ under the same
# name. No third-party copy is tried. SHA256 decides which bytes are used.
# When the host cannot be reached, the R2 copy below is the fallback where R2
# credentials are set; no workflow passes them, so CI relies on this host and
# its actions cache. curl retries transient failures only (not a 404).
URLS=(
  "https://johnvansickle.com/ffmpeg/releases/ffmpeg-$VERSION-amd64-static.tar.xz"
  "https://johnvansickle.com/ffmpeg/old-releases/ffmpeg-$VERSION-amd64-static.tar.xz"
)
OK=
for URL in "${URLS[@]}"; do
  if curl --fail --location --retry 5 --retry-connrefused --retry-delay 10 \
    --retry-max-time 900 --connect-timeout 20 --max-time 600 \
    "$URL" --output "$WORK/ffmpeg.tar.xz" &&
    echo "$SHA256  $WORK/ffmpeg.tar.xz" | sha256sum --check --strict; then
    OK=1
    break
  fi
  echo "install-oracle-ffmpeg.sh: $URL failed; trying the next source." >&2
done
if [[ -z "$OK" ]]; then
  echo 'install-oracle-ffmpeg.sh: no source served the pinned archive; taking the pinned executables from R2' >&2
  from_r2
  "$DEST/ffmpeg" -version
  "$DEST/ffprobe" -version
  exit 0
fi
tar -xJf "$WORK/ffmpeg.tar.xz" -C "$WORK" \
  "ffmpeg-$VERSION-amd64-static/ffmpeg" "ffmpeg-$VERSION-amd64-static/ffprobe"
mkdir -p "$DEST"
install -m 755 "$WORK/ffmpeg-$VERSION-amd64-static/ffmpeg" "$DEST/ffmpeg"
install -m 755 "$WORK/ffmpeg-$VERSION-amd64-static/ffprobe" "$DEST/ffprobe"
"$DEST/ffmpeg" -version
"$DEST/ffprobe" -version
