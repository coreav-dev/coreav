#!/bin/bash
# verify1.sh FILE: callgrind Ir of bin-mw-base and bin-mw-pad (CAVLC streams at the class frame count, the rest at a
# tenth), and the output md5 at a tenth
f=$1; c=$(echo $f | cut -d. -f3); s=$(echo $f | cut -d. -f1)
case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
case $s in 5185721dda11|6f7799768413|9db41aa16b5a|b71b6b31e54f|ec473c99c538|f9e973f44570) d=1;; *) d=10;; esac
for b in base pad; do
  valgrind --tool=callgrind --callgrind-out-file=/dev/null /tmp/rb/bin-mw-$b --frames $((n/d)) --discard /tmp/pmu/up/es/$f >/dev/null 2>/tmp/t3/v/v.$s.$b.log
  ir=$(grep "Collected" /tmp/t3/v/v.$s.$b.log | awk '{print $4}')
  md=$(/tmp/rb/bin-mw-$b --frames $((n/10)) /tmp/pmu/up/es/$f 2>/dev/null | md5sum | cut -c1-32)
  echo "$s $c $b 1/$d $ir $md"
done
