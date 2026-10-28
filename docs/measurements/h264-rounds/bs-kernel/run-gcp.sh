#!/bin/bash
# Gating run for the boundary-strength kernel round, eight-thread arms (BS-4 of docs/design/h264.md, "Kernel round:
# boundary strengths in vector registers"): run-gcp.sh BASE K OUT. Builds both decoders alike from one worktree
# each, as run-ab-helper.sh does, then runs scripts/h264_parity_gcp.py of the base tree ("Where the eight-thread
# arms run"): one new c4-standard-16 VM, one call a class at eight threads, --min-seconds 3, ten rounds, seed
# 20261026, a null arm of base, the in-call gate and the host check on 41e324ebb708.0.sd.h264, the null and spread
# bars of that host, at most 2.5 hours and 5 USD. OUT receives t8.<class>.json, host.txt, ref.json, spend.json and
# verdict.json; a void verdict means a new run into a new directory. Paths are this VM's.
set -euo pipefail
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || { echo "RUSTFLAGS is set; stopping" >&2; exit 4; }
BASE=$1; K=$2; OUT=$3
W=$HOME/gcp/bs-kernel/w; mkdir -p $W $OUT
REPO=$HOME/coreav; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
git -C $REPO fetch -q origin
for pair in "base $BASE" "k $K"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "base = $BASE, k = $K"
  sha256sum $W/decode-base $W/decode-k $FF
  rustc -vV
} > $OUT/provenance.txt
python3 -I $W/src-base/scripts/h264_parity_gcp.py --out $OUT --es-dir $E \
  --decoder base=$W/decode-base --decoder k=$W/decode-k --ffmpeg $FF \
  --gate-stream $E/41e324ebb708.0.sd.h264 --null-arm base \
  --rounds 10 --min-seconds 3 --seed 20261026 --max-hours 2.5 --cap-usd 5 2>&1 | tee $OUT/run.log | tail -3
