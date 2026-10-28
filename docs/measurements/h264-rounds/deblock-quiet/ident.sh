#!/bin/bash
# ident.sh A B: md5 of the output of /tmp/slow/bin-A and bin-B on the 32 training streams at the class frame counts, 1 and 8 threads.
for f in $(ls /tmp/pmu/up/es); do for b in $1 $2; do for t in 1 8; do echo "$f $b $t"; done; done; done | \
  xargs -P 8 -L 1 bash -c 'f=$0; b=$1; t=$2; c=$(echo $f | cut -d. -f3); case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac; m=$(/tmp/slow/bin-$b --frames $n --threads $t /tmp/pmu/up/es/$f | md5sum | cut -c1-32); echo "$m ${f:0:12} $b $t"'
