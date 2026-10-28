#!/bin/bash
# Local timing A/B (information): base fb893fd vs k a3e9e9a, one thread pinned to core 2, 6 rounds, all 32 streams.
# Paths are this VM's.
cd ~/i8/tree
for c in sd:1800 720p:900 1080p:300 4k:150; do cls=${c%%:*}; F=${c##*:}
  python3 -I scripts/h264_parity.py --decoder base=$HOME/ab/i4/tgt-base/dist/examples/decode \
    --decoder k=$HOME/ab/i4/tgt-i8/dist/examples/decode --frames $F --pin 2 --rounds 6 \
    --report ~/i8/loc/g.$cls.json ~/tr/es/*.$cls.h264 > ~/i8/loc/g.$cls.log 2>&1
  echo "$cls done $(date -u +%H:%M)" >> ~/i8/loc/progress.txt
done
echo ALL >> ~/i8/loc/progress.txt
