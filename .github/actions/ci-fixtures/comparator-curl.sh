#!/usr/bin/env bash
# Stands in for curl while scripts/composed_audio_fixtures.py builds the
# comparator: the request for the registered comparator archive is answered
# from the Actions cache, and the generator's own digest check still judges
# the bytes. Every other request is refused, so a changed address fails at
# once instead of reaching the network unnoticed.
set -euo pipefail
target=
while [ "$#" -gt 1 ]; do
  case "$1" in
    -o|--output) target="$2"; shift ;;
  esac
  shift
done
if [ "$#" -ne 1 ] || [ "$1" != "$COREAV_COMPARATOR_URL" ] || [ -z "$target" ]; then
  echo 'comparator stand-in for curl: only the registered comparator archive is served' >&2
  exit 2
fi
cp "$COREAV_COMPARATOR_ARCHIVE" "$target"
