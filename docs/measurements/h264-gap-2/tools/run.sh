#!/bin/bash
# The runs of docs/measurements/h264-gap-2, on one machine, one core.
#
#   GAP2=WORKDIR STREAMS=DIR OURS=decode FFMPEG=ffmpeg_g tools/run.sh
#
# WORKDIR/streams.txt lists "id class frames" per line. Writes, per stream
# and decoder (ours, ffsimd, ffc): perf/<id>.<side>.data, .txt (by symbol)
# and .ips (sample addresses), cg/<id>.<side>.out (callgrind) and wall.txt.
set -u
cmd_of() {
  local side=$1 f=$2 F=$3
  case $side in
    ours) echo "$OURS --frames $F --discard $f";;
    ffsimd) echo "$FFMPEG -hide_banner -v error -threads 1 -i $f -frames:v $F -f null -";;
    ffc) echo "$FFMPEG -hide_banner -v error -cpuflags 0 -threads 1 -i $f -frames:v $F -f null -";;
  esac
}
mkdir -p "$GAP2/perf" "$GAP2/cg"
: > "$GAP2/wall.txt"
while read -r id cls F; do
  f=$(ls "$STREAMS"/$id*.h264)
  for side in ours ffsimd ffc; do
    cmd=$(cmd_of $side "$f" "$F")
    # FFmpeg reads stdin, hence </dev/null throughout.
    sudo perf record -q -e cpu-clock -F 999 -o "$GAP2/perf/$id.$side.data" taskset -c 2 $cmd </dev/null >/dev/null 2>&1
    sudo perf report -i "$GAP2/perf/$id.$side.data" --stdio --no-children -n --sort symbol </dev/null 2>/dev/null | grep -E "^ +[0-9]" > "$GAP2/perf/$id.$side.txt"
    sudo perf script -i "$GAP2/perf/$id.$side.data" -F ip </dev/null 2>/dev/null > "$GAP2/perf/$id.$side.ips"
    t=$(sudo perf stat -r 3 taskset -c 2 $cmd </dev/null 2>&1 >/dev/null | grep "time elapsed" | awk '{print $1, $3}')
    echo "$id $cls $side $t" >> "$GAP2/wall.txt"
  done
done < "$GAP2/streams.txt"
# callgrind runs in parallel: simulated counts do not depend on load.
while read -r id cls F; do
  f=$(ls "$STREAMS"/$id*.h264)
  for side in ours ffsimd ffc; do
    echo "timeout 3600 valgrind --tool=callgrind --dump-instr=yes --cache-sim=yes --branch-sim=yes --callgrind-out-file=$GAP2/cg/$id.$side.out $(cmd_of $side "$f" "$F") </dev/null >/dev/null 2>$GAP2/cg/$id.$side.log"
  done
done < "$GAP2/streams.txt" | xargs -P 8 -I{} bash -c "{}"
