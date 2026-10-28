#!/bin/bash
# Gating run for the PGO round (docs/design/pgo.md, bars PG-1 to PG-7): run-pgo-helper.sh COMMIT
# (the commit that holds the note; both arms are built from it on this host with one toolchain).
# Derived from the deblock-driver round's script. Every timing call is bracketed by the host gate
# (HOST-GATE.md): a gate before it (retried a minute apart, at most 10 times) and after it; a failed
# after-gate voids the call (kept as *.void<N>) and it runs again, at most 3 attempts. Calls: g (fast,
# 1 thread pinned to core 2, 5 rounds, 32 training streams), t8 (fast, 8 threads unpinned, 10 rounds),
# gs (safe, as g), h (fast, as g, the 64 held-out streams). Paths are this VM's.
set -euo pipefail
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || { echo "RUSTFLAGS is set; stopping" >&2; exit 4; }
C=$1
HOSTGATE=${HOSTGATE:-$HOME/hg/hostgate.py}
OUT=$HOME/ab/pgo/out; W=$HOME/ab/pgo/w; mkdir -p $OUT $W
REPO=$HOME/coreav; E=$HOME/tr/es; H=$HOME/hd/es; J=$HOME/jvt/extracted
FF=$HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg
cpu=$(grep -m1 "model name" /proc/cpuinfo | grep -o "@ [0-9.]*GHz")
model=$(lscpu | awk -F: '/^Model:/{gsub(/ /,"",$2); print $2}')
case "$cpu/$model" in
  "@ 2.90GHz/106") ;;
  *) echo "host is $cpu model $model, not a bar class; stopping" > $OUT/STOPPED; exit 4;;
esac
REQ="$cpu"
git -C $REPO fetch -q origin
rm -rf $W/src; git -C $REPO worktree prune
git -C $REPO worktree add -f --detach $W/src $C >/dev/null 2>&1
cd $W/src
for m in fast safe; do
  flag=""; [ $m = safe ] && flag="--no-default-features"
  CARGO_TARGET_DIR=$W/tgt-before-$m cargo build --locked --profile dist --example decode -p coreav-h264 $flag 2>&1 | tail -1
  cp $W/tgt-before-$m/dist/examples/decode $W/decode-before-$m
  python3 scripts/pgo_build.py --mode $m --extracted $J --work $W/pgo --out $W/decode-after-$m > $OUT/pgo-build-$m.log 2>&1
  ls -l $W/pgo/$m.profdata >> $OUT/pgo-build-$m.log
  rm -rf $W/tgt-before-$m $W/pgo/gen-$m $W/pgo/use-$m $W/pgo/raw-$m
done
cd $HOME
{
  echo "commit $C (the commit that holds docs/design/pgo.md; both arms built from it)"
  echo "before: cargo build --locked --profile dist --example decode -p coreav-h264 [--no-default-features]; after: python3 scripts/pgo_build.py --mode fast|safe --extracted <97 JVT bitstreams> --work <dir> --out <bin>"
  sha256sum $W/decode-* $W/pgo/*.profdata $HOSTGATE
  rustc -vV
  grep -m1 "model name" /proc/cpuinfo
  lscpu | grep -E "^Model:|CPU family|Stepping|Model name"
  echo "class: $cpu model $model"
  grep -o 'avx512[a-z_0-9]*' /proc/cpuinfo | sort -u | tr '\n' ' '; echo
  $FF -version | head -1
  sha256sum $FF
  echo "harness: scripts/h264_parity.py of $C"; sha256sum $W/src/scripts/h264_parity.py
  echo "training streams checked against docs/measurements/h264-training/es.json:"
  python3 -I -c "
import json,hashlib
d=json.load(open('$W/src/docs/measurements/h264-training/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$E/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print(len(d)-len(bad),'of',len(d),'match; differing:',bad)
d=json.load(open('$W/src/docs/measurements/h264-n2-heldout/es.json'))
bad=[n for n,v in d.items() if hashlib.sha256(open('$H/'+n,'rb').read()).hexdigest()!=v['es_sha256']]
print('held-out:',len(d)-len(bad),'of',len(d),'match; differing:',bad)"
} > $OUT/provenance.txt
# PG-7: each after build against its before build, on the 32 training streams at 1 and 8 threads and on the
# JVT bitstreams at 1 thread (output md5 and exit status).
: > $OUT/identity.txt
for m in fast safe; do
  for s in $E/*.h264; do
    case $s in *.sd.*) F=1800;; *.720p.*) F=900;; *.1080p.*) F=300;; *.4k.*) F=150;; esac
    b=$($W/decode-before-$m --frames $F --threads 1 $s | md5sum | cut -c1-32)
    st=same; r=""
    for t in 1 8; do
      x=$($W/decode-after-$m --frames $F --threads $t $s | md5sum | cut -c1-32); r="$r $x"
      [ "$x" = "$b" ] || st=DIFFERENT
    done
    echo "$m train $(basename $s) $F $b$r $st" >> $OUT/identity.txt
  done
  python3 -I - $W/decode-before-$m $W/decode-after-$m $J $W/src/crates/coreav-h264/tests/data/conformance-streams.json $m >> $OUT/identity.txt <<'PY'
import glob, hashlib, json, os, subprocess, sys
b, a, ext, listing, mode = sys.argv[1:6]
def run(binary, f):
    h = hashlib.md5()
    p = subprocess.Popen([binary, f], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    for chunk in iter(lambda: p.stdout.read(1 << 20), b''):
        h.update(chunk)
    return p.wait(timeout=900), h.hexdigest()[:32]
for s in json.load(open(listing))['streams']:
    if not s['in_scope']:
        continue
    name = os.path.basename(s['bitstream'])
    f = glob.glob(os.path.join(ext, s['name'], '**', name), recursive=True)[0]
    rb, ra = run(b, f), run(a, f)
    print(mode, 'conf', s['name'], rb[0], rb[1], ra[0], ra[1], 'same' if rb == ra else 'DIFFERENT')
PY
done
echo "identity: $(grep -c ' same$' $OUT/identity.txt) same of $(wc -l < $OUT/identity.txt)"
if grep -q DIFFERENT $OUT/identity.txt; then echo "identity failed; not timing" > $OUT/STOPPED; exit 3; fi
gate() { python3 -I $HOSTGATE "$1" >> $OUT/hostgate.log 2>&1; }
for call in g t8 gs h; do
  case $call in
    g)  OPTS="--pin 2 --rounds 5"; M=fast; D=$E;;
    t8) OPTS="--threads 8 --rounds 10"; M=fast; D=$E;;
    gs) OPTS="--pin 2 --rounds 5"; M=safe; D=$E;;
    h)  OPTS="--pin 2 --rounds 5"; M=fast; D=$H;;
  esac
  for c in sd:1800 720p:900 1080p:300 4k:150; do
    cls=${c%%:*}; F=${c##*:}; done_ok=0
    for attempt in 1 2 3; do
      ok=0; for i in $(seq 10); do gate "before $call.$cls #$attempt" && { ok=1; break; }; sleep 60; done
      [ $ok = 1 ] || { echo "host noisy before $call.$cls; stopping" | tee -a $OUT/progress.txt > $OUT/STOPPED; exit 5; }
      python3 -I $W/src/scripts/h264_parity.py --decoder before=$W/decode-before-$M --decoder after=$W/decode-after-$M \
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
cp "$(dirname "$0")/analyze.py" $OUT/analyze.py
python3 -I $OUT/analyze.py $OUT > $OUT/analysis.txt
echo ALL >> $OUT/progress.txt
