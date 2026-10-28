#!/bin/bash
# perf.sh: perf cpu-clock at 10 kHz, five runs a stream with the AVX2 kernels (pd/S.R.data) and five with the
# detected kernels (pdd/S.R.data), class frame counts, pinned to CPU 2. D (env) is the work directory, ES the streams.
cd "$D" && mkdir -p pd pdd
for r in 0 1 2 3 4; do
  for f in "$ES"/*.h264; do
    b=$(basename $f .h264); c=${b##*.}
    case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
    taskset -c 2 perf record -q -e cpu-clock -F 10000 -o pd/$b.$r.data ./prof-main --kernels avx2 --threads 1 --discard --frames $n $f >/dev/null 2>>perf.err
    taskset -c 2 perf record -q -e cpu-clock -F 10000 -o pdd/$b.$r.data ./prof-main --threads 1 --discard --frames $n $f >/dev/null 2>>perf.err
  done
done
echo done > perf.done
