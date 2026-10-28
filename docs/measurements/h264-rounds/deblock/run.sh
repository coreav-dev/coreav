#!/bin/bash
# Gating run for DB-1..DB-4 and DB-C: base 304aeb70 (chroma round), A 7313c8ef, C 58b8e4ba.
set -euo pipefail
OUT=/tmp/dbrun/out; mkdir -p $OUT
REPO=$HOME/coreav
for pair in "base 304aeb70" "a 7313c8ef" "c 58b8e4ba"; do
  set -- $pair; name=$1; rev=$2
  rm -rf /tmp/dbrun/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach /tmp/dbrun/src-$name $rev >/dev/null 2>&1
  (cd /tmp/dbrun/src-$name && CARGO_TARGET_DIR=/tmp/dbrun/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp /tmp/dbrun/tgt-$name/dist/examples/decode /tmp/dbrun/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=/tmp/dbrun/tgt-<name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "base = 304aeb70 (perf/h264-chroma-mc-inline), a = 7313c8ef, c = 58b8e4ba (perf/h264-deblock-branchfree)"
  sha256sum /tmp/dbrun/decode-base /tmp/dbrun/decode-a /tmp/dbrun/decode-c
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping"
  $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg -version | head -1
} > $OUT/provenance.txt
# DB-4 identity: whole decoded output; base at 1 thread, a and c at 1 and 8 threads.
: > $OUT/identity.txt
for s in $HOME/h264work/training/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$(/tmp/dbrun/decode-base --frames $F --threads 1 $s | md5sum | cut -c1-32)
  st=same; line="$(basename $s) $F $b"
  for d in a c; do for t in 1 8; do
    h=$(/tmp/dbrun/decode-$d --frames $F --threads $t $s | md5sum | cut -c1-32)
    line="$line $d$t=$h"; [ "$h" = "$b" ] || st=DIFFERENT
  done; done
  echo "$line $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing"; exit 3; fi
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  python3 -I $REPO/scripts/h264_parity.py --decoder base=/tmp/dbrun/decode-base --decoder a=/tmp/dbrun/decode-a --decoder c=/tmp/dbrun/decode-c \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $HOME/h264work/training/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done"
done
python3 -I /tmp/dbrun/analyze.py $OUT g
