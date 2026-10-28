#!/bin/bash
# Second run of the build-alignment round (docs/design/h264.md, "Build round: code alignment", "Second run"):
# run-ab-helper.sh BASE K, the first run's arguments. The arms are the first run's binaries (the same files,
# checked by hash, not rebuilt); every bar AL-1 to AL-5 is measured again, the eight-thread calls with ten
# rounds. Host gates, voiding and attempts as in the first run's script (which this is a copy of).
set -euo pipefail
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || { echo "RUSTFLAGS is set; stopping" >&2; exit 4; }
BASE=$1; K=$2
HOSTGATE=${HOSTGATE:-$HOME/hg/hostgate.py}
OUT=$HOME/ab/align2/out; W=$HOME/ab/align/w; mkdir -p $OUT
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
: > $W/alignment2.txt
# The first run's arms, the same files.
echo "52b09f171ea026c018f1f0318a3175d876a6b651277e0e002b99bba9cc056d9c  $W/decode-base
07d17e2a382c1fd41ea83600e37bc1ce4d3393aa65c7f8984619479061a16104  $W/decode-k" | sha256sum -c - || { echo "arms differ from the first run; stopping" > $OUT/STOPPED; exit 3; }
for name in base k; do
  nm -n --defined-only $W/decode-$name | awk -v n=$name '$2=="t"||$2=="T"{c++; if (strtonum("0x"$1)%64==0) a++} END{print n, "text symbols", c, "on 64-byte lines", a}' >> $W/alignment2.txt
done
# K's text must be on 64-byte lines (all but the few symbols not compiled from Rust).
awk '$1=="k" && $8*10 < $4*9 {bad=1} END{exit bad}' $W/alignment2.txt || { echo "k is not aligned; stopping" > $OUT/STOPPED; exit 3; }
{
  echo "the binaries of the first run (build-alignment/gating/provenance.txt)"
  echo "base = $BASE, k = $K"
  sha256sum $W/decode-base $W/decode-k $HOSTGATE
  cat $W/alignment2.txt
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
# Two calls a class: one thread pinned to core 2 (g), then eight threads, unpinned (t8).
for mode in g t8; do
if [ $mode = g ]; then OPTS="--pin 2 --rounds 5"; else OPTS="--threads 8 --rounds 10"; fi
for c in sd:1800 720p:900 1080p:300 4k:150; do
  cls=${c%%:*}; F=${c##*:}; done_ok=0
  for attempt in 1 2 3; do
    ok=0; for i in $(seq 10); do gate "before $mode.$cls #$attempt" && { ok=1; break; }; sleep 60; done
    [ $ok = 1 ] || { echo "host noisy before $mode.$cls; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 5; }
    python3 -I $W/src-base/scripts/h264_parity.py --decoder base=$W/decode-base --decoder k=$W/decode-k \
      --ffmpeg $FF --frames $F $OPTS --require-cpu "$REQ" --report $OUT/$mode.$cls.json \
      $E/*.$cls.h264 > $OUT/$mode.$cls.log 2>&1
    if gate "after $mode.$cls #$attempt"; then done_ok=1; break; fi
    mv $OUT/$mode.$cls.json $OUT/$mode.$cls.json.void$attempt; mv $OUT/$mode.$cls.log $OUT/$mode.$cls.log.void$attempt
    echo "VOID $mode.$cls attempt $attempt $(date -u +%H:%M)" | tee -a $OUT/hostgate.log >> $OUT/progress.txt
  done
  [ $done_ok = 1 ] || { echo "$mode.$cls void three times; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 6; }
  echo "$mode.$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
done
done
# The analysis beside this script (K, the last code commit, predates it).
cp "$(dirname "$0")/../analyze.py" $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
