#!/usr/bin/env bash
set -euo pipefail
DEST="${1:?usage: install-jm-decoder.sh /absolute/bin/dir}"
source "$(dirname "${BASH_SOURCE[0]}")/jm-decoder.env"
mkdir -p "$DEST"
DEST=$(cd "$DEST" && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/coreav-jm.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
curl --fail --location --retry 3 --max-time 120 "$JM_URL" -o "$WORK/jm.zip"
printf '%s  %s\n' "$JM_SHA256" "$WORK/jm.zip" | sha256sum --check --status
unzip -q "$WORK/jm.zip" -d "$WORK/source"
BUILD=$(find "$WORK/source" -name Makefile -not -path '*/lencod/*' -not -path '*/ldecod/*' -print -quit)
test -n "$BUILD"
if ! make -C "${BUILD%/*}" -j2 ldecod > /dev/null 2>&1; then
  echo 'JM black-box decoder build failed' >&2
  exit 1
fi
BINARY=$(find "$WORK/source" -type f \( -name ldecod.exe -o -name ldecod \) -perm /111 -print -quit)
test -n "$BINARY"
install -m 755 "$BINARY" "$DEST/ldecod"
{
  printf 'JM_VERSION=%s\nJM_SHA256=%s\n' "$JM_VERSION" "$JM_SHA256"
  printf 'BUILD_COMMAND=make -j2 ldecod\n'
  cc --version | head -1
  sha256sum "$DEST/ldecod"
} > "$DEST/jm-build.txt"
