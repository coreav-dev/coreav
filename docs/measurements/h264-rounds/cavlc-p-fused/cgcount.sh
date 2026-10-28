#!/bin/bash
# cgcount.sh BIN OUTDIR : callgrind Ir, a tenth of class frames, all 32 streams, 8 parallel
BIN=$1; OUT=$2; mkdir -p $OUT
ls /tmp/pmu/up/es/*.h264 | xargs -P 8 -I{} bash -c 'f={}; b=$(basename $f .h264); c=${b#*.0.}; case $c in sd) n=180;; 720p) n=90;; 1080p) n=30;; 4k) n=15;; esac; timeout 1800 valgrind --tool=callgrind --callgrind-out-file=/dev/null '$BIN' --frames $n --threads 1 --discard $f 2>&1 >/dev/null | grep -o "Collected : [0-9]*" | sed "s/Collected : /$b /" > '$OUT'/$b.txt'
