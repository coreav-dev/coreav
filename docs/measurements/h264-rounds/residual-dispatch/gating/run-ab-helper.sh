#!/bin/bash
# Gating run for the entropy round "the residual's block dispatch" (#480): run-ab-helper.sh BASE K
# (BASE: main e2271b3a; K: the branch's code commit fded8560). Bars RD-1 to RD-6 of docs/design/h264.md, "Entropy round:
# the residual's block dispatch". Harness: scripts/h264_parity.py of main acf3ee2f, beside this script, with the options of
# the method section: --shuffle-seed 20261026 --null-arm base --gate-stream 36501d448eaa.0.720p.h264 --gate-every 4.
# Calls per class: g (one thread pinned to core 2, 20 rounds, 32 training streams; `base`, `k`, and `knofast` = k built with
# --no-default-features, information), t8 (eight threads unpinned, 10 rounds, --min-seconds 3; base and k; the in-call gate
# is one thread on core 2 at 900 frames repeated to 3 s, --gate-threads 1 --gate-pin 2), h (one thread pinned, 5 rounds, the
# 64 held-out streams; base and k). Every call is bracketed by the host gate (HOST-GATE.md): a gate before it (retried a
# minute apart, at most 10 times) and after it; a failed after-gate voids the call (kept as *.void<N>) and it runs again, at
# most 3 attempts. A stream still unresolved after its retries stays in the medians and is listed.
set -euo pipefail
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || { echo "RUSTFLAGS is set; stopping" >&2; exit 4; }
BASE=$1; K=$2
HERE=$(cd "$(dirname "$0")" && pwd)
HARNESS=$HERE/h264_parity_acf3ee2f.py
HOSTGATE=${HOSTGATE:-$HOME/hg/hostgate.py}
OUT=$HOME/ab/residual/out; W=$HOME/ab/residual/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es; H=$HOME/hd/es
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
GATE=$E/36501d448eaa.0.720p.h264
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
model=$(lscpu | awk -F: '/^Model:/{gsub(/ /,"",$2); print $2}')
case "$cpu/$model" in
  "@ 2.90GHz/106") ;;
  *) echo "host is $cpu model $model, not a bar class; stopping" > $OUT/STOPPED; exit 4;;
esac
REQ="$cpu"
git -C $REPO fetch -q origin
build() {  # build NAME REV [cargo flags]
  name=$1; rev=$2; shift 2
  rm -rf $W/src-$name; git -C $REPO worktree prune
  git -C $REPO worktree add -f --detach $W/src-$name $rev >/dev/null 2>&1
  (cd $W/src-$name && CARGO_TARGET_DIR=$W/tgt-$name cargo build --locked --profile dist --example decode -p coreav-h264 "$@" 2>&1 | tail -1)
  cp $W/tgt-$name/dist/examples/decode $W/decode-$name; rm -rf $W/tgt-$name
}
build base $BASE
build k $K
build knofast $K --no-default-features
{
  echo "base = $BASE, k = $K, knofast = $K built with --no-default-features"
  echo "build: (cd <worktree at rev> && CARGO_TARGET_DIR=<tgt> cargo build --locked --profile dist --example decode -p coreav-h264 [--no-default-features])"
  sha256sum $W/decode-base $W/decode-k $W/decode-knofast $HOSTGATE $HARNESS
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name"
  echo "class: $cpu model $model"
  $FF -version | head -1
  sha256sum $FF
  python3 -I -c "
import json,hashlib
d=json.load(open('$W/src-base/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$E/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print('training',len(d)-len(bad),'of',len(d),'match; differing:',bad)
d=json.load(open('$W/src-base/docs/measurements/h264-n2-heldout/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$H/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print('held-out',len(d)-len(bad),'of',len(d),'match; differing:',bad)"
} > $OUT/provenance.txt
: > $OUT/identity.txt
for s in $E/*.h264; do
  case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
  b=$($W/decode-base --frames $F --threads 1 $s | md5sum | cut -c1-32)
  st=same; r=""
  for n in k knofast; do for t in 1 8; do
    m=$($W/decode-$n --frames $F --threads $t $s | md5sum | cut -c1-32); r="$r $n$t=${m:0:8}"
    [ "$m" = "$b" ] || st=DIFFERENT
  done; done
  echo "$(basename $s) $F ${b:0:8}$r $st" >> $OUT/identity.txt
done
echo "identity: $(grep -c ' same$' $OUT/identity.txt) of $(wc -l < $OUT/identity.txt)"
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
gate() { python3 -I $HOSTGATE "$1" >> $OUT/hostgate.log 2>&1; }
for call in g t8 h; do
  case $call in
    g)  OPTS="--pin 2 --rounds 20"; D=$E; DEC="--decoder base=$W/decode-base --decoder k=$W/decode-k --decoder knofast=$W/decode-knofast";;
    t8) OPTS="--threads 8 --rounds 10 --min-seconds 3 --gate-frames 900 --gate-threads 1 --gate-pin 2"; D=$E; DEC="--decoder base=$W/decode-base --decoder k=$W/decode-k";;
    h)  OPTS="--pin 2 --rounds 5"; D=$H; DEC="--decoder base=$W/decode-base --decoder k=$W/decode-k";;
  esac
  for c in sd:1800 720p:900 1080p:300 4k:150; do
    cls=${c%%:*}; F=${c##*:}; done_ok=0
    for attempt in 1 2 3; do
      ok=0; for i in $(seq 10); do gate "before $call.$cls #$attempt" && { ok=1; break; }; sleep 60; done
      [ $ok = 1 ] || { echo "host noisy before $call.$cls; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 5; }
      python3 -I $HARNESS $DEC --ffmpeg $FF --frames $F $OPTS --require-cpu "$REQ" --shuffle-seed 20261026 --null-arm base \
        --gate-stream $GATE --gate-every 4 --report $OUT/$call.$cls.json $D/*.$cls.h264 > $OUT/$call.$cls.log 2>&1
      if gate "after $call.$cls #$attempt"; then done_ok=1; break; fi
      mv $OUT/$call.$cls.json $OUT/$call.$cls.json.void$attempt; mv $OUT/$call.$cls.log $OUT/$call.$cls.log.void$attempt
      echo "VOID $call.$cls attempt $attempt $(date -u +%H:%M)" | tee -a $OUT/hostgate.log >> $OUT/progress.txt
    done
    [ $done_ok = 1 ] || { echo "$call.$cls void three times; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 6; }
    echo "$call.$cls done $(date -u +%H:%M)" >> $OUT/progress.txt
  done
done
cp "$HERE/analyze.py" $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
