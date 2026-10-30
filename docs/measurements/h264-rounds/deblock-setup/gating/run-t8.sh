#!/bin/bash
# The eight-thread calls of the deblock-setup gating run, run again with the harness of main acf3ee2f (the in-call
# gate's baseline after a discarded warm sample). The first t8.sd call of the run (the harness before that fix) read the gate's cold
# first sample as the baseline, so every later sample drifted -24% and every stream ended unresolved; it is kept, void,
# in t8-aborted/. The binaries, the identity check and the one-thread calls (g) are those of run-ab-helper.sh's first
# part, unchanged. Same options as run-ab-helper.sh's t8 calls.
# Second change: the in-call gate (an sd stream of 300 frames, 0.12 s, one thread unpinned) read +10.9% and -6.2% in one call
# (noise of a short run), so it is the same stream at 1800 frames, one thread on core 2, repeated to 3 s (--gate-frames 1800
# --gate-threads 1 --gate-pin 2); the first re-run is kept in t8-aborted-2/.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
HOSTGATE=${HOSTGATE:-$HOME/hg/hostgate.py}
OUT=$HOME/ab/deblock-setup/out; W=$HOME/ab/deblock-setup/w
E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
model=$(lscpu | awk -F: '/^Model:/{gsub(/ /,"",$2); print $2}')
case "$cpu/$model" in
  "@ 2.90GHz/106") ;;
  *) echo "host is $cpu model $model, not a bar class; stopping" > $OUT/STOPPED; exit 4;;
esac
REQ="$cpu"
{ echo "t8 re-run: harness $HERE/h264_parity_acf3ee2f.py"; sha256sum $HERE/h264_parity_acf3ee2f.py $W/decode-base $W/decode-k; } >> $OUT/provenance.txt
gate() { python3 -I $HOSTGATE "$1" >> $OUT/hostgate.log 2>&1; }
mode=t8; OPTS="--threads 8 --rounds 10 --min-seconds 3"
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}; done_ok=0
  for attempt in 1 2 3; do
    ok=0; for i in $(seq 10); do gate "before $mode.$cls #$attempt" && { ok=1; break; }; sleep 60; done
    [ $ok = 1 ] || { echo "host noisy before $mode.$cls; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 5; }
    python3 -I $HERE/h264_parity_acf3ee2f.py --decoder base=$W/decode-base --decoder k=$W/decode-k \
      --ffmpeg $FF --frames $F $OPTS --require-cpu "$REQ" --report $OUT/$mode.$cls.json \
      --shuffle-seed 20261026 --null-arm base --gate-stream $E/41e324ebb708.0.sd.h264 --gate-frames 1800 --gate-threads 1 --gate-pin 2 --gate-every 4 \
      $E/*.$cls.h264 > $OUT/$mode.$cls.log 2>&1
    if gate "after $mode.$cls #$attempt"; then done_ok=1; break; fi
    mv $OUT/$mode.$cls.json $OUT/$mode.$cls.json.void$attempt; mv $OUT/$mode.$cls.log $OUT/$mode.$cls.log.void$attempt
    echo "VOID $mode.$cls attempt $attempt $(date -u +%H:%M)" | tee -a $OUT/hostgate.log >> $OUT/progress.txt
  done
  [ $done_ok = 1 ] || { echo "$mode.$cls void three times; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 6; }
  echo "$mode.$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
cp "$HERE/analyze.py" $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
