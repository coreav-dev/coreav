#!/bin/bash
# gcp_round.sh NAME BASE K [extra h264_parity_gcp.py args]: the eight-thread arms of a round on Compute Engine
# (docs/design/h264.md, "Where the eight-thread arms run"). Builds both arms here (dist, --locked, one toolchain,
# RUSTFLAGS unset), then runs scripts/h264_parity_gcp.py of origin/main with the training streams, FFmpeg 9.0.2
# and the in-call gate stream. Output ~/ab/NAME/gcp-outN (a new directory per run).
set -euo pipefail
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || { echo "RUSTFLAGS is set; stopping" >&2; exit 4; }
NAME=$1; BASE=$2; K=$3; shift 3
EXTRA=("$@")
R=$HOME/ab/$NAME; W=$R/gw; mkdir -p $W
REPO=$HOME/coreav
git -C $REPO fetch -q origin
for pair in "base $BASE" "k $K"; do
  name=${pair%% *}; rev=${pair##* }
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name ${BUILD_PREFIX:-} cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
  rm -rf $W/tgt-$name
done
mkdir -p $W/tools
for f in h264_parity_gcp.py gcp_vm_job.py h264_parity.py; do git -C $REPO show origin/main:scripts/$f > $W/tools/$f; done
echo "tools of origin/main $(git -C $REPO rev-parse --short origin/main)" > $R/tools.txt
n=1; while [ -e $R/gcp-out$n ]; do n=$((n+1)); done
python3 $W/tools/h264_parity_gcp.py --out $R/gcp-out$n --es-dir ${ESDIR:-$HOME/tr/es} \
  --decoder base=$W/decode-base --decoder k=$W/decode-k \
  --ffmpeg $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg \
  --gate-stream $HOME/tr/es/36501d448eaa.0.720p.h264 --null-arm base "${EXTRA[@]}"
