#!/usr/bin/env bash
# Oracle adapter for the coreav binary: runs
# `coreav probe --ffprobe-compat` and normalizes its output with the same
# normalizer the reference ffprobe capture uses, so the CLI's result can be
# compared with the library adapter's (tools/isobmff-adapter) on every
# gating field. Streams level only.
# Usage: oracle-coreav-cli.sh streams MEDIA_FILE
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEVEL="${1:?usage: oracle-coreav-cli.sh streams MEDIA_FILE}"
MEDIA_FILE="${2:?usage: oracle-coreav-cli.sh streams MEDIA_FILE}"
COREAV="${COREAV:-$ROOT/target/release/coreav}"
COREAV_ORACLE="${COREAV_ORACLE:-$ROOT/target/release/coreav-oracle}"
FILE_NAME="${MEDIA_FILE##*/}"

if [[ "$#" -ne 2 || "$LEVEL" != "streams" ]]; then
  echo "usage: oracle-coreav-cli.sh streams MEDIA_FILE" >&2
  exit 2
fi
for exe in "$COREAV" "$COREAV_ORACLE"; do
  if [[ ! -x "$exe" ]]; then
    echo "oracle-coreav-cli.sh: not executable: $exe" >&2
    exit 2
  fi
done

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
RAW="$TMP_DIR/raw"
set +e
"$COREAV" probe --ffprobe-compat "$MEDIA_FILE" > "$RAW" 2> "$TMP_DIR/stderr"
EXIT_CODE=$?
set -e
# 0 and 1 are results (accepted, rejected); anything else is a harness error.
if [[ "$EXIT_CODE" -gt 1 ]]; then
  cat "$TMP_DIR/stderr" >&2
  exit 2
fi
"$COREAV_ORACLE" normalize streams --file "$FILE_NAME" --exit-code "$EXIT_CODE" --input "$RAW"
