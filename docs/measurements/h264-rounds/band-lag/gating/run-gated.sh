#!/bin/bash
# Gating of the band publication round with the host-health gate before and after each call (the method from 2026-10-23).
# base = main 9cc143d3, a = 9d7aca35 (decoder code of 544f954e). Paths are this VM's; adapt them for another host.
set -uo pipefail
BEFORE=9cc143d3; AFTER=9d7aca35
OUT=/tmp/mt/ab/out; W=/tmp/mt/ab/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/h264work/training
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
export ES_DIR=$E FFMPEG=$FF
GATE="python3 -I /tmp/mt/gate/hostgate.py"
gate_before() { for i in $(seq 1 240); do $GATE "before $1" >> $OUT/hostgate.log 2>&1 && return 0; sleep 60; done; echo "gate never passed before $1" >> $OUT/STOPPED; exit 5; }
gated() { label=$1; report=$2; log=$3; shift 3
  for v in 1 2 3 4; do
    gate_before "$label"
    "$@" > $log 2>&1
    if $GATE "after $label" >> $OUT/hostgate.log 2>&1; then return 0; fi
    echo "void $label #$v" >> $OUT/hostgate.log
    mv $report $report.void$v 2>/dev/null; mv $log $log.void$v
  done; echo "$label voided 4 times" >> $OUT/STOPPED; exit 6; }
for pair in "base $BEFORE" "a $AFTER"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264); the same two binaries for both runs"
  echo "base = main $BEFORE, a = $AFTER (perf/h264-band-lag; its decoder code is 544f954e's, later commits change tests and comments only)"
  sha256sum $W/decode-base $W/decode-a
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name|^CPU\(s\)|Thread\(s\) per core|Core\(s\) per socket|Socket\(s\)"
  $FF -version | head -1
  sha256sum $FF
  echo "host gate: docs/measurements/h264-rounds/host-gate/hostgate.py of measured/host-gate 5c289339, sha256 $(sha256sum /tmp/mt/gate/hostgate.py | cut -c1-64)"
  echo "harness: scripts/h264_parity.py of the base tree $(git -C $W/src-base rev-parse HEAD)"
  sha256sum $W/src-base/scripts/h264_parity.py $W/src-a/scripts/h264_parity.py
  python3 -I -c "
import json,hashlib
d=json.load(open('$W/src-base/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$E/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print('streams:',len(d)-len(bad),'of',len(d),'match es.json; differing:',bad)"
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
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
runset() { prefix=$1; shift
  for c in sd:1800 720p:900 1080p:300 4k:150; do
    cls=${c%%:*}; F=${c##*:}
    gated "$prefix.$cls" $OUT/$prefix.$cls.json $OUT/$prefix.$cls.log timeout 9000 python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder a=$W/decode-a \
      --ffmpeg $FF --frames $F --rounds 5 "$@" --report $OUT/$prefix.$cls.json $E/*.$cls.h264
    echo "$prefix $cls done $(date -u +%H:%M)" >> $OUT/progress.txt
  done; }
runset t --threads 1,8
runset p --pin 2
cp $W/src-a/docs/measurements/h264-rounds/band-lag/analyze.py $W/src-a/docs/measurements/h264-rounds/band-lag/cpu8.py $OUT/
python3 -I $OUT/analyze.py $OUT > $OUT/analysis.txt
for c in sd:1800 720p:900 1080p:300 4k:150; do
  gated "cpu8.${c%%:*}" $OUT/cpu8.${c%%:*}.json $OUT/cpu8.${c%%:*}.txt python3 -I $OUT/cpu8.py $OUT/cpu8.${c%%:*}.json ${c##*:} $W/decode-base $W/decode-a $E/*.${c%%:*}.h264
done
echo ALL >> $OUT/progress.txt
