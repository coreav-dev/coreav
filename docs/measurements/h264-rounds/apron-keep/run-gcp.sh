#!/bin/bash
# Gating run of the aprons owned ahead, layout kept (docs/design/h264.md, "Threading round: the aprons' rows owned ahead, the decoder's layout kept"): run-gcp.sh
# OUT. scripts/h264_parity_gcp.py of main (worktree ~/ap/src-kbase) on one new c4-standard-16 VM: decoders base (main)
# and k (the change rebased on it), null arm base, FFmpeg 9.0.2,
# one call a class at eight threads, 5 rounds, --min-seconds 3, seed 20261029, gate stream 41e324ebb708.0.sd.h264,
# at most 3 hours and 5 USD. Binaries from ../apron-copies/build.sh into ~/ap/decode-*. Paths are this VM's.
set -uo pipefail
OUT=$1; P=$(mktemp -d)
E=$HOME/tr/es; A=$HOME/ap
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
ARMS="kbase:base kk:k"
{
  echo "build: (cd <worktree at rev> && cargo build --locked --profile dist --example decode -p coreav-h264)"
  for a in $ARMS; do echo "${a#*:} = $(git -C $A/src-${a%%:*} rev-parse HEAD)"; done
  for a in $ARMS; do sha256sum $A/decode-${a%%:*}; done; sha256sum $FF
  rustc -vV
} > $P/provenance.txt
python3 -I $A/src-kbase/scripts/h264_parity_gcp.py --out $OUT --es-dir $E \
  $(for a in $ARMS; do echo --decoder ${a#*:}=$A/decode-${a%%:*}; done) --ffmpeg $FF \
  --gate-stream $E/41e324ebb708.0.sd.h264 --null-arm base \
  --rounds 5 --min-seconds 3 --seed 20261029 --zone ${ZONE:-us-central1-a} --max-hours 3 --cap-usd 5 2>&1 | tee $P/run.log | tail -3
mkdir -p $OUT && cp $P/provenance.txt $P/run.log $OUT/
