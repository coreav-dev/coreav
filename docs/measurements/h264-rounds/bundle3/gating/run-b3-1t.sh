#!/bin/bash
# Gating run of bundle 3 (B3-1 to B3-4 and B3-7: one thread pinned, discard and write, twenty rounds over the 32 training streams (g), five over the 64 held-out (h), --min-seconds 3): base = main 974f46c1, k = perf/h264-bundle3 (e40e6db2; its crates/ are 72cf01fe's). Adapted from the first-touch round's run-ft-1t.sh: the same binaries as the Compute Engine arms (w/decode-base, w/decode-k).
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
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "base = $BASE, k = $K"
  sha256sum $W/decode-base $W/decode-k $HOSTGATE
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name"
  echo "class: $cpu model $model"
  grep -o 'avx512[a-z_0-9]*' /proc/cpuinfo | sort -u | tr '\n' ' '; echo
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of origin/main 641fb937 (tools.txt; the Compute Engine arms use the same file)"
  sha256sum $W/h264_parity.py
  echo "streams checked against docs/measurements/h264-training/es.json of the base tree:"
  python3 -I -c "
import json,hashlib
d=json.load(open('$HOME/coreav/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$E/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print(len(d)-len(bad),'of',len(d),'match; differing:',bad)"
} > $OUT/provenance.txt
: > $OUT/identity.txt
for s in $E/*.h264 $HOME/hd/es/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$($W/decode-base --frames $F --threads 1 $s | md5sum | cut -c1-32)
  st=same; r=""
  for n in k; do for t in 1 8; do
    m=$($W/decode-$n --frames $F --threads $t $s | md5sum | cut -c1-32); r="$r $m"
    [ "$m" = "$b" ] || st=DIFFERENT
  done; done
  echo "$(basename $s) $F $b$r $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
gate() { python3 -I $HOSTGATE "$1" >> $OUT/hostgate.log 2>&1; }
# One call a class: one thread pinned to core 2 (g).
for set in "g:$HOME/tr/es:20" "h:$HOME/hd/es:5"; do
IFS=: read mode E rounds <<< "$set"
OPTS="--pin 2 --rounds $rounds --min-seconds 3"
for c in sd:1800 720p:900 1080p:300 4k:150; do
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
