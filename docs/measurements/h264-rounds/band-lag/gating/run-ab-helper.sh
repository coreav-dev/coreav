#!/bin/bash
# Gating run for L1..L5 (docs/design/h264.md, "Threading round: a band published when its rows are
# filtered"): base = main 9cc143d3, a = perf/h264-band-lag at $AFTER. Run t (threads 1,8 unpinned)
# and run p (1 thread pinned), the same two binaries; then CPU time at 8 threads (information).
set -euo pipefail
BEFORE=9cc143d3; AFTER=${1:?the branch tip to measure}
OUT=$HOME/ab/bandlag/out; W=$HOME/ab/bandlag/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
[ "$cpu" = "@ 2.90GHz" ] || { echo "host is $cpu, not the bar machine; stopping" > $OUT/STOPPED; exit 4; }
git -C $REPO fetch -q origin main perf/h264-band-lag
for pair in "base $BEFORE" "a $AFTER"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264); the same two binaries for both runs"
  echo "base = main $BEFORE, a = $AFTER (perf/h264-band-lag)"
  sha256sum $W/decode-base $W/decode-a
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name|^CPU\(s\)|Thread\(s\) per core|Core\(s\) per socket|Socket\(s\)"
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of the base tree $(git -C $W/src-base rev-parse HEAD); identical in the a tree"
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
  b=$($W/decode-base --frames $F --threads 1 $s | md5sum | cut -c1-32); line="$(basename $s) $F $b"; st=same
  for t in 1 2 3 8; do
    a=$($W/decode-a --frames $F --threads $t $s | md5sum | cut -c1-32); line="$line $a"
    [ "$a" = "$b" ] || st=DIFFERENT
  done
  echo "$line $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
runset() { prefix=$1; shift
  for c in sd:1800 720p:900 1080p:300 4k:150; do
    cls=${c%%:*}; F=${c##*:}
    timeout 9000 python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder a=$W/decode-a \
      --ffmpeg $FF --frames $F --rounds 5 "$@" --require-cpu "@ 2.90GHz" --report $OUT/$prefix.$cls.json \
      $E/*.$cls.h264 > $OUT/$prefix.$cls.log 2>&1
    echo "$prefix $cls done $(date -u +%H:%M)" >> $OUT/progress.txt
  done; }
runset t --threads 1,8
runset p --pin 2
cp $W/src-a/docs/measurements/h264-rounds/band-lag/analyze.py $W/src-a/docs/measurements/h264-rounds/band-lag/cpu8.py $OUT/
python3 -I $OUT/analyze.py $OUT > $OUT/analysis.txt
for c in sd:1800 720p:900 1080p:300 4k:150; do
  python3 -I $OUT/cpu8.py $OUT/cpu8.${c%%:*}.json ${c##*:} $W/decode-base $W/decode-a $E/*.${c%%:*}.h264 > $OUT/cpu8.${c%%:*}.txt 2>&1
done
echo ALL >> $OUT/progress.txt
