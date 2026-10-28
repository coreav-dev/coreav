#!/bin/bash
# Gating run for WP-1..WP-4 (docs/design/h264.md, "Kernel round: weighted prediction
# inlined"): run.sh BEFORE AFTER, the base on main and the branch tip.
set -euo pipefail
BEFORE=$1; AFTER=$2
OUT=/tmp/wprun/out; mkdir -p $OUT
REPO=$HOME/coreav
for pair in "before $BEFORE" "after $AFTER"; do
  set -- $pair; name=$1; rev=$2
  rm -rf /tmp/wprun/src-$name; git -C $REPO worktree add -f --detach /tmp/wprun/src-$name $rev >/dev/null 2>&1
  (cd /tmp/wprun/src-$name && CARGO_TARGET_DIR=/tmp/wprun/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp /tmp/wprun/tgt-$name/dist/examples/decode /tmp/wprun/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=/tmp/wprun/tgt-<name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "before = $BEFORE (main), after = $AFTER (perf/h264-combine)"
  sha256sum /tmp/wprun/decode-before /tmp/wprun/decode-after
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping"
  $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg -version | head -1
} > $OUT/provenance.txt
# WP-4 identity: whole decoded output, before at 1 thread, after at 1 and 8 threads.
: > $OUT/identity.txt
for s in $HOME/h264work/training/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$(/tmp/wprun/decode-before --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a1=$(/tmp/wprun/decode-after --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a8=$(/tmp/wprun/decode-after --frames $F --threads 8 $s | md5sum | cut -c1-32)
  st=same; [ "$b" = "$a1" ] && [ "$b" = "$a8" ] || st=DIFFERENT
  echo "$(basename $s) $F $b $a1 $a8 $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing"; exit 3; fi
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  python3 -I $REPO/scripts/h264_parity.py --decoder before=/tmp/wprun/decode-before --decoder after=/tmp/wprun/decode-after \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $HOME/h264work/training/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done"
done
cp /tmp/wprun/src-after/docs/measurements/h264-rounds/combine/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g
