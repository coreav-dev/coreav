#!/usr/bin/env bash
set -euo pipefail
if [[ $# -lt 4 ]]; then
  echo 'usage: fetch-ci-archive.sh URL SHA256 OUTPUT CACHE [CURL_OPTIONS...]' >&2
  exit 2
fi
URL=$1
DIGEST=$2
OUTPUT=$3
CACHE=$4
shift 4
if [[ ! "$DIGEST" =~ ^[0-9a-f]{64}$ ]]; then
  echo 'reason=ci_archive_invalid_digest' >&2
  exit 2
fi
WORK=$(mktemp -d "${TMPDIR:-/tmp}/coreav-ci-archive.XXXXXX")
PART=''
trap 'rm -rf "$WORK"; if [[ -n "$PART" ]]; then rm -f "$PART" || true; fi' EXIT
ARCHIVE="$WORK/archive"
pinned() {
  printf '%s  %s\n' "$DIGEST" "$ARCHIVE" | shasum -a 256 --check >&2
}
if [[ -f "$CACHE" ]]; then
  if ! { cp "$CACHE" "$ARCHIVE" && pinned; }; then
    echo 'reason=ci_archive_cache_rejected' >&2
    rm -f "$ARCHIVE"
  fi
fi
if [[ ! -f "$ARCHIVE" ]]; then
  if ! curl --fail --silent --show-error --location --retry 8 --retry-all-errors \
      --retry-connrefused --retry-max-time 360 --connect-timeout 20 --max-time 300 \
      --speed-limit 1024 --speed-time 30 "$@" "$URL" --output "$ARCHIVE"; then
    echo 'reason=ci_archive_download_unavailable' >&2
    exit 1
  fi
  if ! pinned; then
    echo 'reason=ci_archive_digest_mismatch' >&2
    exit 1
  fi
  if ! { mkdir -p "$(dirname "$CACHE")" && PART=$(mktemp "$CACHE.XXXXXX") &&
      cp "$ARCHIVE" "$PART" && mv -f "$PART" "$CACHE"; }; then
    echo 'reason=ci_archive_cache_write_failed' >&2
  fi
fi
mkdir -p "$(dirname "$OUTPUT")"
cp "$ARCHIVE" "$OUTPUT"
