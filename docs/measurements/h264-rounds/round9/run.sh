#!/bin/bash
# Gating run for round 9 (implementer e39f2f16), factorial: base main 5dcea041, b perf/h264-idct a1cc6cf, ab perf/h264-mb-motion 325784d.
# Supersedes the two-build request (base vs 1ea19de), which was stopped before it started.
set -euo pipefail
OUT=/tmp/r9run/out; mkdir -p $OUT
REPO=$HOME/coreav
for pair in "base 5dcea041" "b a1cc6cf" "ab 325784d"; do
  set -- $pair; name=$1; rev=$2
  rm -rf /tmp/r9run/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach /tmp/r9run/src-$name $rev >/dev/null 2>&1
  (cd /tmp/r9run/src-$name && CARGO_TARGET_DIR=/tmp/r9run/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp /tmp/r9run/tgt-$name/dist/examples/decode /tmp/r9run/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=/tmp/r9run/tgt-<name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "base = main 5dcea041, b = perf/h264-idct a1cc6cf, ab = perf/h264-mb-motion 325784d"
  sha256sum /tmp/r9run/decode-base /tmp/r9run/decode-b /tmp/r9run/decode-ab
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping"
  $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg -version | head -1
  sha256sum $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
  echo "harness: scripts/h264_parity.py from the base tree $(git -C /tmp/r9run/src-base rev-parse HEAD); analyze.py from the ab tree $(git -C /tmp/r9run/src-ab rev-parse HEAD)"
  sha256sum /tmp/r9run/src-base/scripts/h264_parity.py /tmp/r9run/src-ab/docs/measurements/h264-rounds/round9/analyze.py
  echo "streams checked against docs/measurements/h264-training/es.json:"
  python3 -I -c "
import json,hashlib
d=json.load(open('/tmp/r9run/src-base/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$HOME/h264work/training/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print(len(d)-len(bad),'of',len(d),'match; differing:',bad)"
} > $OUT/provenance.txt
# R9-4 identity: whole decoded output; base at 1 thread, b and ab at 1 and 8 threads (128 comparisons).
: > $OUT/identity.txt
for s in $HOME/h264work/training/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$(/tmp/r9run/decode-base --frames $F --threads 1 $s | md5sum | cut -c1-32)
  st=same; line="$(basename $s) $F $b"
  for d in b ab; do for t in 1 8; do
    h=$(/tmp/r9run/decode-$d --frames $F --threads $t $s | md5sum | cut -c1-32)
    line="$line $d$t=$h"; [ "$h" = "$b" ] || st=DIFFERENT
  done; done
  echo "$line $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing"; exit 3; fi
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  python3 -I /tmp/r9run/src-base/scripts/h264_parity.py --decoder base=/tmp/r9run/decode-base --decoder b=/tmp/r9run/decode-b --decoder ab=/tmp/r9run/decode-ab \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $HOME/h264work/training/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done"
done
cp /tmp/r9run/src-ab/docs/measurements/h264-rounds/round9/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g
