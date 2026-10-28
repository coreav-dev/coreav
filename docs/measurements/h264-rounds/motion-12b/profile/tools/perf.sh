#!/bin/bash
S=$HOME/h264work/training; FF=$HOME/ffbuild/ffmpeg-9.0.2/ffmpeg_g
while read id cls n; do f=$(ls $S/$id*.h264)
 for rep in 1 2 3; do
 sudo perf record -q -e cpu-clock -F 4999 -o perf/$id.ours.$rep.data taskset -c 2 /tmp/mbc/decode --frames $n --discard $f </dev/null >/dev/null 2>&1
 sudo perf record -q -e cpu-clock -F 4999 -o perf/$id.ffc.$rep.data taskset -c 2 $FF -hide_banner -v error -cpuflags 0 -threads 1 -i $f -frames:v $n -f null - </dev/null >/dev/null 2>&1
 done
done <<L
41e324ebb708 sd 600
5185721dda11 sd 600
9db41aa16b5a 720p 300
1d4b1775d78a 720p 300
6da34958296c 1080p 100
b71b6b31e54f 1080p 100
3b1b441e23ae 4k 40
e624cbe0d2be 4k 40
L
for d in perf/*.data; do sudo perf report -i $d --stdio --no-children -n --sort symbol 2>/dev/null | grep -E '^ +[0-9]' > ${d%.data}.txt; done
echo done
