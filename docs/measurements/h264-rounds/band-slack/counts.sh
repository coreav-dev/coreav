#!/bin/bash
# counts.sh: decline reasons at 8 threads, all 32 streams (r32-probe, counters only)
ES=~/h264work/train/es; B=~/h264work/bins
for f in $(ls $ES); do c=$(echo $f | cut -d. -f3); case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
  echo "== $f"; $B/r32-probe --frames $n --threads 8 --discard $ES/$f 2>&1 >/dev/null | grep PROBE
done
