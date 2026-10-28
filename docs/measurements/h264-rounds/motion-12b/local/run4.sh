#!/bin/bash
set -u
E=$HOME/h264work/training; O=.; R=$HOME/coreav
DEC="--decoder base=/tmp/mbc/decode --decoder m=/tmp/mbc/decode-m --decoder mpad=/tmp/mbc/decode-mpad"
for env in def mmap; do
  if [ $env = mmap ]; then export MALLOC_MMAP_THRESHOLD_=131072; else unset MALLOC_MMAP_THRESHOLD_; fi
  python3 -I $R/scripts/h264_parity.py $DEC --frames 150 --rounds 11 --pin 2 --report $O/r4.$env.4k.json $E/ed75c044e944.0.4k.h264 $E/e624cbe0d2be.0.4k.h264 > $O/r4.$env.4k.log 2>&1
  echo "$env done"
done
echo DONE > $O/done4
