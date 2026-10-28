#!/bin/bash
# cgfull.sh STREAMS A B: callgrind instruction totals of /tmp/slow/bin-A and bin-B, one thread, --discard, the class frame counts.
for s in $(echo $1 | tr , ' '); do for b in $2 $3; do echo "$s $b"; done; done | \
  xargs -P 8 -L 1 bash -c 's=$0; b=$1; f=$(ls /tmp/pmu/up/es | grep ^$s); c=$(echo $f | cut -d. -f3); case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac; o=/tmp/slow/cgfull/$s.$b.out; [ -s $o ] || valgrind --tool=callgrind --callgrind-out-file=$o /tmp/slow/bin-$b --frames $n --threads 1 --discard /tmp/pmu/up/es/$f >/dev/null 2>&1; echo "$s $b $(grep ^summary: $o | cut -d" " -f2)"'
