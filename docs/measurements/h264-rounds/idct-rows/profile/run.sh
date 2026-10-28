#!/bin/bash
# perf part of docs/measurements/h264-gap-2/tools/run.sh, sides ours and ffsimd only.
set -u
G=/tmp/prof/gap; STREAMS=$HOME/h264work/training
OURS=/tmp/prof/tgt/dist/examples/decode; FFMPEG=$HOME/ffbuild/ffmpeg-9.0.2/ffmpeg_g
cmd_of() {
  case $1 in
    ours) echo "$OURS --frames $3 --discard $2";;
    ffsimd) echo "$FFMPEG -hide_banner -v error -threads 1 -i $2 -frames:v $3 -f null -";;
  esac
}
: > $G/wall.txt
while read -r id cls F; do
  f=$(ls $STREAMS/$id*.h264)
  for side in ours ffsimd; do
    cmd=$(cmd_of $side "$f" "$F")
    timeout 600 sudo perf record -q -e cpu-clock -F 999 -o $G/perf/$id.$side.data taskset -c 2 $cmd </dev/null >/dev/null 2>&1
    sudo perf report -i $G/perf/$id.$side.data --stdio --no-children -n --sort symbol </dev/null 2>/dev/null | grep -E "^ +[0-9]" > $G/perf/$id.$side.txt
    sudo perf script -i $G/perf/$id.$side.data -F ip </dev/null 2>/dev/null > $G/perf/$id.$side.ips
    t=$(timeout 600 sudo perf stat -r 3 taskset -c 2 $cmd </dev/null 2>&1 >/dev/null | grep "time elapsed" | awk '{print $1, $3}')
    echo "$id $cls $side $t" | tee -a $G/wall.txt
  done
done < $G/streams.txt
