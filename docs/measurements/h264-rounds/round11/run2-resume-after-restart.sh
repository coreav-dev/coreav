#!/bin/bash
# Resume after a VM restart: identity (192/192) and the sd and 720p calls had finished; the 1080p call was killed mid-way
# (no report written). Same binaries, same settings, remaining classes only.
set -euo pipefail
OUT=$HOME/ab/r11/out; W=$HOME/ab/r11/w; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
[ "$cpu" = "@ 2.90GHz" ] || { echo "host is $cpu, not the bar machine; stopping" > $OUT/STOPPED; exit 4; }
mv $OUT/g.1080p.log $OUT/g.1080p.killed-by-restart.log 2>/dev/null || true
for c in 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  timeout 7200 python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder m=$W/decode-m --decoder p=$W/decode-p --decoder mp=$W/decode-mp \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $E/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done $(date -u +%m-%d\ %H:%M) (after restart)" >> $OUT/progress.txt
done
cp $W/src-mp/docs/measurements/h264-rounds/round11/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
