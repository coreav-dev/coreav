#!/bin/bash
# prof8.sh BIN OUT THREADS: perf cpu-clock samples of --discard on the 32 training streams, per-symbol self share
BIN=$1; OUT=$2; T=$3; mkdir -p $OUT
ES=~/h264work/train/es
for f in $ES/*.h264; do
  s=$(basename $f .h264); c=${s##*.}; case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
  perf record -q -F 2000 -e cpu-clock -o /tmp/p8.data -- $BIN --threads $T --frames $n --discard $f >/dev/null 2>&1
  perf report -i /tmp/p8.data --no-children --sort sym --stdio -q -n 2>/dev/null | head -80 > $OUT/$s.txt
done
rm -f /tmp/p8.data
