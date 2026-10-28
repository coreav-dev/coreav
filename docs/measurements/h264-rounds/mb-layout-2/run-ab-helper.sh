#!/bin/bash
# Gating run for the second MB layer layout round: run-ab-helper.sh BEFORE AFTER (BEFORE: main; AFTER: the round's
# code commit). Bars ML2-1 to ML2-7 of docs/design/h264.md, "MB layer round 2: the layout without the added
# instructions". Derived from the small-bundle round's script (bundle/gating/run-ab-helper.sh), with the PGO round's
# held-out call. Every timing call is bracketed by the host gate (HOST-GATE.md, HOSTGATE below): a gate before it
# (retried a minute apart, at most 10 times) and after it; a failed after-gate voids the call (kept as *.void<N>) and
# it runs again, at most 3 attempts. Calls, per class: g (one thread pinned to core 2, ten rounds, the 32 training
# streams), t8 (eight threads unpinned, ten rounds), gm (as g, with MALLOC_MMAP_THRESHOLD_=131072 for the whole call:
# the placement guard), h (as g, the 64 held-out streams). Paths are this VM's.
set -euo pipefail
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || { echo "RUSTFLAGS is set; stopping" >&2; exit 4; }
[ -z "${MALLOC_MMAP_THRESHOLD_:-}" ] || { echo "MALLOC_MMAP_THRESHOLD_ is set; stopping" >&2; exit 4; }
BASE=$1; K=$2
HOSTGATE=${HOSTGATE:-$HOME/hg/hostgate.py}
OUT=$HOME/ab/mb-layout-2/out; W=$HOME/ab/mb-layout-2/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es; H=$HOME/hd/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
# The bars are fixed for the 2.90 GHz class (family 6, model 106) alone; both arms run in the same calls on one
# host, and the class is recorded.
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
model=$(lscpu | awk -F: '/^Model:/{gsub(/ /,"",$2); print $2}')
case "$cpu/$model" in
  "@ 2.90GHz/106") ;;
  *) echo "host is $cpu model $model, not a bar class; stopping" > $OUT/STOPPED; exit 4;;
esac
REQ="$cpu"
git -C $REPO fetch -q origin
for pair in "before $BASE" "after $K"; do
  set -- $pair; name=$1; rev=$2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name
done
{
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt-name> cargo build --locked --profile dist --example decode -p coreav-h264)"
  echo "before = $BASE, after = $K"
  sha256sum $W/decode-before $W/decode-after $HOSTGATE
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name"
  echo "class: $cpu model $model"
  grep -o 'avx512[a-z_0-9]*' /proc/cpuinfo | sort -u | tr '\n' ' '; echo
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of the base tree $(git -C $W/src-before rev-parse HEAD); identical in the branch trees"
  sha256sum $W/src-*/scripts/h264_parity.py
  echo "streams checked against es.json of the base tree:"
  python3 -I -c "
import json,hashlib
for lab,f,e in (('training','h264-training','$E'),('held-out','h264-n2-heldout','$H')):
    d=json.load(open('$W/src-before/docs/measurements/'+f+'/es.json'))
    bad=[n for n,v in d.items() if hashlib.sha256(open(e+'/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
    print(lab,len(d)-len(bad),'of',len(d),'match; differing:',bad)"
} > $OUT/provenance.txt
# ML2-7, on this host: the 32 training streams at one and eight threads, and the 64 held-out ones at one thread.
: > $OUT/identity.txt
for s in $E/*.h264 $H/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  threads="1 8"; case $s in $H/*) threads=1;; esac
  b=$($W/decode-before --frames $F --threads 1 $s | md5sum | cut -c1-32)
  st=same; r=""
  for t in $threads; do
    m=$($W/decode-after --frames $F --threads $t $s | md5sum | cut -c1-32); r="$r $m"
    [ "$m" = "$b" ] || st=DIFFERENT
  done
  echo "$(basename $s) $F $b$r $st" >> $OUT/identity.txt
done
echo "identity: $(grep -c ' same$' $OUT/identity.txt) same of $(wc -l < $OUT/identity.txt)"
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
gate() { python3 -I $HOSTGATE "$1" >> $OUT/hostgate.log 2>&1; }
for call in g t8 gm h; do
  ENVS=""; D=$E
  case $call in
    g)  OPTS="--pin 2 --rounds 10";;
    t8) OPTS="--threads 8 --rounds 10";;
    gm) OPTS="--pin 2 --rounds 10"; ENVS="MALLOC_MMAP_THRESHOLD_=131072";;
    h)  OPTS="--pin 2 --rounds 10"; D=$H;;
  esac
  for c in sd:1800 720p:900 1080p:300 4k:150; do
    cls=${c%%:*}; F=${c##*:}; done_ok=0
    for attempt in 1 2 3; do
      ok=0; for i in $(seq 10); do gate "before $call.$cls #$attempt" && { ok=1; break; }; sleep 60; done
      [ $ok = 1 ] || { echo "host noisy before $call.$cls; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 5; }
      env $ENVS python3 -I $W/src-before/scripts/h264_parity.py --decoder before=$W/decode-before --decoder after=$W/decode-after \
        --ffmpeg $FF --frames $F $OPTS --require-cpu "$REQ" --report $OUT/$call.$cls.json \
        $D/*.$cls.h264 > $OUT/$call.$cls.log 2>&1
      if gate "after $call.$cls #$attempt"; then done_ok=1; break; fi
      mv $OUT/$call.$cls.json $OUT/$call.$cls.json.void$attempt; mv $OUT/$call.$cls.log $OUT/$call.$cls.log.void$attempt
      echo "VOID $call.$cls attempt $attempt $(date -u +%H:%M)" | tee -a $OUT/hostgate.log >> $OUT/progress.txt
    done
    [ $done_ok = 1 ] || { echo "$call.$cls void three times; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 6; }
    echo "$call.$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
  done
done
# The analysis beside this script.
cp "$(dirname "$0")/analyze.py" $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
