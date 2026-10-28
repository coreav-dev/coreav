#!/bin/sh
# ident.sh BIN: md5 of decoded output per stream, base vs BIN at 1 and 8 threads.
ES=$HOME/h264work/train/es; B=$HOME/h264work/bins
for f in $(ls $ES); do
  a=$($B/r28-base $ES/$f | md5sum | cut -c1-32); o="$f $a"; s=same
  for t in 1 8; do x=$($B/$1 --threads $t $ES/$f | md5sum | cut -c1-32); o="$o $1/$t=$(echo $x|cut -c1-8)"; [ $x = $a ] || s=DIFF; done
  echo "$o $s"
done
