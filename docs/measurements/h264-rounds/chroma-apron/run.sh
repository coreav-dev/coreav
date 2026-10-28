#!/bin/bash
# run.sh: the chroma-apron probe (plan-probe.md, chroma-apron): one harness call per class, 8 threads, unpinned.
H=~/wt41/scripts/h264_parity.py; E=~/h264work/train/es; B=~/h264work/bins; O=~/h264work/r32/cap
FF=~/h264work/ff902/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
for c in sd:1800 720p:900 1080p:300 4k:150; do cls=${c%%:*}; F=${c##*:}
  for attempt in 1 2 3; do
    hx=0; python3 -I $H --decoder base=$B/m-b366429 --decoder probe=$B/r32-capron2 --ffmpeg $FF --frames $F \
      --threads 8 --rounds 10 --shuffle-seed 20261027 --null-arm base --gate-stream $E/36501d448eaa.0.720p.h264 \
      --gate-every 4 --report $O/cap.$cls.json $E/*.$cls.h264 > $O/cap.$cls.log 2>&1 || hx=$?
    if [ $hx = 0 ]; then
      un=$(python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]))['method']['unresolved'])" $O/cap.$cls.json)
      [ "$un" -le 2 ] && { echo "$cls done attempt $attempt unresolved $un" >> $O/progress.txt; break; }
      echo "$cls VOID attempt $attempt unresolved $un" >> $O/progress.txt
    else echo "$cls VOID attempt $attempt harness exit $hx" >> $O/progress.txt; fi
    [ ! -e $O/cap.$cls.json ] || mv $O/cap.$cls.json $O/cap.$cls.json.void$attempt; mv $O/cap.$cls.log $O/cap.$cls.log.void$attempt
  done
done
echo ALL >> $O/progress.txt
