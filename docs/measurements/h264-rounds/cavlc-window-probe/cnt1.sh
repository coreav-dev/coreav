#!/bin/bash
f=$1; c=$(echo $f | cut -d. -f3); s=$(echo $f | cut -d. -f1)
case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
valgrind --tool=callgrind --callgrind-out-file=/tmp/win/cg.$s.out /tmp/win/prof-base --frames $n --discard /tmp/pmu/up/es/$f >/dev/null 2>/tmp/win/cg.$s.log
