#!/bin/bash
# Gating run for the AVX-512 deblocking round: run-ab-helper.sh BASE A (BASE: main at the commit the
# branch is based on; A: the branch tip). Bars AV-1 to AV-4 of docs/design/h264.md, "Kernel round: an
# AVX-512 tier". Paths are the A/B helper VM's.
set -euo pipefail
BASE=$1; A=$2
OUT=$HOME/ab/avx512/out; W=$HOME/ab/avx512/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
[ "$cpu" = "@ 2.90GHz" ] || { echo "host is $cpu, not the bar machine; stopping" > $OUT/STOPPED; exit 4; }
# The tier needs AVX-512 F, BW, VL and VBMI; without them both builds run the same kernels.
for f in avx512f avx512bw avx512vl avx512vbmi; do
  grep -qw $f /proc/cpuinfo || { echo "host lacks $f; stopping" > $OUT/STOPPED; exit 5; }
done
git -C $REPO fetch -q origin
for pair in "base $BASE" "a $A"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "base = main $BASE, a = the branch $A"
  sha256sum $W/decode-base $W/decode-a
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name"
  grep -m1 -o 'avx512[a-z_0-9]*' /proc/cpuinfo | sort -u | tr '\n' ' '; echo
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of the base tree $(git -C $W/src-base rev-parse HEAD); identical in the branch tree"
  sha256sum $W/src-base/scripts/h264_parity.py $W/src-a/scripts/h264_parity.py
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
  a1=$($W/decode-a --frames $F --threads 1 $s | md5sum | cut -c1-32)
  a8=$($W/decode-a --frames $F --threads 8 $s | md5sum | cut -c1-32)
  st=same; [ "$b" = "$a1" ] && [ "$b" = "$a8" ] || st=DIFFERENT
  echo "$(basename $s) $F $b $a1 $a8 $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder a=$W/decode-a \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json \
    $E/*.$cls.h264 > $OUT/g.$cls.log 2>&1
  echo "$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
cp $W/src-a/docs/measurements/h264-rounds/avx512-deblock/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
