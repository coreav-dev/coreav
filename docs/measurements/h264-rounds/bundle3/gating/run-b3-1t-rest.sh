#!/bin/bash
# Continuation of run-b3-1t.sh: that run was killed by its own 10 h timeout during the held-out 1080p call (a crashed run, no report written for it); h.sd, h.720p and all g.* calls are complete. This script runs the remaining held-out calls (1080p, 4k) with the same binaries, harness and options; identity (96 of 96) and provenance are those of the first run.
# Gating run of PR #546 (bundle 3) (B3-1 to B3-4 and B3-7: one thread pinned, discard and write, twenty rounds over the 32 training streams (g), five over the 64 held-out (h), --min-seconds 3): base = main 974f46c1, k = perf/h264-bundle3 (e40e6db2; its crates/ are 72cf01fe's). Adapted from the first-touch round's run-ft-1t.sh: the same binaries as the Compute Engine arms (w/decode-base, w/decode-k).
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
BASE=974f46c1; K=e40e6db2
HOSTGATE=${HOSTGATE:-$HOME/hg/hostgate.py}
OUT=$HOME/ab/bundle3/out; W=$HOME/ab/bundle3/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es; G=$HOME/tr/es/36501d448eaa.0.720p.h264
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
for set in "h:$HOME/hd/es:5"; do
IFS=: read mode E rounds <<< "$set"
OPTS="--pin 2 --rounds $rounds --min-seconds 3"
for c in 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}; done_ok=0
  for attempt in 1 2 3; do
    ok=0; for i in $(seq 10); do gate "before $mode.$cls #$attempt" && { ok=1; break; }; sleep 60; done
    [ $ok = 1 ] || { echo "host noisy before $mode.$cls; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 5; }
    python3 -I $W/h264_parity.py --decoder base=$W/decode-base --decoder k=$W/decode-k \
      --ffmpeg $FF --frames $F $OPTS --require-cpu "$REQ" --report $OUT/$mode.$cls.json \
      --shuffle-seed 20261027 --null-arm base --gate-stream $G --gate-every 4 \
      $E/*.$cls.h264 > $OUT/$mode.$cls.log 2>&1
    if gate "after $mode.$cls #$attempt"; then done_ok=1; break; fi
    mv $OUT/$mode.$cls.json $OUT/$mode.$cls.json.void$attempt; mv $OUT/$mode.$cls.log $OUT/$mode.$cls.log.void$attempt
    echo "VOID $mode.$cls attempt $attempt $(date -u +%H:%M)" | tee -a $OUT/hostgate.log >> $OUT/progress.txt
  done
  [ $done_ok = 1 ] || { echo "$mode.$cls void three times; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 6; }
  echo "$mode.$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
done
# The analysis beside this script.
echo ALL >> $OUT/progress.txt
