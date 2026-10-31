#!/bin/bash
# Gating run for round 11: base main 555689b, m 6e24c3c, p 171463d, mp 532d88d. Paths adapted to the A/B helper's VM.
set -euo pipefail
OUT=$HOME/ab/r11/out; W=$HOME/ab/r11/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
[ "$cpu" = "@ 2.90GHz" ] || { echo "host is $cpu, not the bar machine; stopping" > $OUT/STOPPED; exit 4; }
for pair in "base 555689b" "m 6e24c3c" "p 171463d" "mp 532d88d"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "base = main 555689b, m = perf/h264-mb-records 6e24c3c, p = perf/h264-predict-glue 171463d, mp = perf/h264-mb-round11 532d88d"
  sha256sum $W/decode-base $W/decode-m $W/decode-p $W/decode-mp
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name"
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of the base tree $(git -C $W/src-base rev-parse HEAD); identical across the four trees"
  sha256sum $W/src-base/scripts/h264_parity.py $W/src-m/scripts/h264_parity.py $W/src-p/scripts/h264_parity.py $W/src-mp/scripts/h264_parity.py
  echo "streams checked against docs/measurements/h264-training/es.json of the base tree:"
  python3 -I -c "
import json,hashlib
d=json.load(open('$W/src-base/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$E/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print(len(d)-len(bad),'of',len(d),'match; differing:',bad)"
} > $OUT/provenance.txt
: > $OUT/identity.txt
for s in $E/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$($W/decode-base --frames $F --threads 1 $s | md5sum | cut -c1-32)
  st=same; line="$(basename $s) $F $b"
  for d in m p mp; do for t in 1 8; do
    h=$($W/decode-$d --frames $F --threads $t $s | md5sum | cut -c1-32)
    line="$line $d$t=$h"; [ "$h" = "$b" ] || st=DIFFERENT
  done; done
  echo "$line $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder m=$W/decode-m --decoder p=$W/decode-p --decoder mp=$W/decode-mp \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $E/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
cp $W/src-mp/docs/measurements/h264-rounds/round11/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
