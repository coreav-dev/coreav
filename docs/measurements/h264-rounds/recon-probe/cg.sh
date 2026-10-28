#!/bin/bash
# cg.sh STREAM: callgrind, the class frame count, the same options as the perf runs (no cache simulation).
f=$1; b=$(basename $f .h264); c=${b##*.}
case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
cd /tmp/rp && valgrind --tool=callgrind --dump-instr=yes --callgrind-out-file=cg/$b.out ./prof-main --kernels avx2 --threads 1 --discard --frames $n $f > cg/$b.log 2>&1
