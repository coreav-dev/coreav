#!/bin/bash
# run_cg.sh STREAM: callgrind of CoreAV (instr level) and FFmpeg (fn level) at 1/10 of class frames
f=$1; c=$(echo $f | cut -d. -f3)
case $c in sd) n=180;; 720p) n=90;; 1080p) n=30;; 4k) n=15;; esac
s=$(echo $f | cut -d. -f1)
ES=/tmp/pmu/up/es
valgrind --tool=callgrind --dump-instr=yes --dump-line=no --callgrind-out-file=/tmp/ent/cg/$s.$c.core.out \
  /tmp/pmu/up/prof-b3664291 --frames $n --discard $ES/$f >/dev/null 2>/tmp/ent/cg/$s.core.log
valgrind --tool=callgrind --callgrind-out-file=/tmp/ent/cg/$s.$c.ff.out \
  /tmp/ent/ffsym -nostdin -v error -threads 1 -i $ES/$f -frames:v $n -f null - >/dev/null 2>/tmp/ent/cg/$s.ff.log
echo $f $n
