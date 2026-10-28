#!/bin/bash
# perf.sh: five perf cpu-clock runs a stream, class frame counts, pinned to CPU 2.
cd /tmp/rp
for r in 0 1 2 3 4; do
  for f in $HOME/h264work/train/es/*.h264; do
    b=$(basename $f .h264); c=${b##*.}
    case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
    taskset -c 2 perf record -q -e cpu-clock -F 10000 -o pd/$b.$r.data ./prof-main --kernels avx2 --threads 1 --discard --frames $n $f >/dev/null 2>>perf.err
  done
done
echo done > perf.done
