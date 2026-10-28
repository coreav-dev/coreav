#!/bin/bash
# ic.sh BIN STREAM: callgrind instruction total of one stream, one thread, --discard, callgrind class counts
b=$1; f=$2; s=$(basename $f .h264); c=${s##*.}
case $c in sd) n=240;; 720p) n=120;; 1080p) n=60;; 4k) n=30;; esac
valgrind --tool=callgrind --callgrind-out-file=/dev/null ~/h264work/bins/$b --threads 1 --discard --frames $n $f 2>&1 | grep -o "Collected : [0-9]*" | awk -v s=$s -v b=$b '{print s, b, $3}'
