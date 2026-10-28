#!/bin/bash
# identity.sh A B OUT: md5 of the frames decode-A and decode-B write, the 32 training streams at the class frame
# counts, at one and eight threads (paths are this VM's: ~/ap/decode-*, ~/tr/es).
A=$1; B=$2; OUT=$3
echo "$B against $A, --frames at the class counts, md5 of the written frames" > $OUT
for s in $(ls ~/tr/es); do
  case $s in *.sd.*) f=1800;; *.720p.*) f=900;; *.1080p.*) f=300;; *) f=150;; esac
  for t in 1 8; do
    a=$(~/ap/decode-$A --frames $f --threads $t ~/tr/es/$s | md5sum | cut -c1-32)
    b=$(~/ap/decode-$B --frames $f --threads $t ~/tr/es/$s | md5sum | cut -c1-32)
    [ "$a" = "$b" ] && v=same || v=DIFFERENT
    echo "$s threads=$t $A=$a $B=$b $v" >> $OUT
  done
done
