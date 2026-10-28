#!/bin/bash
# ident1.sh FILE: md5 of bin-mw-base and bin-sl-c at the class frame count, 1 and 8 threads; callgrind Ir of both
# (CAVLC streams at the class frame count, the rest at a tenth)
f=$1; c=$(echo $f | cut -d. -f3); s=$(echo $f | cut -d. -f1)
case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
for t in 1 8; do
  a=$(/tmp/rb/bin-mw-base --threads $t --frames $n /tmp/pmu/up/es/$f 2>/dev/null | md5sum | cut -c1-32)
  b=$(/tmp/rb/bin-sl-c --threads $t --frames $n /tmp/pmu/up/es/$f 2>/dev/null | md5sum | cut -c1-32)
  echo "id $s $c t$t $a $b $([ $a = $b ] && echo identical || echo DIFFERENT)"
done
case $s in 5185721dda11|6f7799768413|9db41aa16b5a|b71b6b31e54f|ec473c99c538|f9e973f44570) d=1;; *) d=10;; esac
for b in base c; do
  case $b in base) bin=/tmp/rb/bin-mw-base;; c) bin=/tmp/rb/bin-sl-c;; esac
  valgrind --tool=callgrind --callgrind-out-file=/dev/null $bin --frames $((n/d)) --discard /tmp/pmu/up/es/$f >/dev/null 2>/tmp/skp/i/i.$s.$b.log
  echo "ir $s $c $b 1/$d $(grep Collected /tmp/skp/i/i.$s.$b.log | awk '{print $4}')"
done
