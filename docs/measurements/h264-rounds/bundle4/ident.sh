#!/bin/bash
# ident.sh A B: every training stream at the class frame counts, 1 and 8 threads: output md5 of A and of B.
A=$1; B=$2
for f in ~/tr/es/*.h264; do c=$(basename $f | cut -d. -f3)
  case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
  for t in 1 8; do
    a=$($A --frames $n --threads $t $f | md5sum | cut -c1-32); b=$($B --frames $n --threads $t $f | md5sum | cut -c1-32)
    echo "$(basename $f) t$t $([ $a = $b ] && echo same || echo DIFF) $a"
  done
done
