#!/bin/bash
# build.sh: prof binaries (dist with line tables) of both arms, one target dir each: ~/r5/main = main at 82ad7c31,
# ~/r5/tree = this round's branch.
set -euo pipefail
[ -z "${RUSTFLAGS:-}" ] || { echo RUSTFLAGS set; exit 4; }
for a in base k; do
  d=~/r5/main; [ $a = k ] && d=~/r5/tree
  (cd $d && CARGO_PROFILE_DIST_DEBUG=line-tables-only CARGO_TARGET_DIR=~/r5/ptgt-$a cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp ~/r5/ptgt-$a/dist/examples/decode ~/r5/prof-$a
done
sha256sum ~/r5/prof-*
