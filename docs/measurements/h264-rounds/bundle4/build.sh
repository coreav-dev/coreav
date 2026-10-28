#!/bin/bash
# build.sh: prof binaries (dist with line tables) of every arm, one target dir each: ~/b4/src-base = main at 41c62136,
# ~/b4/src-H = rejected/h264-banded-hints (bb9547a6), ~/b4/src-k = bundle 4 (db25cc6a).
set -euo pipefail
[ -z "${RUSTFLAGS:-}" ] || { echo RUSTFLAGS set; exit 4; }
for a in ${ARMS:-base H k}; do
  (cd ~/b4/src-$a && CARGO_PROFILE_DIST_DEBUG=line-tables-only CARGO_TARGET_DIR=~/b4/ptgt-$a cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp ~/b4/ptgt-$a/dist/examples/decode ~/b4/prof-$a
done
sha256sum ~/b4/prof-*
