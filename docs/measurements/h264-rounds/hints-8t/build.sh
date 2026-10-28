#!/bin/bash
# build.sh: prof binaries (dist with line tables) of every arm, one target dir each: ~/c8/src-base = main at 7af5d4f5,
# ~/c8/src-EWO = the cut (perf/h264-8t-hints), src-E, src-EW, src-EO = the cut with BANDED_HINTS narrowed (one commit each).
set -euo pipefail
[ -z "${RUSTFLAGS:-}" ] || { echo RUSTFLAGS set; exit 4; }
for a in ${ARMS:-base E EW EO EWO}; do
  (cd ~/c8/src-$a && CARGO_PROFILE_DIST_DEBUG=line-tables-only CARGO_TARGET_DIR=~/c8/tgt-$a cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp ~/c8/tgt-$a/dist/examples/decode ~/c8/prof-$a
done
sha256sum ~/c8/prof-*
