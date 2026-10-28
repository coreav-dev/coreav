#!/bin/bash
D=$HOME/ap/t1x; mkdir -p $D; cd ~/ap/src-obase
for c in sd:1800 720p:900 1080p:300 4k:150; do cls=${c%%:*}; F=${c##*:}
  python3 -I scripts/h264_parity.py --decoder base=$HOME/ap/decode-obase --decoder k2=$HOME/ap/decode-ok --decoder k3=$HOME/ap/decode-k3 \
    --frames $F --pin 2 --rounds 6 --shuffle-seed 11 \
    --report $D/g.$cls.json ~/tr/es/*.$cls.h264 > $D/g.$cls.log 2>&1
  echo "$cls done $(date -u +%H:%M)" >> $D/progress.txt
done
echo ALL >> $D/progress.txt
