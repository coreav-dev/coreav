#!/bin/bash
# Probe AP (plan-probe.md): run-gcp.sh OUT. Runs scripts/h264_parity_gcp.py of the base tree on one new
# c4-standard-16 VM ("Where the eight-thread arms run"): decoders base, twice and none (built by ../build.sh into
# ~/ap/decode-*), null arm base, one call a class at eight threads, 5 rounds, --min-seconds 3, seed 20261026, gate
# stream 41e324ebb708.0.sd.h264, at most 3 hours and 5 USD. OUT receives t8.<class>.json, host.txt, ref.json,
# spend.json and verdict.json; a void verdict means a new run into a new directory. Paths are this VM's.
set -uo pipefail
OUT=$1; P=$(mktemp -d)
E=$HOME/tr/es; A=$HOME/ap
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
{
  echo "build: (cd <worktree at rev> && cargo build --locked --profile dist --example decode -p coreav-h264)"
  for n in base twice none; do echo "$n = $(git -C $A/src-$n rev-parse HEAD)"; done
  sha256sum $A/decode-base $A/decode-twice $A/decode-none $FF
  rustc -vV
} > $P/provenance.txt
python3 -I $A/src-base/scripts/h264_parity_gcp.py --out $OUT --es-dir $E \
  --decoder base=$A/decode-base --decoder twice=$A/decode-twice --decoder none=$A/decode-none --ffmpeg $FF \
  --gate-stream $E/41e324ebb708.0.sd.h264 --null-arm base \
  --rounds 5 --min-seconds 3 --seed 20261026 --zone ${ZONE:-us-central1-a} --max-hours 3 --cap-usd 5 2>&1 | tee $P/run.log | tail -3
mkdir -p $OUT && cp $P/provenance.txt $P/run.log $OUT/
