#!/bin/bash
# Gating run for round 10: base perf/h264-mb-motion 325784d vs perf/h264-motion-store 4eea757.
set -euo pipefail
OUT=/tmp/r10run/out; mkdir -p $OUT
REPO=$HOME/coreav
for pair in "before 325784d" "after 4eea757"; do
  set -- $pair; name=$1; rev=$2
  rm -rf /tmp/r10run/src-$name; git -C $REPO worktree add -f --detach /tmp/r10run/src-$name $rev >/dev/null 2>&1
  (cd /tmp/r10run/src-$name && CARGO_TARGET_DIR=/tmp/r10run/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp /tmp/r10run/tgt-$name/dist/examples/decode /tmp/r10run/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=/tmp/r10run/tgt-<name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "before = perf/h264-mb-motion 325784d, after = perf/h264-motion-store 4eea757"
  sha256sum /tmp/r10run/decode-before /tmp/r10run/decode-after
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping"
  $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg -version | head -1
  sha256sum $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
  echo "harness: scripts/h264_parity.py and round8/analyze.py from the before tree $(git -C /tmp/r10run/src-before rev-parse HEAD)"
  sha256sum /tmp/r10run/src-before/scripts/h264_parity.py /tmp/r10run/src-before/docs/measurements/h264-rounds/round8/analyze.py
  echo "streams checked against docs/measurements/h264-training/es.json:"
  python3 -I -c "
import json,hashlib
d=json.load(open('/tmp/r10run/src-before/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$HOME/h264work/training/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print(len(d)-len(bad),'of',len(d),'match; differing:',bad)"
} > $OUT/provenance.txt
# R10-4 identity: whole decoded output, before at 1 thread, after at 1 and 8 threads.
: > $OUT/identity.txt
for s in $HOME/h264work/training/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$(/tmp/r10run/decode-before --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a1=$(/tmp/r10run/decode-after --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a8=$(/tmp/r10run/decode-after --frames $F --threads 8 $s | md5sum | cut -c1-32)
  st=same; [ "$b" = "$a1" ] && [ "$b" = "$a8" ] || st=DIFFERENT
  echo "$(basename $s) $F $b $a1 $a8 $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing"; exit 3; fi
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  python3 -I /tmp/r10run/src-before/scripts/h264_parity.py --decoder before=/tmp/r10run/decode-before --decoder after=/tmp/r10run/decode-after \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $HOME/h264work/training/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done"
done
cp /tmp/r10run/src-before/docs/measurements/h264-rounds/round8/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g
