#!/bin/bash
# resume-class.sh NAME CALL CLASS FRAMES: the one call of run-1t.sh that a hard timeout killed, with the same binaries, harness and options (recorded as a crashed run, a host fault by the re-run rule).
set -euo pipefail
NAME=$1; call=$2; cls=$3; F=$4
OUT=$HOME/ab/$NAME/out; W=$HOME/ab/$NAME/w; E=$HOME/tr/es; H=$HOME/hd/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg; GATE=$E/36501d448eaa.0.720p.h264; HOSTGATE=$HOME/hg/hostgate.py
case $call in g) OPTS="--pin 2 --rounds 20"; D=$E;; h) OPTS="--pin 2 --rounds 5"; D=$H;; esac
gate() { python3 -I $HOSTGATE "$1" >> $OUT/hostgate.log 2>&1; }
done_ok=0
for attempt in 1 2 3; do
  ok=0; for i in $(seq 10); do gate "before $call.$cls #$attempt (resumed)" && { ok=1; break; }; sleep 60; done
  [ $ok = 1 ] || { echo "host noisy before $call.$cls; stopping" > $OUT/STOPPED; exit 5; }
  python3 -I $W/h264_parity.py --decoder base=$W/decode-base --decoder k=$W/decode-k --ffmpeg $FF --frames $F $OPTS --require-cpu "@ 2.90GHz" --shuffle-seed ${SEED:-20261026} --null-arm base \
    --gate-stream $GATE --gate-every 4 --report $OUT/$call.$cls.json $D/*.$cls.h264 > $OUT/$call.$cls.log 2>&1
  if gate "after $call.$cls #$attempt (resumed)"; then done_ok=1; break; fi
  mv $OUT/$call.$cls.json $OUT/$call.$cls.json.void$attempt; mv $OUT/$call.$cls.log $OUT/$call.$cls.log.void$attempt
  echo "VOID $call.$cls attempt $attempt $(date -u +%H:%M)" | tee -a $OUT/hostgate.log >> $OUT/progress.txt
done
[ $done_ok = 1 ] || { echo "$call.$cls void three times; stopping" > $OUT/STOPPED; exit 6; }
echo "$call.$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
