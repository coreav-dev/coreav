#!/bin/sh
# perfab.sh STREAM_PREFIX FRAMES REPS BIN... : perf cpu-clock 10 kHz, pinned cpu 3, alternating builds; pd/perf.BIN.STREAM.REP.data
ES=$HOME/h264work/train/es; s=$1; f=$(ls $ES | grep ^$s); fr=$2; n=$3; shift 3
i=0; while [ $i -lt $n ]; do for b in "$@"; do
  taskset -c 3 perf record -q -e cpu-clock -F 10000 -o pd/perf.$b.$s.$i.data $HOME/h264work/bins/$b --frames $fr --discard $ES/$f >/dev/null 2>&1
done; i=$((i+1)); done
