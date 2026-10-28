#!/bin/bash
set -u
E=$HOME/h264work/training; O=.; R=$HOME/coreav
DEC="--decoder base=/tmp/mbc/decode --decoder m=/tmp/mbc/decode-m"
for env in def mmap; do
  if [ $env = mmap ]; then export MALLOC_MMAP_THRESHOLD_=131072; else unset MALLOC_MMAP_THRESHOLD_; fi
  python3 -I $R/scripts/h264_parity.py $DEC --frames 300 --rounds 11 --pin 2 --report $O/r2.$env.1080p.json $E/6da34958296c.0.1080p.h264 $E/b71b6b31e54f.0.1080p.h264 > $O/r2.$env.1080p.log 2>&1
  python3 -I $R/scripts/h264_parity.py $DEC --frames 150 --rounds 11 --pin 2 --report $O/r2.$env.4k.json $E/e624cbe0d2be.0.4k.h264 > $O/r2.$env.4k.log 2>&1
  echo "$env done"
done
echo DONE > $O/done2
