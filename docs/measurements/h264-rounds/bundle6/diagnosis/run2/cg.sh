#!/bin/bash
# callgrind: base, p14, p1z at 8 threads and base, p1z at 1 thread, a third of each class's frames, 4 at a time
mkdir -p out/cg
for f in es/*.h264; do s=$(basename $f .h264)
  case $s in *.sd) n=600;; *.720p) n=300;; *.1080p) n=100;; *.4k) n=50;; esac
  for a in base p14 p1z; do echo "$a $s $n 8"; done
  for a in base p1z; do echo "$a $s $n 1"; done
done > out/cg/calls.txt
xargs -P 4 -L1 bash -c 'valgrind --tool=callgrind --callgrind-out-file=out/cg/$0.$1.t$3.out ./decode-$0 --threads $3 --frames $2 --discard es/$1.h264 >/dev/null 2>out/cg/$0.$1.t$3.log; echo "$0 $1 t$3 rc=$?" >> out/cg/done.txt' < out/cg/calls.txt
