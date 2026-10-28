#!/bin/bash
# callgrind with branch simulation: ours (main 686fb48f) and FFmpeg C-only, gap-2 streams, fewer frames.
S=$HOME/h264work/training; FF=$HOME/ffbuild/ffmpeg-9.0.2/ffmpeg_g
run() { id=$1; n=$2; f=$(ls $S/$id*.h264)
  timeout 3000 valgrind -q --tool=callgrind --branch-sim=yes --dump-instr=yes --callgrind-out-file=/tmp/mbc/cg/$id.ours.out /tmp/mbc/decode --frames $n --discard $f >/dev/null 2>&1
  timeout 3000 valgrind -q --tool=callgrind --branch-sim=yes --dump-instr=yes --callgrind-out-file=/tmp/mbc/cg/$id.ffc.out $FF -hide_banner -v error -cpuflags 0 -threads 1 -i $f -frames:v $n -f null - >/dev/null 2>&1
  echo $id done; }
run 41e324ebb708 600 & run 5185721dda11 600 & run 9db41aa16b5a 300 & run 1d4b1775d78a 300 &
run 6da34958296c 100 & run b71b6b31e54f 100 & run 3b1b441e23ae 40 & run e624cbe0d2be 40 &
wait; echo ALL > /tmp/mbc/cg/done
