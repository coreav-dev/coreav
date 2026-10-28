#!/bin/bash
# round 10's P_Skip-heavy 4k streams, base vs m, 11 rounds, both malloc settings
set -u
E=$HOME/h264work/training; O=.; R=$HOME/coreav
DEC="--decoder base=/tmp/mbc/decode --decoder m=/tmp/mbc/decode-m"
for env in def mmap; do
  if [ $env = mmap ]; then export MALLOC_MMAP_THRESHOLD_=131072; else unset MALLOC_MMAP_THRESHOLD_; fi
  python3 -I $R/scripts/h264_parity.py $DEC --frames 150 --rounds 11 --pin 2 --report $O/r3.$env.4k.json $E/ed75c044e944.0.4k.h264 $E/81ed4778ece2.0.4k.h264 > $O/r3.$env.4k.log 2>&1
  echo "$env done"
done
echo DONE > $O/done3
