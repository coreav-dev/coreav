#!/bin/bash
# cg.sh STREAM: callgrind with cache simulation, the class frame count, the same options as the perf runs.
f=$1; b=$(basename $f .h264); c=${b##*.}
case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
cd /tmp/mbs && valgrind --tool=callgrind --dump-instr=yes --cache-sim=yes --D1=49152,12,64 --callgrind-out-file=cg/$b.out ./prof-main --kernels avx2 --threads 1 --discard --frames $n $f > cg/$b.log 2>&1
