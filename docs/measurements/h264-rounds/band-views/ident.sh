#!/bin/bash
# ident.sh BIN...: md5 of each BIN at 8 threads against m-2a3972b at 1 thread, all 32 streams
ES=~/h264work/train/es; B=~/h264work/bins; bad=0
for f in $(ls $ES); do c=$(echo $f | cut -d. -f3); case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
  ref=$($B/${REF:-m-2a3972b} --frames $n --threads 1 $ES/$f | md5sum | cut -c1-32)
  for b in "$@"; do for t in 1 8; do m=$($B/$b --frames $n --threads $t $ES/$f | md5sum | cut -c1-32); [ "$m" = "$ref" ] || { echo "DIFF $b $t $f"; bad=1; }; done; done
done; echo "identity done bad=$bad"
