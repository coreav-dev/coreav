#!/bin/bash
# Gating run for DP-1..DP-4: base main e6d810cf, a 824746a9 (perf/h264-deblock-chroma-pair). Paths adapted to the A/B helper's VM.
set -euo pipefail
BEFORE=e6d810cf; AFTER=824746a9
OUT=$HOME/ab/dp/out; W=$HOME/ab/dp/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
[ "$cpu" = "@ 2.90GHz" ] || { echo "host is $cpu, not the bar machine; stopping" > $OUT/STOPPED; exit 4; }
for pair in "before $BEFORE" "after $AFTER"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "before = main $BEFORE, after = $AFTER (perf/h264-deblock-chroma-pair)"
  sha256sum $W/decode-before $W/decode-after
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name"
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of the before tree $(git -C $W/src-before rev-parse HEAD); identical in the after tree"
  sha256sum $W/src-before/scripts/h264_parity.py $W/src-after/scripts/h264_parity.py
  echo "streams checked against docs/measurements/h264-training/es.json of the before tree:"
  python3 -I -c "
import json,hashlib
d=json.load(open('$W/src-before/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$E/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print(len(d)-len(bad),'of',len(d),'match; differing:',bad)"
} > $OUT/provenance.txt
: > $OUT/identity.txt
for s in $E/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$($W/decode-before --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a1=$($W/decode-after --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a8=$($W/decode-after --frames $F --threads 8 $s | md5sum | cut -c1-32)
  st=same; [ "$b" = "$a1" ] && [ "$b" = "$a8" ] || st=DIFFERENT
  echo "$(basename $s) $F $b $a1 $a8 $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  timeout 7200 python3 -I $W/src-before/scripts/h264_parity.py --decoder before=$W/decode-before --decoder after=$W/decode-after \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $E/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
cp $W/src-after/docs/measurements/h264-rounds/deblock-pair/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
