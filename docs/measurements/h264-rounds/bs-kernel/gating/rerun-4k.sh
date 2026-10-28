#!/bin/bash
# Re-run of the g.4k call of the bS kernel gating run (run-ab-helper.sh), alone: the first g.4k call had 8 unresolved streams, above the
# four of the bars ("More than four unresolved streams in its calls make the run a host fault"). Same binaries (decode-base, decode-k of
# that run), same options; the first call is kept as g.4k.json.void1 and g.4k.log.void1; this is attempt 2.
# Gating run for the boundary-strength kernel round, one-thread arms: run-ab-helper.sh BASE K (BASE: main, the commit
# the branch is based on; K: the branch's code commit). Bars BS-1 to BS-3 and BS-5's identity of docs/design/h264.md,
# "Kernel round: boundary strengths in vector registers"; the eight-thread arms (BS-4) run on Compute Engine
# (run-gcp.sh). The harness runs with the options of "Measurement method: host faults inside a call": a seeded order
# per round, a null arm of base, and the in-call gate on one training stream. Every timing call is bracketed by the
# host gate (HOST-GATE.md, HOSTGATE below): a gate before it (retried a minute apart, at most 10 times) and after it;
# a failed after-gate voids the call (kept as *.void<N>) and it runs again, at most 3 attempts. One thread pinned to
# core 2 at twenty rounds (g), per class. Paths are this VM's.
set -euo pipefail
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || { echo "RUSTFLAGS is set; stopping" >&2; exit 4; }
BASE=$1; K=$2
HOSTGATE=${HOSTGATE:-$HOME/hg/hostgate.py}
OUT=$HOME/ab/bs-kernel/out; W=$HOME/ab/bs-kernel/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
# The bars are fixed for the 2.90 GHz class (family 6, model 106) alone; both arms run in the same calls on one
# host, and the class is recorded.
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
model=$(lscpu | awk -F: '/^Model:/{gsub(/ /,"",$2); print $2}')
case "$cpu/$model" in
  "@ 2.90GHz/106") ;;
  *) echo "host is $cpu model $model, not a bar class; stopping" > $OUT/STOPPED; exit 4;;
esac
REQ="$cpu"
gate() { python3 -I $HOSTGATE "$1" >> $OUT/hostgate.log 2>&1; }
# One call a class: one thread pinned to core 2 (g).
for mode in g; do
OPTS="--pin 2 --rounds 20"
for c in 4k:150; do
  cls=${c%%:*}; F=${c##*:}; done_ok=0
  for attempt in 2 3; do
    ok=0; for i in $(seq 10); do gate "before $mode.$cls #$attempt" && { ok=1; break; }; sleep 60; done
    [ $ok = 1 ] || { echo "host noisy before $mode.$cls; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 5; }
    python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder k=$W/decode-k \
      --ffmpeg $FF --frames $F $OPTS --require-cpu "$REQ" --report $OUT/$mode.$cls.json \
      --shuffle-seed 20261026 --null-arm base --gate-stream $E/41e324ebb708.0.sd.h264 --gate-every 4 \
      $E/*.$cls.h264 > $OUT/$mode.$cls.log 2>&1
    if gate "after $mode.$cls #$attempt"; then done_ok=1; break; fi
    mv $OUT/$mode.$cls.json $OUT/$mode.$cls.json.void$attempt; mv $OUT/$mode.$cls.log $OUT/$mode.$cls.log.void$attempt
    echo "VOID $mode.$cls attempt $attempt $(date -u +%H:%M)" | tee -a $OUT/hostgate.log >> $OUT/progress.txt
  done
  [ $done_ok = 1 ] || { echo "$mode.$cls void three times; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 6; }
  echo "$mode.$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
done
echo ALL4K >> $OUT/progress.txt
