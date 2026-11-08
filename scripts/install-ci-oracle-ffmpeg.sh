#!/usr/bin/env bash
# Install the pinned oracle FFmpeg (scripts/oracle-ffmpeg.env): test-only
# black-box binaries; no libraries or implementation source.
set -euo pipefail
DEST="${1:?usage: install-ci-oracle-ffmpeg.sh /absolute/bin/dir}"
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
CACHED_ARCHIVE="${COREAV_ORACLE_ARCHIVE:-${XDG_CACHE_HOME:-$HOME/.cache}/coreav-oracle/$SHA256.tar.xz}"
pinned() {
  echo "$SHA256  $ARCHIVE_FILE" | sha256sum --check --strict
}
if [[ -f "$CACHED_ARCHIVE" ]]; then
  cp "$CACHED_ARCHIVE" "$ARCHIVE_FILE"
  if ! pinned; then
    echo 'install-ci-oracle-ffmpeg.sh: discarding an unverified cached archive' >&2
    rm -f "$ARCHIVE_FILE"
  fi
fi
if [[ ! -f "$ARCHIVE_FILE" ]]; then
  if [[ "${COREAV_ORACLE_FROM_R2:-0}" == 1 ]] ||
    ! { curl --fail --location --retry 5 --retry-all-errors --retry-connrefused \
      --retry-max-time 300 --connect-timeout 20 --max-time 60 \
      "$URL" --output "$ARCHIVE_FILE" && pinned; }; then
    if [[ -n "${R2_ACCESS_KEY_ID:-}" && -n "${R2_SECRET_ACCESS_KEY:-}" && -n "${R2_ENDPOINT:-}" ]]; then
      echo 'install-ci-oracle-ffmpeg.sh: taking the pinned archive from R2' >&2
      python3 -I "$HERE/oracle_from_r2.py" "$ARCHIVE_FILE"
      pinned
    else
      echo 'install-ci-oracle-ffmpeg.sh: oracle_download_unavailable after bounded retries' >&2
      exit 1
    fi
  fi
  mkdir -p "$(dirname "$CACHED_ARCHIVE")"
  CACHE_PART=$(mktemp "$CACHED_ARCHIVE.XXXXXX")
  cp "$ARCHIVE_FILE" "$CACHE_PART"
  mv -f "$CACHE_PART" "$CACHED_ARCHIVE"
fi
tar -xJf "$ARCHIVE_FILE" -C "$WORK" "$ARCHIVE/bin/ffmpeg" "$ARCHIVE/bin/ffprobe"
python3 -I "$HERE/oracle_pin.py" --verify-bin "$WORK/$ARCHIVE/bin"
install -m 755 "$WORK/$ARCHIVE/bin/ffmpeg" "$WORK/$ARCHIVE/bin/ffprobe" "$DEST/"
python3 -I "$HERE/oracle_pin.py" --verify-bin "$DEST"
"$DEST/ffmpeg" -version
"$DEST/ffprobe" -version
