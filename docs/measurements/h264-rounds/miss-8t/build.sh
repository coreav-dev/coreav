#!/bin/bash
# build.sh: the two arms' binaries (dist with line tables): ~/m8/src-main = main at df389085, ~/m8/src-ft = df389085
# with #508's code commit 68c8f059 cherry-picked (99f6e3e9 locally; picture.rs only).
set -euo pipefail
[ -z "${RUSTFLAGS:-}" ] || { echo RUSTFLAGS set; exit 4; }
for a in main ft; do
  (cd ~/m8/src-$a && CARGO_PROFILE_DIST_DEBUG=line-tables-only CARGO_TARGET_DIR=~/m8/tgt-$a cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp ~/m8/tgt-$a/dist/examples/decode ~/m8/prof-$a
done
sha256sum ~/m8/prof-*
