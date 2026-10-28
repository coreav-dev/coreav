#!/bin/bash
# run_core.sh STREAM: callgrind of CoreAV (instr level) at 1/10 of class frames, the #483 code (prof-200f9a79)
f=$1; c=$(echo $f | cut -d. -f3)
case $c in sd) n=180;; 720p) n=90;; 1080p) n=30;; 4k) n=15;; esac
s=$(echo $f | cut -d. -f1)
valgrind --tool=callgrind --dump-instr=yes --dump-line=no --callgrind-out-file=/tmp/ent2/cg/$s.$c.core.out \
  /tmp/pmu/up/prof-200f9a79 --frames $n --discard /tmp/pmu/up/es/$f >/dev/null 2>/tmp/ent2/cg/$s.core.log
