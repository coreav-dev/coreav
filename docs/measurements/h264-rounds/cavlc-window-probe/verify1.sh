#!/bin/bash
# verify1.sh FILE FRAC: callgrind Ir of bin-cw-base and bin-cw-pad (frames = class count / FRAC), and md5 at a tenth
f=$1; d=$2; c=$(echo $f | cut -d. -f3); s=$(echo $f | cut -d. -f1)
case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
for b in base pad; do
  valgrind --tool=callgrind --callgrind-out-file=/dev/null /tmp/rb/bin-cw-$b --frames $((n/d)) --discard /tmp/pmu/up/es/$f >/dev/null 2>/tmp/win/v.$s.$b.log
  ir=$(grep "Collected" /tmp/win/v.$s.$b.log | awk '{print $4}')
  md=$(/tmp/rb/bin-cw-$b --frames $((n/10)) /tmp/pmu/up/es/$f 2>/dev/null | md5sum | cut -c1-32)
  echo "$s $c $b $d $ir $md"
done
