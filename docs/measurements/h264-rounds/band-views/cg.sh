#!/bin/bash
# cg.sh STREAM_PREFIX THREADS
mkdir -p cg; ES=~/h264work/train/es; f=$(ls $ES | grep ^$1); c=$(echo $f | cut -d. -f3)
case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
valgrind --tool=callgrind --callgrind-out-file=cg/$1.t$2.out ~/h264work/bins/m-2a3972b --frames $n --threads $2 --discard $ES/$f > /dev/null 2> cg/$1.t$2.log
