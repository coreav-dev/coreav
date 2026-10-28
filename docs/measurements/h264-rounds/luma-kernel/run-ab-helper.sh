#!/bin/bash
# Gating run for the luma kernel round: run-ab-helper.sh BASE K (BASE: main, the commit the branch is
# based on; K: the branch's last code commit). Bars LK-1 to LK-5 of docs/design/h264.md, "Kernel round:
# luma interpolation by byte pairs". Every timing call is bracketed by the host gate (HOST-GATE.md, HOSTGATE
# below): a gate before it (retried a minute apart, at most 10 times) and after it; a failed after-gate
# voids the call (kept as *.void<N>) and it runs again, at most 3 attempts. Paths are the A/B helper VM's.
set -euo pipefail
BASE=$1; K=$2
HOSTGATE=${HOSTGATE:-$HOME/hg/hostgate.py}
OUT=$HOME/ab/luma/out; W=$HOME/ab/luma/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
# Either bar class of the round method: the 2.90 GHz class (family 6, model 106) or the
# 2.50 GHz class (model 85); both arms run in the same calls on one host, and the class is recorded.
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
model=$(lscpu | awk -F: '/^Model:/{gsub(/ /,"",$2); print $2}')
case "$cpu/$model" in
  "@ 2.90GHz/106"|"@ 2.50GHz/85") ;;
  *) echo "host is $cpu model $model, not a bar class; stopping" > $OUT/STOPPED; exit 4;;
esac
REQ="$cpu"
git -C $REPO fetch -q origin
for pair in "base $BASE" "k $K"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "base = $BASE, k = $K"
  sha256sum $W/decode-base $W/decode-k $HOSTGATE
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name"
  echo "class: $cpu model $model"
  grep -o 'avx512[a-z_0-9]*' /proc/cpuinfo | sort -u | tr '\n' ' '; echo
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of the base tree $(git -C $W/src-base rev-parse HEAD); identical in the branch trees"
  sha256sum $W/src-*/scripts/h264_parity.py
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
  st=same; r=""
  for n in k; do for t in 1 8; do
    m=$($W/decode-$n --frames $F --threads $t $s | md5sum | cut -c1-32); r="$r $m"
    [ "$m" = "$b" ] || st=DIFFERENT
  done; done
  echo "$(basename $s) $F $b$r $st" >> $OUT/identity.txt
done
grep -c " same$" $OUT/identity.txt
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
gate() { python3 -I $HOSTGATE "$1" >> $OUT/hostgate.log 2>&1; }
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}; done_ok=0
  for attempt in 1 2 3; do
    ok=0; for i in $(seq 10); do gate "before g.$cls #$attempt" && { ok=1; break; }; sleep 60; done
    [ $ok = 1 ] || { echo "host noisy before g.$cls; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 5; }
    python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder k=$W/decode-k \
      --ffmpeg $FF --frames $F --rounds 5 --pin 2 --require-cpu "$REQ" --report $OUT/g.$cls.json \
      $E/*.$cls.h264 > $OUT/g.$cls.log 2>&1
    if gate "after g.$cls #$attempt"; then done_ok=1; break; fi
    mv $OUT/g.$cls.json $OUT/g.$cls.json.void$attempt; mv $OUT/g.$cls.log $OUT/g.$cls.log.void$attempt
    echo "VOID g.$cls attempt $attempt $(date -u +%H:%M)" | tee -a $OUT/hostgate.log >> $OUT/progress.txt
  done
  [ $done_ok = 1 ] || { echo "g.$cls void three times; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 6; }
  echo "$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
# The analysis beside this script (K, the last code commit, predates it).
cp "$(dirname "$0")/analyze.py" $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT g > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
