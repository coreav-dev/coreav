#!/bin/bash
# perf of the inline(never) build on the eight profiled streams; symbol totals.
S=$HOME/h264work/training; D=/tmp/intra/tgt-ni/dist/examples/decode; O=/tmp/intra/perf; mkdir -p $O
while read -r id cls F; do
  f=$(ls $S/$id*.h264)
  timeout 300 sudo perf record -q -e cpu-clock -F 999 -o $O/$id.data taskset -c 2 $D --frames $F --discard $f </dev/null >/dev/null 2>&1
  sudo perf report -i $O/$id.data --stdio --no-children -n --sort symbol </dev/null 2>/dev/null | grep -E "^ +[0-9]" > $O/$id.txt
done < $HOME/coreav/docs/measurements/h264-rounds/idct-rows/profile/streams.txt
cat $O/*.txt | grep -E "intra|reconstruct_intra|idct4x4_add|Scalar" | awk '{n=$2; $1=$2=$3=$4=""; a[$0]+=n} END{for(k in a) printf "%6d ms %s\n", a[k], k}' | sort -rn
