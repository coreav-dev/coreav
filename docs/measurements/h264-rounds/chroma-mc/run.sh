#!/bin/bash
# Gating run for CM-1..CM-4: base main 5dcea041 vs branch e116fbd.
set -euo pipefail
OUT=/tmp/cmrun/out; mkdir -p $OUT
REPO=$HOME/coreav
for pair in "before 5dcea041" "after e116fbd"; do
  set -- $pair; name=$1; rev=$2
  rm -rf /tmp/cmrun/src-$name; git -C $REPO worktree add -f --detach /tmp/cmrun/src-$name $rev >/dev/null 2>&1
  (cd /tmp/cmrun/src-$name && CARGO_TARGET_DIR=/tmp/cmrun/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp /tmp/cmrun/tgt-$name/dist/examples/decode /tmp/cmrun/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=/tmp/cmrun/tgt-<name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "before = main 5dcea041, after = perf/h264-chroma-mc-inline e116fbd"
  sha256sum /tmp/cmrun/decode-before /tmp/cmrun/decode-after
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping"
  $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg -version | head -1
} > $OUT/provenance.txt
# CM-4 identity: whole decoded output, before at 1 thread, after at 1 and 8 threads.
: > $OUT/identity.txt
for s in $HOME/h264work/training/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$(/tmp/cmrun/decode-before --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a1=$(/tmp/cmrun/decode-after --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a8=$(/tmp/cmrun/decode-after --frames $F --threads 8 $s | md5sum | cut -c1-32)
  st=same; [ "$b" = "$a1" ] && [ "$b" = "$a8" ] || st=DIFFERENT
  echo "$(basename $s) $F $b $a1 $a8 $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing"; exit 3; fi
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  python3 -I $REPO/scripts/h264_parity.py --decoder before=/tmp/cmrun/decode-before --decoder after=/tmp/cmrun/decode-after \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $HOME/h264work/training/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done"
done
cp $REPO/docs/measurements/h264-rounds/round8/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g
