#!/usr/bin/env bash
# The library oracle adapter for a file's container, picked exactly as the
# coreav binary picks it, so its captures (scripts/oracle-coreav-cli.sh) are
# compared with these: tools/mkv-adapter for Matroska and WebM (the EBML
# header ID 1A 45 DF A3 first), else tools/isobmff-adapter, and, when that
# one does not recognise the container, tools/ts-adapter for MPEG transport
# streams (which have no header to recognise: the reader finds the packet
# size and the first packet itself).
#
# An adapter that fails (a usage error, a crash, a timeout) fails this
# script with its own status: a rejection is a successful run with
# `probe_ok: false`, so a non-zero status is never a container's answer and
# is never taken for one.
# Usage: oracle-library-adapter.sh LEVEL MEDIA_FILE
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MEDIA_FILE="${2:?usage: oracle-library-adapter.sh LEVEL MEDIA_FILE}"
MAGIC="$(head -c 4 "$MEDIA_FILE" | od -An -tx1 | tr -d ' \n')"
if [[ "$MAGIC" == "1a45dfa3" ]]; then
  # The binary probes with the default allowlist; so does this capture.
  MKV_ADAPTER_ALLOWLIST=default exec "${MKV_ADAPTER:-$ROOT/target/release/mkv-adapter}" "$@"
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
# Written to a file, not held in a shell variable: a packets document of a
# large file is tens of megabytes.
ISO="$TMP_DIR/iso.json"
"${ISOBMFF_ADAPTER:-$ROOT/target/release/isobmff-adapter}" "$@" > "$ISO"

# A rejection for the container alone, as the binary treats it: offer the
# file to the transport stream reader. Any other rejection is the ISO base
# media reader's answer.
if grep -q '"rejection": *"unsupported_container"' "$ISO"; then
  TS="$TMP_DIR/ts.json"
  TS_ADAPTER_ALLOWLIST=default "${TS_ADAPTER:-$ROOT/target/release/ts-adapter}" "$@" > "$TS"
  # A file neither reader recognises keeps the ISO base media rejection,
  # which is what the binary prints for it.
  if ! grep -q '"rejection": *"unsupported_container"' "$TS"; then
    cat "$TS"
    exit 0
  fi
fi
cat "$ISO"
