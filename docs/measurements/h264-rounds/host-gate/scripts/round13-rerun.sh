#!/bin/bash
# Round 13 re-measure: base main 9cc143d, g perf/h264-mc-glue-2 14435e9, r perf/h264-mc-ref-13b a445499, gr perf/h264-mc-glue-r 22c6fbd.
# Host-health gate before and after every timing call; main may have moved since, the base stays 9cc143d (recorded).
set -euo pipefail
OUT=$HOME/ab/r13b/out; W=$HOME/ab/r13b/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
[ "$cpu" = "@ 2.90GHz" ] || { echo "host is $cpu, not the bar machine; stopping" > $OUT/STOPPED; exit 4; }
git -C $REPO fetch -q origin main perf/h264-mc-glue-2 perf/h264-mc-ref-13b perf/h264-mc-glue-r
echo "origin/main at the start of the run: $(git -C $REPO rev-parse --short=8 origin/main) (the base stays 9cc143d)" > $OUT/main-at-start.txt
for pair in "base 9cc143d" "g 14435e9" "r a445499" "gr 22c6fbd"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
  rm -rf $W/tgt-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "base = main 9cc143d, g = perf/h264-mc-glue-2 14435e9, r = perf/h264-mc-ref-13b a445499, gr = perf/h264-mc-glue-r 22c6fbd"
  cat $OUT/main-at-start.txt
  sha256sum $W/decode-base $W/decode-g $W/decode-r $W/decode-gr
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name|^CPU\(s\)|Thread|Core|Socket"
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of the base tree $(git -C $W/src-base rev-parse HEAD); identical across the four trees"
  sha256sum $W/src-base/scripts/h264_parity.py $W/src-g/scripts/h264_parity.py $W/src-r/scripts/h264_parity.py $W/src-gr/scripts/h264_parity.py
  echo "streams checked against docs/measurements/h264-training/es.json of the base tree:"
  python3 -I -c "
import json,hashlib
d=json.load(open('$W/src-base/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$E/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print(len(d)-len(bad),'of',len(d),'match; differing:',bad)"
  echo "host-health gate: see hostgate.log (rule in HOST-GATE.md)"
} > $OUT/provenance.txt
: > $OUT/identity.txt
for s in $E/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$($W/decode-base --frames $F --threads 1 $s | md5sum | cut -c1-32)
  st=same; line="$(basename $s) $F $b"
  for d in g r gr; do for t in 1 8; do
    h=$($W/decode-$d --frames $F --threads $t $s | md5sum | cut -c1-32)
    line="$line $d$t=$h"; [ "$h" = "$b" ] || st=DIFFERENT
  done; done
  echo "$line $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
gated_call() { # REPORT LOG cmd... : host-health gate before and after; a failed "after" voids the call and re-runs it
  report=$1; log=$2; shift 2; attempt=0
  while true; do
    waited=0
    until python3 $HOME/ab/hostgate.py "before $(basename $report)" >> $OUT/hostgate.log 2>&1; do
      sleep 60; waited=$((waited+1))
      if [ $waited -gt 90 ]; then echo "host noisy for more than 90 minutes before $(basename $report); stopping" > $OUT/STOPPED; exit 7; fi
    done
    "$@" > $log 2>&1
    if python3 $HOME/ab/hostgate.py "after $(basename $report)" >> $OUT/hostgate.log 2>&1; then return 0; fi
    attempt=$((attempt+1)); mv $report $report.void$attempt 2>/dev/null || true; mv $log $log.void$attempt 2>/dev/null || true
    echo "VOID $(basename $report) attempt $attempt (after-check failed); re-running" >> $OUT/hostgate.log
    if [ $attempt -ge 3 ]; then echo "$(basename $report) voided 3 times; stopping" > $OUT/STOPPED; exit 7; fi
  done
}
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  gated_call $OUT/g.$cls.json $OUT/g.$cls.log timeout 7200 python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder g=$W/decode-g --decoder r=$W/decode-r --decoder gr=$W/decode-gr \
    --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "@ 2.90GHz" --report $OUT/g.$cls.json $E/*.$cls.h264
  echo "g $cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}
  gated_call $OUT/t8.$cls.json $OUT/t8.$cls.log timeout 7200 python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder g=$W/decode-g \
    --ffmpeg $FF --frames $F --rounds 3 --threads 8 --require-cpu "@ 2.90GHz" --report $OUT/t8.$cls.json $E/*.$cls.h264
  echo "t8 $cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
cp $W/src-gr/docs/measurements/h264-rounds/round13/analyze.py $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g t8 > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
