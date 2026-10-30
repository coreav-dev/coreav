#!/usr/bin/env bash
# Install the pinned oracle FFmpeg (scripts/oracle-ffmpeg.env): test-only
# black-box binaries; no libraries or implementation source.
set -euo pipefail
DEST="${1:?usage: install-oracle-ffmpeg.sh /absolute/bin/dir}"
HERE=$(dirname "${BASH_SOURCE[0]}")
# shellcheck source=oracle-ffmpeg.env
source "$HERE/oracle-ffmpeg.env"
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
  echo 'The pinned fixture binary supports Linux x86_64 only.' >&2
  exit 1
}
mkdir -p "$DEST"
WORK=$(mktemp -d "$DEST/.download.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
ARCHIVE_FILE="$WORK/ffmpeg.tar.xz"
pinned() {
  echo "$SHA256  $ARCHIVE_FILE" | sha256sum --check --strict
}
# The publisher's release is the origin of record; no third-party copy is
# tried. A host without a route to it (some sandboxes reach only a few hosts)
# takes the same archive from the bench bucket, where R2 credentials are set;
# SHA256 decides which bytes are used either way. No workflow passes R2
# credentials, so CI relies on the publisher and its actions cache. Set
# COREAV_ORACLE_FROM_R2=1 to take the bucket's copy without trying the
# publisher. curl retries transient failures only (not a 404).
if [[ "${COREAV_ORACLE_FROM_R2:-0}" == 1 ]] ||
  ! { curl --fail --location --retry 5 --retry-connrefused --retry-delay 10 \
    --retry-max-time 900 --connect-timeout 20 --max-time 900 \
    "$URL" --output "$ARCHIVE_FILE" && pinned; }; then
  echo 'install-oracle-ffmpeg.sh: taking the pinned archive from R2' >&2
  python3 -I "$HERE/oracle_from_r2.py" "$ARCHIVE_FILE"
  pinned
fi
tar -xJf "$ARCHIVE_FILE" -C "$WORK" "$ARCHIVE/bin/ffmpeg" "$ARCHIVE/bin/ffprobe"
install -m 755 "$WORK/$ARCHIVE/bin/ffmpeg" "$WORK/$ARCHIVE/bin/ffprobe" "$DEST/"
"$DEST/ffmpeg" -version
"$DEST/ffprobe" -version
