#!/bin/bash
# cg.sh STREAM: callgrind of one stream (DECODE = main built with line tables, OUT = output directory), one thread, --discard
f=$1; b=$(basename $f .h264); c=${b##*.}
case $c in sd) n=240;; 720p) n=120;; 1080p) n=60;; 4k) n=30;; esac
valgrind --tool=callgrind --callgrind-out-file=$OUT/$b.out $DECODE --threads 1 --discard --frames $n $f > $OUT/$b.log 2>&1
