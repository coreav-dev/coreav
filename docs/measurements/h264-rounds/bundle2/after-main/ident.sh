#!/bin/bash
# ident.sh A B OUT: md5 of the full decode of each training stream by bins A and B at 1 and 8 threads
a=$1; b=$2; out=$3; : > $out
for f in ~/h264work/train/es/*.h264; do s=$(basename $f .h264)
  for t in 1 8; do
    x=$(~/h264work/bins/$a --threads $t $f | md5sum | cut -c1-32); y=$(~/h264work/bins/$b --threads $t $f | md5sum | cut -c1-32)
    [ "$x" = "$y" ] && r=same || r=DIFF; echo "$s ${t}T $r $x" >> $out
  done
done
echo "same $(grep -c ' same ' $out) of $(wc -l < $out)" >> $out
