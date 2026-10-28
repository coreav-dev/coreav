#!/bin/bash
# AW-4: one thread pinned (core 2), six rounds, the 32 training streams at the class frame counts, base (main
# at the run) and k (the change on it) with FFmpeg, scripts/h264_parity.py of main, on the development VM. Paths are this VM's.
D=$1; mkdir -p $D; cd ~/ap/src-obase
for c in sd:1800 720p:900 1080p:300 4k:150; do cls=${c%%:*}; F=${c##*:}
  python3 -I scripts/h264_parity.py --decoder base=$HOME/ap/decode-obase --decoder k=$HOME/ap/decode-ok \
    --frames $F --pin 2 --rounds 6 --shuffle-seed 7 \
    --report $D/g.$cls.json ~/tr/es/*.$cls.h264 > $D/g.$cls.log 2>&1
  echo "$cls done $(date -u +%H:%M)" >> $D/progress.txt
done
echo ALL >> $D/progress.txt
