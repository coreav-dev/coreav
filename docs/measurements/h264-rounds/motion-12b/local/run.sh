#!/bin/bash
# local direction: base (main 686fb48f) vs m (Motion 12 B), gap-2 streams, pinned, 5 rounds; default malloc and MALLOC_MMAP_THRESHOLD_=131072
set -u
E=$HOME/h264work/training; O=.; R=$HOME/coreav
DEC="--decoder base=/tmp/mbc/decode --decoder m=/tmp/mbc/decode-m"
for env in def mmap; do
  for c in 1080p:300 720p:900 sd:1800 4k:150; do
    cls=${c%%:*}; F=${c##*:}
    S=$(awk -v c=$cls '$2==c{print "'$E'/"$1".0."c".h264"}' /tmp/prof2/gap/streams.txt)
    if [ $env = mmap ]; then export MALLOC_MMAP_THRESHOLD_=131072; else unset MALLOC_MMAP_THRESHOLD_; fi
    python3 -I $R/scripts/h264_parity.py $DEC --frames $F --rounds 5 --pin 2 --report $O/$env.$cls.json $S > $O/$env.$cls.log 2>&1
    echo "$env $cls done"
  done
done
echo DONE > $O/done
