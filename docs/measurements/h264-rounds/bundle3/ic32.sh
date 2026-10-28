#!/bin/bash
# ic32.sh OUT BIN... : callgrind instruction totals (--discard, 1 thread) of each bin on the 32 training streams,
# frames by class (sd 600, 720p 300, 1080p 100, 4k 50)
out=$1; shift; : > $out
for f in ~/h264work/train/es/*.h264; do s=$(basename $f .h264)
  case $s in *.sd) n=600;; *.720p) n=300;; *.1080p) n=100;; *) n=50;; esac
  for b in "$@"; do echo "$s $b $n $f"; done
done | xargs -P 8 -L 1 bash -c 'c=$(timeout 3000 valgrind --tool=callgrind --callgrind-out-file=/dev/null ~/h264work/bins/$1 --threads ${T:-1} --frames $2 --discard $3 2>&1 >/dev/null | sed -n "s/.*Collected : //p"); echo "$0 $1 $c"' >> $out
echo done >> $out
