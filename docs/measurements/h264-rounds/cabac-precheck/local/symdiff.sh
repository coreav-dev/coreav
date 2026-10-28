#!/bin/sh
# symdiff.sh STREAM FRAMES ROUNDS BIN_A BIN_B: perf cpu-clock samples per symbol, rounds interleaved, summed per build
s=$1; n=$2; R=$3; A=$4; B=$5; f=$(ls ~/h264work/train/es/$s*); out=~/h264work/r20/sym-$s
: > $out.raw
for r in $(seq $R); do for b in $A $B; do
  perf record -q -F 4000 -e cpu-clock -o /tmp/sd.data -- taskset -c 3 ~/h264work/bins/$b --frames $n --discard $f >/dev/null 2>&1
  perf report -i /tmp/sd.data --no-children --sort sym --stdio -q -n 2>/dev/null | awk -v b=$b '$2 ~ /^[0-9]+$/ {sym=""; for(i=4;i<=NF;i++) sym=sym" "$i; print b"\t"$2"\t"sym}' >> $out.raw
done; done
rm -f /tmp/sd.data
